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

use std::cmp::Ordering;
use std::collections::HashMap;

use geo::algorithm::coordinate_position::{CoordPos, CoordinatePosition};
use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Coord, Geometry, LineString, MultiPolygon, Polygon};

use crate::noding::key;
use crate::polygonize::node_lines_with;

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

/// Depth to the left and to the right of `edges[at]` (in its own direction).
fn edge_depths(rings: &[Ring], edges: &[Vec<C>], at: usize) -> (i32, i32) {
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
    (depth(rings, left), depth(rings, right))
}

// -- GEOS BufferBuilder / PolygonBuilder: the result polygons in GEOS's order ---------------

type Key = (u64, u64);

/// GEOS `Orientation::isCCW`: orientation of a ring from its highest point (not its area).
fn is_ccw(ring: &[C]) -> bool {
    if ring.len() < 4 {
        return false;
    }
    let n = ring.len() - 1;
    let mut up_hi = ring[0];
    let mut up_low: Option<C> = None;
    let mut prev_y = up_hi.y;
    let mut i_up_hi = 0;
    for (i, c) in ring.iter().enumerate().take(n + 1).skip(1) {
        if c.y > prev_y && c.y >= up_hi.y {
            i_up_hi = i;
            up_hi = *c;
            up_low = Some(ring[i - 1]);
        }
        prev_y = c.y;
    }
    if i_up_hi == 0 {
        return false;
    }
    let mut i_down_low = i_up_hi;
    loop {
        i_down_low = (i_down_low + 1) % n;
        if !(i_down_low != i_up_hi && ring[i_down_low].y == up_hi.y) {
            break;
        }
    }
    let down_low = ring[i_down_low];
    let down_hi = ring[if i_down_low > 0 {
        i_down_low - 1
    } else {
        n - 1
    }];
    if up_hi == down_hi {
        let up_low = up_low.unwrap_or(up_hi);
        if up_low == up_hi || down_low == up_hi || up_low == down_low {
            return false;
        }
        RobustKernel::orient2d(up_low, up_hi, down_low) == Orientation::CounterClockwise
    } else {
        down_hi.x - up_hi.x < 0.0
    }
}

/// GEOS `Quadrant::quadrant`.
fn quadrant(dx: f64, dy: f64) -> u8 {
    match (dx >= 0.0, dy >= 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    }
}

/// The planar graph of the noded edges: a directed edge `2 * e` runs along edge `e`, `2 * e + 1`
/// against it (its `sym` is `de ^ 1`); every node keeps its directed edges counter-clockwise
/// (GEOS `DirectedEdgeStar`).
struct Graph<'a> {
    edges: &'a [Vec<C>],
    node_of: Vec<usize>,
    star: Vec<Vec<usize>>,
    in_result: Vec<bool>,
}

impl Graph<'_> {
    fn origin(&self, de: usize) -> C {
        let e = &self.edges[de >> 1];
        if de & 1 == 0 {
            e[0]
        } else {
            e[e.len() - 1]
        }
    }

    fn toward(&self, de: usize) -> C {
        let e = &self.edges[de >> 1];
        if de & 1 == 0 {
            e[1]
        } else {
            e[e.len() - 2]
        }
    }

    /// The directed edges leaving a node of which this or the reverse is in the result.
    fn area_edges(&self, node: usize) -> Vec<usize> {
        self.star[node]
            .iter()
            .copied()
            .filter(|&d| self.in_result[d] || self.in_result[d ^ 1])
            .collect()
    }
}

