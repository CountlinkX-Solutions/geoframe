//! Batch I3 (part 1): polygonal coverage validation (`is_valid_coverage`,
//! `invalid_coverage_edges`).
//!
//! # Validation
//!
//! The rules of the GEOS `CoverageValidator` (JTS `CoveragePolygonValidator`) as observed
//! through Shapely 2.2 (GEOS 3.14). Every polygon of the series is checked against the others
//! on its own, ring segment by ring segment:
//!
//! 1. a segment that equals a segment of a neighbour (in either direction) is *matched*, and
//!    valid when the two polygons lie on opposite sides of it; two polygons on the same side
//!    overlap, so both segments are invalid;
//! 2. any other segment is invalid when it crosses a neighbour's segment, touches one with an
//!    endpoint in the interior of either, overlaps it collinearly, or meets it at a shared
//!    vertex while that segment runs into the polygon's interior;
//! 3. with a gap width, a segment is also invalid when it runs nearly parallel to a
//!    neighbour's segment closer than the width (a narrow gap);
//! 4. a segment that touches nothing is invalid when its start vertex lies inside a neighbour.
//!
//! The invalid segments of each ring are joined into lines, in the order GEOS reports them.
//! Rows that are not polygons are ignored; null rows stay null.
//!
//! # Work cap
//!
//! The candidate segment pairs and point-in-polygon steps of one call are capped at
//! 50,000,000; a call over the cap is an error.

use std::collections::HashMap;

use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::algorithm::line_intersection::{line_intersection, LineIntersection};
use geo::coordinate_position::CoordPos;
use geo::{Coord, CoordinatePosition, Geometry, Line, LineString, MultiLineString, Polygon};
use rstar::{RTree, RTreeObject, AABB};

use crate::noding::key;
use crate::predicates::guarded;
use crate::GeoColumn;

/// Most candidate segment pairs plus point-in-polygon steps one call examines.
pub(crate) const WORK_CAP: u64 = 50_000_000;

type C = Coord<f64>;

/// A ring with consecutive repeated points removed and its interior side.
pub(crate) struct Ring {
    pub pts: Vec<C>,
    /// The polygon interior lies to the right of the ring direction.
    pub interior_right: bool,
}

/// The rings of a polygon (shell first), skipping rings too short to enclose anything.
pub(crate) fn rings_of(polygon: &Polygon<f64>) -> Vec<Ring> {
    let mut out = Vec::new();
    let mut push = |ring: &LineString<f64>, shell: bool| {
        let mut pts: Vec<C> = Vec::with_capacity(ring.0.len());
        for &c in &ring.0 {
            if pts.last() != Some(&c) {
                pts.push(c);
            }
        }
        if pts.len() >= 2 && pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        if pts.len() < 4 {
            return;
        }
        let ccw = signed_area(&pts) > 0.0;
        out.push(Ring {
            pts,
            interior_right: if shell { !ccw } else { ccw },
        });
    };
    push(polygon.exterior(), true);
    for hole in polygon.interiors() {
        push(hole, false);
    }
    out
}

fn signed_area(pts: &[C]) -> f64 {
    pts.windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum::<f64>()
        / 2.0
}

/// The polygons of a polygonal geometry; `None` for anything else.
pub(crate) fn polygons_of(g: &Geometry<f64>) -> Option<Vec<&Polygon<f64>>> {
    match g {
        Geometry::Polygon(p) => Some(vec![p]),
        Geometry::MultiPolygon(m) => Some(m.0.iter().collect()),
        _ => None,
    }
}

pub(crate) fn finite(value: f64, what: &str) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{what} must be a finite number"))
    }
}

// -- validation ---------------------------------------------------------------------------

struct Item {
    polygons: Vec<Polygon<f64>>,
    rings: Vec<Ring>,
    env: AABB<[f64; 2]>,
}

#[derive(Clone, Copy)]
struct SegEntry {
    item: u32,
    ring: u32,
    idx: u32,
    a: [f64; 2],
    b: [f64; 2],
    right: bool,
}

impl RTreeObject for SegEntry {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.a, self.b)
    }
}

struct PolyEntry {
    item: u32,
    poly: u32,
    env: AABB<[f64; 2]>,
}

impl RTreeObject for PolyEntry {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.env
    }
}

fn xy(c: C) -> [f64; 2] {
    [c.x, c.y]
}

