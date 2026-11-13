//! `buffer(0)` of a polygon or multipolygon, as GEOS computes it for an invalid input.
//!
//! GEOS (`BufferCurveSetBuilder::addPolygon`, `OffsetCurveBuilder::getRingCurve`) emits the
//! rings of the input unchanged for a distance of 0 and labels each ring's two sides interior
//! and exterior by its orientation (`Orientation::isCCWArea`: the sign of its shoelace area,
//! not of its lobes), a hole labelled opposite to a shell. `BufferBuilder` nodes the curves
//! and keeps the edges that separate a depth of at least 1 (`BufferSubgraph::findResultEdges`,
//! `targetRequiredDepth` 1) from a smaller one; `PolygonBuilder` links them into shells
//! (clockwise) and holes (counter-clockwise).
//!
//! The depth of a point is the sum over the rings of `role * winding`, where the winding is
//! counted clockwise for a ring GEOS takes as clockwise and counter-clockwise for one it takes
//! as counter-clockwise, and `role` is +1 for a shell, -1 for a hole. So a bowtie keeps its
//! clockwise lobe only, overlapping parts (depth 2) are dissolved and a hole drops the depth of
//! what it covers back to 0.

use std::collections::HashMap;
use std::f64::consts::PI;

use geo::{Coord, Geometry, LineString, MultiPolygon, Polygon};

use crate::noding::key;
use crate::polygonize::{assemble, node_lines_with};

type C = Coord<f64>;

/// GEOS `math::DD`: a double-double number (`hi + lo`), with the operations of `DD.cpp` in the
/// same order so every rounding matches.
#[derive(Clone, Copy)]
struct Dd {
    hi: f64,
    lo: f64,
}

const SPLIT: f64 = 134_217_729.0;

impl Dd {
    fn new(x: f64) -> Self {
        Dd { hi: x, lo: 0.0 }
    }

    fn add(self, y: Dd) -> Dd {
        let s = self.hi + y.hi;
        let t = self.lo + y.lo;
        let e = s - self.hi;
        let f = t - self.lo;
        let s2 = s - e;
        let t2 = t - f;
        let s3 = (y.hi - e) + (self.hi - s2);
        let t3 = (y.lo - f) + (self.lo - t2);
        let e2 = s3 + t;
        let h_big = s + e2;
        let h = e2 + (s - h_big);
        let e3 = t3 + h;
        let zhi = h_big + e3;
        Dd {
            hi: zhi,
            lo: e3 + (h_big - zhi),
        }
    }