fn graph_of<'a>(edges: &'a [Vec<C>], depths: &[(i32, i32)]) -> Graph<'a> {
    let mut coords: Vec<C> = edges
        .iter()
        .flat_map(|e| [e[0], e[e.len() - 1]])
        .map(|c| Coord {
            x: c.x + 0.0,
            y: c.y + 0.0,
        })
        .collect();
    // GEOS `NodeMap`: nodes ordered by x, then y.
    coords.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let mut index: HashMap<Key, usize> = HashMap::new();
    let mut count = 0;
    for c in &coords {
        index.entry(key(*c)).or_insert_with(|| {
            count += 1;
            count - 1
        });
    }
    let mut graph = Graph {
        edges,
        node_of: Vec::with_capacity(edges.len() * 2),
        star: vec![Vec::new(); count],
        in_result: Vec::with_capacity(edges.len() * 2),
    };
    for (e, &(l, r)) in depths.iter().enumerate() {
        for de in [2 * e, 2 * e + 1] {
            let node = index[&key(graph.origin(de))];
            graph.node_of.push(node);
            graph.star[node].push(de);
        }
        graph.in_result.push(r >= 1 && l <= 0);
        graph.in_result.push(l >= 1 && r <= 0);
    }
    for node in 0..count {
        let mut star = std::mem::take(&mut graph.star[node]);
        star.sort_by(|&a, &b| {
            let o = graph.origin(a);
            let (pa, pb) = (graph.toward(a), graph.toward(b));
            let (adx, ady, bdx, bdy) = (pa.x - o.x, pa.y - o.y, pb.x - o.x, pb.y - o.y);
            if adx == bdx && ady == bdy {
                return Ordering::Equal;
            }
            quadrant(adx, ady).cmp(&quadrant(bdx, bdy)).then_with(|| {
                match RobustKernel::orient2d(o, pb, pa) {
                    Orientation::CounterClockwise => Ordering::Greater,
                    Orientation::Clockwise => Ordering::Less,
                    Orientation::Collinear => Ordering::Equal,
                }
            })
        });
        graph.star[node] = star;
    }
    graph
}

/// GEOS `BufferSubgraph`: a connected part of the graph.
struct Subgraph {
    nodes: Vec<usize>,
    dir_edges: Vec<usize>,
    rightmost: f64,
}

/// `BufferBuilder::createSubgraphs`: depth-first from each unvisited node in node order, then
/// sorted by descending rightmost x (a stable sort; `std::sort` is only stable for up to 16).
fn subgraphs_of(graph: &Graph) -> Vec<Subgraph> {
    let mut visited = vec![false; graph.star.len()];
    let mut out = Vec::new();
    for start in 0..graph.star.len() {
        if visited[start] {
            continue;
        }
        let mut sub = Subgraph {
            nodes: Vec::new(),
            dir_edges: Vec::new(),
            rightmost: f64::NEG_INFINITY,
        };
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            visited[node] = true;
            sub.nodes.push(node);
            for &de in &graph.star[node] {
                sub.dir_edges.push(de);
                for c in &graph.edges[de >> 1] {
                    sub.rightmost = sub.rightmost.max(c.x);
                }
                let sym_node = graph.node_of[de ^ 1];
                if !visited[sym_node] {
                    stack.push(sym_node);
                }
            }
        }
        out.push(sub);
    }
    out.sort_by(|a, b| b.rightmost.total_cmp(&a.rightmost));
    out
}

/// GEOS `EdgeRing` (maximal or minimal).
struct EdgeRing {
    edges: Vec<usize>,
    pts: Vec<C>,
    hole: bool,
    shell: Option<usize>,
    holes: Vec<usize>,
}

#[derive(Default)]
struct PolygonBuilder {
    rings: Vec<EdgeRing>,
    shells: Vec<usize>,
    next: HashMap<usize, usize>,
    next_min: HashMap<usize, usize>,
    ring_of: HashMap<usize, usize>,
    min_ring_of: HashMap<usize, usize>,
}

fn add_points(pts: &mut Vec<C>, edge: &[C], forward: bool, first: bool) {
    if forward {
        pts.extend_from_slice(if first { edge } else { &edge[1..] });
    } else {
        let start = if first { edge.len() } else { edge.len() - 1 };
        for i in (1..=start).rev() {
            pts.push(edge[i - 1]);
        }
    }
}