fn expand(env: &AABB<[f64; 2]>, by: f64) -> AABB<[f64; 2]> {
    let (lo, hi) = (env.lower(), env.upper());
    AABB::from_corners([lo[0] - by, lo[1] - by], [hi[0] + by, hi[1] + by])
}

/// Exact key of a segment, independent of its direction.
type SegKey = ((u64, u64), (u64, u64));

/// The key of a segment, plus whether the segment was flipped to make it.
fn seg_key(p0: C, p1: C) -> (SegKey, bool) {
    let flipped = (p0.x, p0.y) > (p1.x, p1.y);
    if flipped {
        ((key(p1), key(p0)), true)
    } else {
        ((key(p0), key(p1)), false)
    }
}

fn orient(a: C, b: C, c: C) -> Orientation {
    RobustKernel::orient2d(a, b, c)
}

/// `d` lies strictly inside the wedge swept counterclockwise from the ray `v`-`from` to the
/// ray `v`-`to`.
fn in_ccw_wedge(v: C, from: C, to: C, d: C) -> bool {
    match orient(v, from, to) {
        Orientation::CounterClockwise => {
            orient(v, from, d) == Orientation::CounterClockwise
                && orient(v, d, to) == Orientation::CounterClockwise
        }
        Orientation::Clockwise => {
            // The wedge is more than a half plane: everything but the closed complement.
            !(orient(v, to, d) != Orientation::Clockwise
                && orient(v, d, from) != Orientation::Clockwise)
        }
        Orientation::Collinear => {
            let dot = (from.x - v.x) * (to.x - v.x) + (from.y - v.y) * (to.y - v.y);
            // Opposite rays make a half plane; the same ray is a spike without interior.
            dot < 0.0 && orient(v, from, d) == Orientation::CounterClockwise
        }
    }
}

/// `LineSegment.project`: the part of the line `a`-`b` that `seg` projects onto, `None` when
/// `seg` lies wholly beyond one end.
fn project(a: C, b: C, s0: C, s1: C) -> Option<(C, C)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let factor = |p: C| ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2;
    let on = |f: f64| {
        if f < 0.0 {
            a
        } else if f > 1.0 {
            b
        } else {
            Coord {
                x: a.x + f * dx,
                y: a.y + f * dy,
            }
        }
    };
    let (f0, f1) = (factor(s0), factor(s1));
    if (f0 >= 1.0 && f1 >= 1.0) || (f0 <= 0.0 && f1 <= 0.0) {
        return None;
    }
    Some((on(f0), on(f1)))
}

fn dist(a: C, b: C) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Two segments that run alongside each other for more than `tol`, closer than `tol`: the
/// parts of each that the other projects onto both exceed `tol` in length, and their ends lie
/// within `tol` of each other.
fn nearly_parallel(t0: C, t1: C, a0: C, a1: C, tol: f64) -> bool {
    let Some(on_target) = project(t0, t1, a0, a1) else {
        return false;
    };
    let Some(on_adjacent) = project(a0, a1, t0, t1) else {
        return false;
    };
    if dist(on_target.0, on_target.1) <= tol || dist(on_adjacent.0, on_adjacent.1) <= tol {
        return false;
    }
    let same = dist(on_target.0, on_adjacent.0).max(dist(on_target.1, on_adjacent.1));
    let opposite = dist(on_target.0, on_adjacent.1).max(dist(on_target.1, on_adjacent.0));
    same.min(opposite) <= tol
}

struct Validator {
    items: Vec<Option<Item>>,
    segments: RTree<SegEntry>,
    polygons: RTree<PolyEntry>,
    gap: f64,
    work: u64,
    cap: u64,
}