    fn neg(self) -> Dd {
        Dd {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    fn sub(self, y: Dd) -> Dd {
        self.add(y.neg())
    }

    fn mul(self, y: Dd) -> Dd {
        let c1 = SPLIT * self.hi;
        let hx0 = c1 - self.hi;
        let c2 = SPLIT * y.hi;
        let hx = c1 - hx0;
        let tx = self.hi - hx;
        let hy0 = c2 - y.hi;
        let big_c = self.hi * y.hi;
        let hy = c2 - hy0;
        let ty = y.hi - hy;
        let c = ((((hx * hy - big_c) + hx * ty) + tx * hy) + tx * ty)
            + (self.hi * y.lo + self.lo * y.hi);
        let zhi = big_c + c;
        let hx2 = big_c - zhi;
        Dd {
            hi: zhi,
            lo: c + hx2,
        }
    }

    fn div(self, y: Dd) -> Dd {
        let big_c = self.hi / y.hi;
        let c = SPLIT * big_c;
        let hc0 = c - big_c;
        let u = SPLIT * y.hi;
        let hc = c - hc0;
        let tc = big_c - hc;
        let hy0 = u - y.hi;
        let big_u = big_c * y.hi;
        let hy = u - hy0;
        let ty = y.hi - hy;
        let u2 = (((hc * hy - big_u) + hc * ty) + tc * hy) + tc * ty;
        let c2 = ((((self.hi - big_u) - u2) + self.lo) - big_c * y.lo) / y.hi;
        let u3 = big_c + c2;
        Dd {
            hi: u3,
            lo: (big_c - u3) + c2,
        }
    }

    fn to_f64(self) -> f64 {
        self.hi + self.lo
    }
}

/// GEOS `CGAlgorithmsDD::intersection`: the crossing of the lines `p1 p2` and `q1 q2`.
pub(crate) fn geos_intersection(p1: C, p2: C, q1: C, q2: C) -> Option<C> {
    let (p1x, p1y, p2x, p2y) = (Dd::new(p1.x), Dd::new(p1.y), Dd::new(p2.x), Dd::new(p2.y));
    let (q1x, q1y, q2x, q2y) = (Dd::new(q1.x), Dd::new(q1.y), Dd::new(q2.x), Dd::new(q2.y));
    let (px, py) = (p1y.sub(p2y), p2x.sub(p1x));
    let pw = p1x.mul(p2y).sub(p2x.mul(p1y));
    let (qx, qy) = (q1y.sub(q2y), q2x.sub(q1x));
    let qw = q1x.mul(q2y).sub(q2x.mul(q1y));
    let x = py.mul(qw).sub(qy.mul(pw));
    let y = qx.mul(pw).sub(px.mul(qw));
    let w = px.mul(qy).sub(qx.mul(py));
    let (xi, yi) = (x.div(w).to_f64(), y.div(w).to_f64());
    (xi.is_finite() && yi.is_finite()).then_some(Coord { x: xi, y: yi })
}

struct Ring {
    pts: Vec<C>,
    /// +1: depth grows with the counter-clockwise winding, -1: with the clockwise winding.
    sign: i32,
}

/// GEOS `Area::ofRingSigned` (positive for a clockwise ring, same operation order).
fn signed_area_geos(ring: &[C]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let x0 = ring[0].x;
    let mut sum = 0.0;
    for i in 1..ring.len() - 1 {
        let x = ring[i].x - x0;
        sum += x * (ring[i - 1].y - ring[i + 1].y);
    }
    sum / 2.0
}

fn clean(ring: &LineString<f64>) -> Vec<C> {
    let mut out: Vec<C> = Vec::with_capacity(ring.0.len());
    for c in &ring.0 {
        if out.last() != Some(c) {
            out.push(*c);
        }
    }
    out
}

/// The rings GEOS offers to the curve set builder, labelled.
fn rings_of(polygons: &[&Polygon<f64>]) -> Vec<Ring> {
    let mut rings = Vec::new();
    for polygon in polygons {
        let shell = clean(polygon.exterior());
        // GEOS skips a polygon whose shell has fewer than three distinct vertices.
        if shell.len() < 3 {
            continue;
        }
        let holes = polygon.interiors().iter().map(clean);
        for (role, pts) in std::iter::once((1, shell)).chain(holes.map(|h| (-1, h))) {
            if pts.len() < 4 {
                continue;
            }
            // `isCCWArea`: the signed area is negative for a counter-clockwise ring.
            let ccw = signed_area_geos(&pts) < 0.0;
            rings.push(Ring {
                pts,
                sign: role * if ccw { 1 } else { -1 },
            });
        }
    }
    rings
}

/// Winding number of `ring` around `q` (counter-clockwise positive).
fn winding(ring: &[C], q: C) -> i32 {
    let mut w = 0;
    for s in ring.windows(2) {
        let (a, b) = (s[0], s[1]);
        let side = (b.x - a.x) * (q.y - a.y) - (q.x - a.x) * (b.y - a.y);
        if a.y <= q.y {
            if b.y > q.y && side > 0.0 {
                w += 1;
            }
        } else if b.y <= q.y && side < 0.0 {
            w -= 1;
        }
    }
    w
}

fn depth(rings: &[Ring], q: C) -> i32 {
    rings.iter().map(|r| r.sign * winding(&r.pts, q)).sum()
}

fn distance_to_segment(p: C, a: C, b: C) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

/// Orients `edge` so that the region of depth at least 1 is on its right, or `None` when the
/// edge does not separate depth 1 from less.
fn result_edge(rings: &[Ring], edges: &[Vec<C>], at: usize) -> Option<Vec<C>> {
    let edge = &edges[at];
    let (a, b) = (edge[0], edge[1]);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    let mid = Coord {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    };
    // Probe at a quarter of the distance to the nearest other segment: the probe then sees
    // only this edge's side of the linework.
    let mut nearest = len / 2.0;
    for (i, other) in edges.iter().enumerate() {
        for (k, s) in other.windows(2).enumerate() {
            if i == at && k == 0 {
                continue;
            }
            nearest = nearest.min(distance_to_segment(mid, s[0], s[1]));
        }
    }
    let delta = nearest / 4.0;
    let (nx, ny) = (-dy / len, dx / len);
    let left = Coord {
        x: mid.x + nx * delta,
        y: mid.y + ny * delta,
    };
    let right = Coord {
        x: mid.x - nx * delta,
        y: mid.y - ny * delta,
    };
    let (l, r) = (depth(rings, left), depth(rings, right));
    if r >= 1 && l < 1 {
        Some(edge.clone())
    } else if l >= 1 && r < 1 {
        Some(edge.iter().rev().copied().collect())
    } else {
        None
    }
}

/// Counter-clockwise angle in (0, 2 pi] from direction `from` to direction `to`.
fn turn(from: C, to: C) -> f64 {
    let angle = (from.x * to.y - from.y * to.x).atan2(from.x * to.x + from.y * to.y);
    if angle <= 0.0 {
        angle + 2.0 * PI
    } else {
        angle
    }
}

/// Splits a closed walk into simple cycles at repeated vertices (GEOS `MinimalEdgeRing`).
fn split_walk(walk: &[C]) -> Vec<Vec<C>> {
    let mut stack: Vec<C> = Vec::new();
    let mut at: HashMap<(u64, u64), usize> = HashMap::new();
    let mut cycles = Vec::new();
    for &c in walk {
        match at.get(&key(c)).copied() {
            Some(p) => {
                let mut ring: Vec<C> = stack[p..].to_vec();
                ring.push(c);
                for dropped in &stack[p + 1..] {
                    at.remove(&key(*dropped));
                }
                stack.truncate(p + 1);
                if ring.len() >= 4 {
                    cycles.push(ring);
                }
            }
            None => {
                at.insert(key(c), stack.len());
                stack.push(c);
            }
        }
    }
    cycles
}

/// Links directed edges (interior on the right) into rings: at a node the next edge is the
/// first one counter-clockwise from the way back, which keeps to the same interior wedge.
fn link_rings(edges: &[Vec<C>]) -> Vec<Vec<C>> {
    let mut leaving: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        leaving.entry(key(e[0])).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut rings = Vec::new();
    for first in 0..edges.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut walk: Vec<C> = edges[first].clone();
        let mut current = first;
        loop {
            let e = &edges[current];
            let node = e[e.len() - 1];
            let back = Coord {
                x: e[e.len() - 2].x - node.x,
                y: e[e.len() - 2].y - node.y,
            };
            let next = leaving
                .get(&key(node))
                .into_iter()
                .flatten()
                .copied()
                .filter(|&c| !used[c] || c == first)
                .min_by(|&p, &q| {
                    let dir = |c: usize| Coord {
                        x: edges[c][1].x - node.x,
                        y: edges[c][1].y - node.y,
                    };
                    turn(back, dir(p)).total_cmp(&turn(back, dir(q)))
                });
            match next {
                Some(c) if c == first => break,
                Some(c) => {
                    used[c] = true;
                    walk.extend_from_slice(&edges[c][1..]);
                    current = c;
                }
                None => break,
            }
        }
        if walk.first() == walk.last() {
            rings.extend(split_walk(&walk));
        }
    }
    rings
}

/// Number of `buffer_zero` calls, for the fuzz harness to tell which cases needed a repair.
#[cfg(test)]
pub(crate) static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// GEOS `buffer(0)` of a polygon or multipolygon (anything else is returned as it is).
pub(crate) fn buffer_zero(geom: &Geometry<f64>) -> Result<Geometry<f64>, String> {
    #[cfg(test)]
    CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let polygons: Vec<&Polygon<f64>> = match geom {
        Geometry::Polygon(p) => vec![p],
        Geometry::MultiPolygon(m) => m.0.iter().collect(),
        other => return Ok(other.clone()),
    };
    let rings = rings_of(&polygons);
    let lines: Vec<LineString<f64>> = rings
        .iter()
        .map(|r| LineString::new(r.pts.clone()))
        .collect();
    let edges = node_lines_with(&lines, true)?;
    let boundary: Vec<Vec<C>> = (0..edges.len())
        .filter_map(|i| result_edge(&rings, &edges, i))
        .collect();
    let (mut shells, mut holes) = (Vec::new(), Vec::new());
    for ring in link_rings(&boundary) {
        // Interior on the right: a clockwise ring is a shell, a counter-clockwise one a hole.
        let area: f64 = ring
            .windows(2)
            .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
            .sum();
        if area < 0.0 {
            shells.push(ring);
        } else if area > 0.0 {
            holes.push(ring);
        }
    }
    Ok(crate::unary::collapse(MultiPolygon::new(assemble(
        &shells, &holes,
    ))))
}