impl PolygonBuilder {
    /// `EdgeRing::computePoints` + `computeRing`: follows `next` (or `next_min`) from `start`.
    fn walk(&mut self, graph: &Graph, start: usize, minimal: bool) -> Result<usize, String> {
        let id = self.rings.len();
        let (mut edges, mut pts) = (Vec::new(), Vec::new());
        let mut de = start;
        loop {
            let marks = if minimal {
                &mut self.min_ring_of
            } else {
                &mut self.ring_of
            };
            if marks.get(&de) == Some(&id) {
                return Err("buffer: directed edge visited twice".to_string());
            }
            marks.insert(de, id);
            edges.push(de);
            add_points(
                &mut pts,
                &graph.edges[de >> 1],
                de & 1 == 0,
                edges.len() == 1,
            );
            let links = if minimal { &self.next_min } else { &self.next };
            de = *links
                .get(&de)
                .ok_or_else(|| "buffer: unlinked directed edge".to_string())?;
            if de == start {
                break;
            }
        }
        if pts.len() < 4 {
            return Err("buffer: invalid ring".to_string());
        }
        let hole = is_ccw(&pts);
        self.rings.push(EdgeRing {
            edges,
            pts,
            hole,
            shell: None,
            holes: Vec::new(),
        });
        Ok(id)
    }

    /// `DirectedEdgeStar::linkResultDirectedEdges` at every node of the subgraph.
    fn link_results(&mut self, graph: &Graph, sub: &Subgraph) -> Result<(), String> {
        for &node in &sub.nodes {
            let (mut first_out, mut incoming, mut linking) = (None, 0, false);
            for out in graph.area_edges(node) {
                let inn = out ^ 1;
                if first_out.is_none() && graph.in_result[out] {
                    first_out = Some(out);
                }
                if !linking {
                    if !graph.in_result[inn] {
                        continue;
                    }
                    incoming = inn;
                    linking = true;
                } else {
                    if !graph.in_result[out] {
                        continue;
                    }
                    self.next.insert(incoming, out);
                    linking = false;
                }
            }
            if linking {
                let first = first_out.ok_or("buffer: no outgoing directed edge")?;
                self.next.insert(incoming, first);
            }
        }
        Ok(())
    }

    /// `DirectedEdgeStar::linkMinimalDirectedEdges` for the maximal ring `er`.
    fn link_minimal(&mut self, graph: &Graph, er: usize, node: usize) {
        let in_ring = |this: &Self, de: usize| this.ring_of.get(&de) == Some(&er);
        let (mut first_out, mut incoming, mut linking) = (None, 0, false);
        for out in graph.area_edges(node).into_iter().rev() {
            let inn = out ^ 1;
            if first_out.is_none() && in_ring(self, out) {
                first_out = Some(out);
            }
            if !linking {
                if !in_ring(self, inn) {
                    continue;
                }
                incoming = inn;
                linking = true;
            } else {
                if !in_ring(self, out) {
                    continue;
                }
                self.next_min.insert(incoming, out);
                linking = false;
            }
        }
        if linking {
            if let Some(first) = first_out {
                self.next_min.insert(incoming, first);
            }
        }
    }

    fn set_shell(&mut self, hole: usize, shell: usize) {
        self.rings[hole].shell = Some(shell);
        self.rings[shell].holes.push(hole);
    }