impl Validator {
    fn new(rows: &[Option<Geometry<f64>>], gap: f64, cap: u64) -> Self {
        let mut items: Vec<Option<Item>> = Vec::with_capacity(rows.len());
        let mut segments = Vec::new();
        let mut polygons = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let Some(polys) = row.as_ref().and_then(polygons_of) else {
                items.push(None);
                continue;
            };
            let mut rings = Vec::new();
            let mut owned = Vec::new();
            for polygon in polys {
                let before = rings.len();
                rings.extend(rings_of(polygon));
                if rings.len() > before {
                    owned.push(polygon.clone());
                }
            }
            if rings.is_empty() {
                items.push(None);
                continue;
            }
            let mut lo = [f64::INFINITY; 2];
            let mut hi = [f64::NEG_INFINITY; 2];
            for ring in &rings {
                for c in &ring.pts {
                    lo = [lo[0].min(c.x), lo[1].min(c.y)];
                    hi = [hi[0].max(c.x), hi[1].max(c.y)];
                }
            }
            for (r, ring) in rings.iter().enumerate() {
                for (k, w) in ring.pts.windows(2).enumerate() {
                    segments.push(SegEntry {
                        item: i as u32,
                        ring: r as u32,
                        idx: k as u32,
                        a: xy(w[0]),
                        b: xy(w[1]),
                        right: ring.interior_right,
                    });
                }
            }
            for (p, polygon) in owned.iter().enumerate() {
                if let Some(rect) = geo::BoundingRect::bounding_rect(polygon) {
                    polygons.push(PolyEntry {
                        item: i as u32,
                        poly: p as u32,
                        env: AABB::from_corners(xy(rect.min()), xy(rect.max())),
                    });
                }
            }
            items.push(Some(Item {
                polygons: owned,
                rings,
                env: AABB::from_corners(lo, hi),
            }));
        }
        Self {
            items,
            segments: RTree::bulk_load(segments),
            polygons: RTree::bulk_load(polygons),
            gap,
            work: 0,
            cap,
        }
    }

    fn spend(&mut self, amount: u64) -> Result<(), String> {
        self.work += amount;
        if self.work > self.cap {
            Err(format!(
                "the coverage check needs more than {} segment pairs",
                self.cap
            ))
        } else {
            Ok(())
        }
    }

    /// Per ring of item `t`, the invalid flag of each segment.
    fn validate(&mut self, t: usize) -> Result<Vec<Vec<bool>>, String> {
        let item = self.items[t].as_ref().expect("a polygonal item");
        let gap = self.gap;
        let tenv = expand(&item.env, gap);
        let mut invalid: Vec<Vec<bool>> = item
            .rings
            .iter()
            .map(|r| vec![false; r.pts.len() - 1])
            .collect();
        let mut matched = invalid.clone();

        // 1. matched segments
        let mut map: HashMap<_, (usize, usize, bool)> = HashMap::new();
        for (r, ring) in item.rings.iter().enumerate() {
            for (k, w) in ring.pts.windows(2).enumerate() {
                let (key, flipped) = seg_key(w[0], w[1]);
                let side = ring.interior_right ^ flipped;
                match map.get(&key) {
                    Some(&(r2, k2, side2)) => {
                        let flags = if side == side2 {
                            &mut invalid
                        } else {
                            &mut matched
                        };
                        flags[r][k] = true;
                        flags[r2][k2] = true;
                    }
                    None => {
                        map.insert(key, (r, k, side));
                    }
                }
            }
        }
        let near: Vec<SegEntry> = self
            .segments
            .locate_in_envelope_intersecting(&tenv)
            .filter(|e| e.item as usize != t)
            .copied()
            .collect();
        self.spend(near.len() as u64)?;
        for e in &near {
            let (a, b) = (
                Coord {
                    x: e.a[0],
                    y: e.a[1],
                },
                Coord {
                    x: e.b[0],
                    y: e.b[1],
                },
            );
            let (key, flipped) = seg_key(a, b);
            if let Some(&(r, k, side)) = map.get(&key) {
                if side == (e.right ^ flipped) {
                    invalid[r][k] = true;
                } else {
                    matched[r][k] = true;
                }
            }
        }
        let known = |invalid: &Vec<Vec<bool>>, matched: &Vec<Vec<bool>>, r: usize, k: usize| {
            invalid[r][k] || matched[r][k]
        };

        // 2. segments that interact with a neighbour's
        let item = self.items[t].as_ref().expect("a polygonal item");
        let mut pairs = 0u64;
        let mut marks: Vec<(usize, usize)> = Vec::new();
        for (r, ring) in item.rings.iter().enumerate() {
            for (k, w) in ring.pts.windows(2).enumerate() {
                if known(&invalid, &matched, r, k) {
                    continue;
                }
                let env = expand(&AABB::from_corners(xy(w[0]), xy(w[1])), gap);
                for e in self.segments.locate_in_envelope_intersecting(&env) {
                    if e.item as usize == t {
                        continue;
                    }
                    pairs += 1;
                    let (a0, a1) = (
                        Coord {
                            x: e.a[0],
                            y: e.a[1],
                        },
                        Coord {
                            x: e.b[0],
                            y: e.b[1],
                        },
                    );
                    if seg_key(w[0], w[1]).0 == seg_key(a0, a1).0 {
                        continue;
                    }
                    let adj = &self.items[e.item as usize]
                        .as_ref()
                        .expect("a polygonal item")
                        .rings[e.ring as usize];
                    if is_invalid(adj, e.idx as usize, w[0], w[1], a0, a1, gap) {
                        marks.push((r, k));
                        break;
                    }
                }
            }
        }
        self.spend(pairs)?;
        for (r, k) in marks {
            invalid[r][k] = true;
        }

        // 3. segments inside a neighbour
        let item = self.items[t].as_ref().expect("a polygonal item");
        let mut steps = 0u64;
        let mut inside: Vec<(usize, usize)> = Vec::new();
        for (r, ring) in item.rings.iter().enumerate() {
            for (k, w) in ring.pts.windows(2).enumerate() {
                if known(&invalid, &matched, r, k) {
                    continue;
                }
                let mid = w[0];
                for entry in self
                    .polygons
                    .locate_in_envelope_intersecting(&AABB::from_point(xy(mid)))
                {
                    if entry.item as usize == t {
                        continue;
                    }
                    let other = self.items[entry.item as usize]
                        .as_ref()
                        .expect("a polygonal item");
                    let polygon = &other.polygons[entry.poly as usize];
                    steps += polygon.exterior().0.len() as u64
                        + polygon
                            .interiors()
                            .iter()
                            .map(|h| h.0.len() as u64)
                            .sum::<u64>();
                    if polygon.coordinate_position(&mid) == CoordPos::Inside {
                        inside.push((r, k));
                        break;
                    }
                }
            }
        }
        self.spend(steps)?;
        for (r, k) in inside {
            invalid[r][k] = true;
        }
        Ok(invalid)
    }

    fn lines(&mut self, t: usize) -> Result<Vec<Vec<C>>, String> {
        let invalid = self.validate(t)?;
        let item = self.items[t].as_ref().expect("a polygonal item");
        let mut lines = Vec::new();
        for (ring, flags) in item.rings.iter().zip(&invalid) {
            invalid_lines(&ring.pts, flags, &mut lines);
        }
        Ok(lines)
    }
}

/// `InvalidSegmentDetector.isInvalid` for one target segment `t0`-`t1` against segment `index`
/// (`a0`-`a1`) of the neighbouring ring `adj`.
fn is_invalid(adj: &Ring, index: usize, t0: C, t1: C, a0: C, a1: C, gap: f64) -> bool {
    if collinear_or_interior(adj, index, t0, t1, a0, a1) {
        return true;
    }
    gap > 0.0 && nearly_parallel(t0, t1, a0, a1, gap)
}

fn collinear_or_interior(adj: &Ring, index: usize, t0: C, t1: C, a0: C, a1: C) -> bool {
    match line_intersection(Line::new(t0, t1), Line::new(a0, a1)) {
        None => false,
        Some(LineIntersection::Collinear { .. }) => true,
        Some(LineIntersection::SinglePoint {
            intersection,
            is_proper,
        }) => {
            if is_proper {
                return true;
            }
            let at_end = |p: C, q0: C, q1: C| p == q0 || p == q1;
            if !at_end(intersection, t0, t1) || !at_end(intersection, a0, a1) {
                return true;
            }
            // The segments meet at a vertex of both: invalid when this segment runs into the
            // interior of the neighbouring polygon there.
            let m = adj.pts.len() - 1;
            let vertex = if intersection == a0 {
                index
            } else {
                (index + 1) % m
            };
            let prev = adj.pts[(vertex + m - 1) % m];
            let next = adj.pts[(vertex + 1) % m];
            let end = if t0 == intersection { t1 } else { t0 };
            if adj.interior_right {
                in_ccw_wedge(intersection, prev, next, end)
            } else {
                in_ccw_wedge(intersection, next, prev, end)
            }
        }
    }
}