    /// `PolygonBuilder::findEdgeRingContaining`.
    fn containing_shell(&self, test: usize) -> Option<usize> {
        let envelope = |pts: &[C]| {
            pts.iter().fold(
                (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ),
                |e, c| (e.0.min(c.x), e.1.min(c.y), e.2.max(c.x), e.3.max(c.y)),
            )
        };
        let contains = |a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)| {
            a.0 <= b.0 && a.1 <= b.1 && a.2 >= b.2 && a.3 >= b.3
        };
        let test_pts = &self.rings[test].pts;
        let test_env = envelope(test_pts);
        let mut best: Option<(usize, (f64, f64, f64, f64))> = None;
        for &shell in &self.shells {
            let shell_pts = &self.rings[shell].pts;
            let env = envelope(shell_pts);
            if env == test_env || !contains(env, test_env) {
                continue;
            }
            let probe = test_pts
                .iter()
                .find(|p| !shell_pts.contains(p))
                .copied()
                .unwrap_or(test_pts[0]);
            let polygon = Polygon::new(LineString::new(shell_pts.clone()), vec![]);
            if polygon.coordinate_position(&probe) == CoordPos::Outside {
                continue;
            }
            if best.is_none_or(|(_, best_env)| contains(best_env, env)) {
                best = Some((shell, env));
            }
        }
        best.map(|(shell, _)| shell)
    }

    /// `PolygonBuilder::add` for one subgraph.
    fn add(&mut self, graph: &Graph, sub: &Subgraph) -> Result<(), String> {
        self.link_results(graph, sub)?;
        let mut max_rings = Vec::new();
        for &de in &sub.dir_edges {
            if graph.in_result[de] && !self.ring_of.contains_key(&de) {
                max_rings.push(self.walk(graph, de, false)?);
            }
        }
        let (mut free_holes, mut edge_rings) = (Vec::new(), Vec::new());
        for er in max_rings {
            let degree = self.rings[er]
                .edges
                .iter()
                .map(|&de| {
                    graph.star[graph.node_of[de]]
                        .iter()
                        .filter(|&&d| self.ring_of.get(&d) == Some(&er))
                        .count()
                })
                .max()
                .unwrap_or(0)
                * 2;
            if degree <= 2 {
                edge_rings.push(er);
                continue;
            }
            let ring_edges = self.rings[er].edges.clone();
            for &de in &ring_edges {
                self.link_minimal(graph, er, graph.node_of[de]);
            }
            let mut minimal = Vec::new();
            for &de in &ring_edges {
                if !self.min_ring_of.contains_key(&de) {
                    minimal.push(self.walk(graph, de, true)?);
                }
            }
            let shells: Vec<usize> = minimal
                .iter()
                .copied()
                .filter(|&m| !self.rings[m].hole)
                .collect();
            match shells.as_slice() {
                [] => free_holes.extend(minimal),
                [shell] => {
                    for &m in &minimal {
                        if self.rings[m].hole {
                            self.set_shell(m, *shell);
                        }
                    }
                    self.shells.push(*shell);
                }
                _ => return Err("buffer: found two shells in MinimalEdgeRing list".to_string()),
            }
        }
        for er in edge_rings {
            if self.rings[er].hole {
                free_holes.push(er);
            } else {
                self.shells.push(er);
            }
        }
        for hole in free_holes {
            if self.rings[hole].shell.is_none() {
                if let Some(shell) = self.containing_shell(hole) {
                    self.set_shell(hole, shell);
                }
            }
        }
        Ok(())
    }

    fn polygons(&self) -> Vec<Polygon<f64>> {
        self.shells
            .iter()
            .map(|&s| {
                let ring = |r: usize| LineString::new(self.rings[r].pts.clone());
                Polygon::new(
                    ring(s),
                    self.rings[s].holes.iter().map(|&h| ring(h)).collect(),
                )
            })
            .collect()
    }
}

/// The polygons `BufferBuilder` builds from the noded `edges` with the depth to the left and
/// right of each, in GEOS's order (parts, ring starts, hole order).
fn build_polygons(edges: &[Vec<C>], depths: &[(i32, i32)]) -> Result<Vec<Polygon<f64>>, String> {
    let graph = graph_of(edges, depths);
    let mut builder = PolygonBuilder::default();
    for sub in subgraphs_of(&graph) {
        builder.add(&graph, &sub)?;
    }
    Ok(builder.polygons())
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
    let depths: Vec<(i32, i32)> = (0..edges.len())
        .map(|i| edge_depths(&rings, &edges, i))
        .collect();
    let polygons = build_polygons(&edges, &depths)?;
    Ok(crate::unary::collapse(MultiPolygon::new(polygons)))
}