/// Joins the invalid segments of a ring into lines, in the order GEOS reports them.
fn invalid_lines(pts: &[C], invalid: &[bool], out: &mut Vec<Vec<C>>) {
    let n = invalid.len();
    if !invalid.iter().any(|&b| b) {
        return;
    }
    if invalid.iter().all(|&b| b) {
        out.push(pts.to_vec());
        return;
    }
    let next = |i: usize| if i + 1 >= n { 0 } else { i + 1 };
    let start_from = |mut i: usize| {
        while !invalid[i] {
            i = next(i);
        }
        i
    };
    let end_from = |mut i: usize| {
        i = next(i);
        while invalid[i] {
            i = next(i);
        }
        i
    };
    let first_start = start_from(0);
    let first_end = end_from(first_start);
    let mut end = first_end;
    loop {
        let start = start_from(end);
        end = end_from(start);
        let mut line = vec![pts[start]];
        let mut i = start;
        loop {
            i = next(i);
            line.push(pts[i]);
            if i == end {
                break;
            }
        }
        out.push(line);
        if end == first_end {
            break;
        }
    }
}

/// A row with a NaN or infinite coordinate: it cannot take part in a coverage.
fn has_non_finite(g: &Geometry<f64>) -> bool {
    geo::CoordsIter::coords_iter(g).any(|c| !c.x.is_finite() || !c.y.is_finite())
}

/// What the validator made of one row.
enum Outcome {
    /// A null row, or one with non-finite coordinates.
    Absent,
    /// A row that is not a polygon: it takes no part.
    Ignored,
    /// The invalid lines of a polygonal row.
    Lines(Vec<Vec<C>>),
}

impl GeoColumn {
    fn coverage_outcomes(&self, gap_width: f64, cap: u64) -> Result<Vec<Outcome>, String> {
        let gap = finite(gap_width, "gap_width")?.max(0.0);
        let stored = self.map_scalars(|_| ());
        let geo = self.geo_rows();
        let absent: Vec<bool> = stored
            .iter()
            .zip(&geo)
            .map(|(s, g)| s.is_none() || g.as_ref().is_some_and(has_non_finite))
            .collect();
        let rows: Vec<Option<Geometry<f64>>> = geo
            .into_iter()
            .zip(&absent)
            .map(|(g, &gone)| g.filter(|_| !gone))
            .collect();
        guarded(|| {
            let mut validator = Validator::new(&rows, gap, cap);
            let mut out = Vec::with_capacity(rows.len());
            for (t, &gone) in absent.iter().enumerate() {
                out.push(if gone {
                    Outcome::Absent
                } else if validator.items[t].is_none() {
                    Outcome::Ignored
                } else {
                    Outcome::Lines(validator.lines(t)?)
                });
            }
            Ok(out)
        })
        .unwrap_or_else(|| Err("the coverage check failed on degenerate input".to_string()))
    }

    /// [`is_valid_coverage`](Self::is_valid_coverage) with an explicit work cap.
    pub(crate) fn is_valid_coverage_capped(
        &self,
        gap_width: f64,
        cap: u64,
    ) -> Result<bool, String> {
        Ok(self
            .coverage_outcomes(gap_width, cap)?
            .iter()
            .all(|o| !matches!(o, Outcome::Lines(lines) if !lines.is_empty())))
    }

    /// Whether the polygons of the column form a valid coverage: no two overlap and the
    /// edges they share match vertex for vertex. A `gap_width` over 0 also rejects gaps
    /// narrower than it. Rows that are not polygons are ignored.
    pub fn is_valid_coverage(&self, gap_width: f64) -> Result<bool, String> {
        self.is_valid_coverage_capped(gap_width, WORK_CAP)
    }

    /// Per row, the edges of its polygon(s) that make the coverage invalid: a `LINESTRING`,
    /// a `MULTILINESTRING` of several, or `LINESTRING EMPTY` when there are none (also for
    /// rows that are not polygons). Null rows, and rows with non-finite coordinates, are null.
    pub fn invalid_coverage_edges(&self, gap_width: f64) -> Result<GeoColumn, String> {
        let rows: Vec<Option<Geometry<f64>>> = self
            .coverage_outcomes(gap_width, WORK_CAP)?
            .into_iter()
            .map(|outcome| {
                let mut parts: Vec<LineString<f64>> = match outcome {
                    Outcome::Absent => return None,
                    Outcome::Ignored => Vec::new(),
                    Outcome::Lines(lines) => lines.into_iter().map(LineString::new).collect(),
                };
                Some(match parts.len() {
                    0 => Geometry::LineString(LineString::new(vec![])),
                    1 => Geometry::LineString(parts.remove(0)),
                    _ => Geometry::MultiLineString(MultiLineString::new(parts)),
                })
            })
            .collect();
        self.rebuild(&rows)
    }
}
