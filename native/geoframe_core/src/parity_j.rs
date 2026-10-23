//! Batch I2 (part 1): `snap`, `shared_paths`, `minimum_clearance` and `minimum_clearance_line`.
//!
//! # `snap`
//!
//! The GEOS `GeometrySnapper` as observed through Shapely 2.2 (GEOS 3.14), applied line by
//! line (every line, polygon ring and point of the geometry is snapped on its own):
//!
//! 1. the snap points are the distinct coordinates of the reference geometry, in the order
//!    they occur;
//! 2. *vertex snapping*: for each snap point in turn, the nearest vertex of the line (as it
//!    is at that moment, first one on ties) moves onto the snap point when it is closer than
//!    the tolerance. A later snap point can take over a vertex an earlier one moved; a ring's
//!    closing vertex follows its first;
//! 3. *segment snapping*: each snap point that is closer than the tolerance to a segment (the
//!    nearest one, first on ties) is inserted into that segment, before its start when it lies
//!    behind it, after its end when it lies beyond it. A snap point equal to a vertex of the
//!    line is never inserted.
//!
//! GEOS' own heuristics for the order of several snap points that compete for one vertex are
//! not documented; this port agrees with it on 99 % of random cases (`docs/parity.md`). The
//! work per geometry (vertices times snap points) is capped at 50,000,000 steps; a row over the
//! cap is null.
//!
//! # `shared_paths`
//!
//! The stretches two linear geometries have in common, split at the nodes of the overlay
//! (every vertex where the overlap starts or ends), as a collection of two multi-lines: first
//! those that run the same way in both inputs, then those that run in opposite ways. A stretch
//! that appears several times (a line that doubles back over it) is reported once. Anything
//! that is not a (multi-)line gives a null row, where Shapely raises.
//!
//! # `minimum_clearance`
//!
//! The smallest distance by which a vertex could move to break the geometry: the least
//! distance between two vertices at different positions, or between a vertex and a segment
//! that does not end in it. Found with R-trees in roughly `O(n log n)`. A geometry without such a
//! pair (a point, an empty geometry, one position repeated) reports [`NO_CLEARANCE`] (GEOS'
//! `DBL_MAX`; Shapely says infinity) and an empty line from `minimum_clearance_line`.

use std::collections::HashSet;

use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Coord, CoordsIter, Geometry, GeometryCollection, LineString, MultiLineString};
use rstar::primitives::{GeomWithData, Line as RLine};
use rstar::RTree;

use crate::noding;
use crate::polygonize::node_lines;
use crate::predicates::guarded;
use crate::GeoColumn;

/// What [`GeoColumn::minimum_clearance`] reports when no clearance exists (a point, an empty
/// geometry, only coincident vertices): GEOS' `DBL_MAX`; Shapely turns it into infinity.
pub const NO_CLEARANCE: f64 = f64::MAX;

/// Most (vertex, snap point) pairs `snap` examines for one geometry.
const SNAP_WORK_CAP: u128 = 50_000_000;

fn same(a: Coord<f64>, b: Coord<f64>) -> bool {
    a.x == b.x && a.y == b.y
}

pub(crate) fn dist(a: Coord<f64>, b: Coord<f64>) -> f64 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    (dx * dx + dy * dy).sqrt()
}

/// Projection factor of `p` on the segment `a`-`b` (0 at `a`, 1 at `b`); 0 for a point.
fn projection(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return 0.0;
    }
    ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2
}

/// `Distance::pointToSegment` of JTS/GEOS.
pub(crate) fn point_segment_distance(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    if same(a, b) {
        return dist(p, a);
    }
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let r = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2;
    if r <= 0.0 {
        return dist(p, a);
    }
    if r >= 1.0 {
        return dist(p, b);
    }
    let s = ((a.y - p.y) * dx - (a.x - p.x) * dy) / len2;
    s.abs() * len2.sqrt()
}

// -- snap ---------------------------------------------------------------------------------

/// Snaps one coordinate sequence (see the module documentation).
fn snap_coords(src: &[Coord<f64>], pts: &[Coord<f64>], tol: f64) -> Vec<Coord<f64>> {
    let mut coords = src.to_vec();
    if coords.is_empty() {
        return coords;
    }
    let closed = coords.len() > 1 && same(coords[0], coords[coords.len() - 1]);
    let end = if closed {
        coords.len() - 1
    } else {
        coords.len()
    };
    for &p in pts {
        let mut best: Option<(usize, f64)> = None;
        for (i, &c) in coords.iter().enumerate().take(end) {
            let d = dist(c, p);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        let Some((i, d)) = best else { continue };
        if d.partial_cmp(&tol) != Some(std::cmp::Ordering::Less) || same(coords[i], p) {
            continue;
        }
        coords[i] = p;
        if i == 0 && closed {
            let last = coords.len() - 1;
            coords[last] = p;
        }
    }
    for &p in pts {
        let mut found: Option<(f64, usize)> = None;
        let mut blocked = false;
        for i in 0..coords.len().saturating_sub(1) {
            let (a, b) = (coords[i], coords[i + 1]);
            if same(a, p) || same(b, p) {
                blocked = true;
                break;
            }
            let d = point_segment_distance(p, a, b);
            if d < tol && found.is_none_or(|(fd, _)| d < fd) {
                let t = projection(p, a, b);
                let pos = if t <= 0.0 {
                    i
                } else if t >= 1.0 {
                    i + 2
                } else {
                    i + 1
                };
                found = Some((d, pos));
            }
        }
        if let (false, Some((_, mut pos))) = (blocked, found) {
            if closed {
                pos = pos.clamp(1, coords.len() - 1);
            }
            coords.insert(pos, p);
        }
    }
    coords
}

fn snap_ring(ring: &LineString<f64>, pts: &[Coord<f64>], tol: f64) -> LineString<f64> {
    LineString::new(snap_coords(&ring.0, pts, tol))
}

fn vertex_count(g: &Geometry<f64>) -> usize {
    g.coords_count()
}

fn snap_geometry(g: &Geometry<f64>, pts: &[Coord<f64>], tol: f64) -> Option<Geometry<f64>> {
    use geo::{MultiPoint, MultiPolygon, Point, Polygon};
    let polygon = |p: &Polygon<f64>| {
        Polygon::new(
            snap_ring(p.exterior(), pts, tol),
            p.interiors()
                .iter()
                .map(|r| snap_ring(r, pts, tol))
                .collect(),
        )
    };
    let point = |p: &Point<f64>| Point(snap_coords(&[p.0], pts, tol)[0]);
    Some(match g {
        Geometry::Point(p) => Geometry::Point(point(p)),
        Geometry::MultiPoint(m) => {
            Geometry::MultiPoint(MultiPoint::new(m.0.iter().map(point).collect()))
        }
        Geometry::Line(l) => Geometry::LineString(snap_ring(&LineString::from(*l), pts, tol)),
        Geometry::LineString(l) => Geometry::LineString(snap_ring(l, pts, tol)),
        Geometry::MultiLineString(m) => Geometry::MultiLineString(MultiLineString::new(
            m.0.iter().map(|l| snap_ring(l, pts, tol)).collect(),
        )),
        Geometry::Polygon(p) => Geometry::Polygon(polygon(p)),
        Geometry::MultiPolygon(m) => {
            Geometry::MultiPolygon(MultiPolygon::new(m.0.iter().map(polygon).collect()))
        }
        Geometry::Rect(r) => Geometry::Polygon(polygon(&r.to_polygon())),
        Geometry::Triangle(t) => Geometry::Polygon(polygon(&t.to_polygon())),
        Geometry::GeometryCollection(c) => {
            Geometry::GeometryCollection(GeometryCollection::new_from(
                c.0.iter()
                    .map(|m| snap_geometry(m, pts, tol))
                    .collect::<Option<Vec<_>>>()?,
            ))
        }
    })
}

/// The distinct coordinates of `g`, in order of appearance.
fn snap_points(g: &Geometry<f64>) -> Vec<Coord<f64>> {
    let mut seen = HashSet::new();
    g.coords_iter()
        .filter(|c| seen.insert(noding::key(*c)))
        .collect()
}

fn snap_pair(a: &Geometry<f64>, pts: &[Coord<f64>], tol: f64) -> Option<Geometry<f64>> {
    if (vertex_count(a) as u128) * (pts.len() as u128) > SNAP_WORK_CAP {
        return None;
    }
    snap_geometry(a, pts, tol)
}

impl GeoColumn {
    /// Snaps the vertices and segments of every geometry to the vertices of the matching row of
    /// `other` (a single row broadcasts), within `tolerance`; see the module documentation.
    /// Null rows, rows over the work cap and rows `geo` cannot represent are null.
    pub fn snap(&self, other: &GeoColumn, tolerance: f64) -> Result<Self, String> {
        let rows = self.zip_pairs(
            other,
            |a, b| guarded(|| snap_pair(a, &snap_points(b), tolerance)).flatten(),
            snap_points,
            |pts, row| guarded(|| snap_pair(row, pts, tolerance)).flatten(),
        )?;
        self.rebuild(&rows)
    }
}

// -- shared_paths -------------------------------------------------------------------------

fn lineal_parts(g: &Geometry<f64>) -> Option<Vec<LineString<f64>>> {
    match g {
        Geometry::LineString(l) => Some(vec![l.clone()]),
        Geometry::Line(l) => Some(vec![LineString::from(*l)]),
        Geometry::MultiLineString(m) => Some(m.0.clone()),
        _ => None,
    }
}

fn on_segment(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> bool {
    RobustKernel::orient2d(a, b, p) == Orientation::Collinear
        && p.x >= a.x.min(b.x)
        && p.x <= a.x.max(b.x)
        && p.y >= a.y.min(b.y)
        && p.y <= a.y.max(b.y)
}

/// Whether the stretch `from`-`to` runs the same way as the first segment of `lines` that
/// covers it.
fn runs_with(from: Coord<f64>, to: Coord<f64>, lines: &[LineString<f64>]) -> bool {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    for line in lines {
        for w in line.0.windows(2) {
            if same(w[0], w[1]) {
                continue;
            }
            if on_segment(from, w[0], w[1]) && on_segment(to, w[0], w[1]) {
                return dx * (w[1].x - w[0].x) + dy * (w[1].y - w[0].y) > 0.0;
            }
        }
    }
    true
}

fn shared_pair(a: &Geometry<f64>, b: &Geometry<f64>) -> Option<Geometry<f64>> {
    // Each input is first noded against itself, as the overlay does: a self-crossing splits the
    // stretches that pass through it.
    let la = node_lines(&lineal_parts(a)?).ok()?;
    let lb = node_lines(&lineal_parts(b)?).ok()?;
    let (la, lb): (Vec<LineString<f64>>, Vec<LineString<f64>>) = (
        la.into_iter().map(LineString::new).collect(),
        lb.into_iter().map(LineString::new).collect(),
    );
    let noded = noding::node(&la, &lb);
    let mut seen: HashSet<Vec<(u64, u64)>> = HashSet::new();
    let (mut forward, mut backward) = (Vec::new(), Vec::new());
    for piece in noded.a.iter().filter(|p| p.shared) {
        let key = |coords: &mut dyn Iterator<Item = &Coord<f64>>| -> Vec<(u64, u64)> {
            coords.map(|c| noding::key(*c)).collect()
        };
        let fwd = key(&mut piece.coords.iter());
        let rev = key(&mut piece.coords.iter().rev());
        if !seen.insert(fwd.clone().min(rev)) {
            continue;
        }
        let line = LineString::new(piece.coords.clone());
        if runs_with(piece.coords[0], piece.coords[1], &lb) {
            forward.push(line);
        } else {
            backward.push(line);
        }
    }
    Some(Geometry::GeometryCollection(GeometryCollection::new_from(
        vec![
            Geometry::MultiLineString(MultiLineString::new(forward)),
            Geometry::MultiLineString(MultiLineString::new(backward)),
        ],
    )))
}

impl GeoColumn {
    /// The shared paths of each pair of linear geometries (a single row of `other` broadcasts):
    /// a `GEOMETRYCOLLECTION` of the multi-line running the same way in both and the multi-line
    /// running in opposite ways. Rows that are null or not (multi-)lines are null.
    pub fn shared_paths(&self, other: &GeoColumn) -> Result<Self, String> {
        let rows = self.zip_pairs(
            other,
            |a, b| guarded(|| shared_pair(a, b)).flatten(),
            |scalar| scalar.clone(),
            |scalar, row| guarded(|| shared_pair(row, scalar)).flatten(),
        )?;
        self.rebuild(&rows)
    }
}

// -- minimum_clearance --------------------------------------------------------------------

struct Clearance {
    distance: f64,
    from: Coord<f64>,
    to: Coord<f64>,
}

fn sequences(g: &Geometry<f64>, out: &mut Vec<Vec<Coord<f64>>>) {
    match g {
        Geometry::Point(p) => out.push(vec![p.0]),
        Geometry::MultiPoint(m) => out.extend(m.0.iter().map(|p| vec![p.0])),
        Geometry::Line(l) => out.push(vec![l.start, l.end]),
        Geometry::LineString(l) => out.push(l.0.clone()),
        Geometry::MultiLineString(m) => out.extend(m.0.iter().map(|l| l.0.clone())),
        Geometry::Polygon(p) => {
            out.push(p.exterior().0.clone());
            out.extend(p.interiors().iter().map(|r| r.0.clone()));
        }
        Geometry::MultiPolygon(m) => m.0.iter().for_each(|p| {
            out.push(p.exterior().0.clone());
            out.extend(p.interiors().iter().map(|r| r.0.clone()));
        }),
        Geometry::Rect(r) => sequences(&Geometry::Polygon(r.to_polygon()), out),
        Geometry::Triangle(t) => sequences(&Geometry::Polygon(t.to_polygon()), out),
        Geometry::GeometryCollection(c) => c.0.iter().for_each(|m| sequences(m, out)),
    }
}

/// The closest point of the segment `a`-`b` to `p`.
fn closest_on_segment(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> Coord<f64> {
    let t = projection(p, a, b).clamp(0.0, 1.0);
    Coord {
        x: a.x + t * (b.x - a.x),
        y: a.y + t * (b.y - a.y),
    }
}

fn clearance(g: &Geometry<f64>) -> Option<Clearance> {
    let mut seqs = Vec::new();
    sequences(g, &mut seqs);
    let mut verts: Vec<Coord<f64>> = Vec::new();
    let mut segs: Vec<(Coord<f64>, Coord<f64>)> = Vec::new();
    for seq in &seqs {
        for c in seq {
            if c.x.is_finite() && c.y.is_finite() {
                verts.push(*c);
            }
        }
        for w in seq.windows(2) {
            let ok = |c: Coord<f64>| c.x.is_finite() && c.y.is_finite();
            if ok(w[0]) && ok(w[1]) && !same(w[0], w[1]) {
                segs.push((w[0], w[1]));
            }
        }
    }
    let vtree = RTree::bulk_load(
        verts
            .iter()
            .enumerate()
            .map(|(i, c)| GeomWithData::new([c.x, c.y], i))
            .collect(),
    );
    let stree = RTree::bulk_load(
        segs.iter()
            .enumerate()
            .map(|(i, (a, b))| GeomWithData::new(RLine::new([a.x, a.y], [b.x, b.y]), i))
            .collect(),
    );
    let mut best: Option<Clearance> = None;
    for &q in &verts {
        let limit = best.as_ref().map_or(f64::INFINITY, |b| b.distance);
        let probe = [q.x, q.y];
        // Nearest vertex at another position (smallest index among equally near ones).
        let mut nearest: Option<(f64, usize)> = None;
        for (item, d2) in vtree.nearest_neighbor_iter_with_distance_2(&probe) {
            if nearest.is_some_and(|(nd, _)| d2.sqrt() > nd) || d2.sqrt() >= limit {
                break;
            }
            let v = verts[item.data];
            if same(v, q) {
                continue;
            }
            let d = dist(v, q);
            if nearest.is_none_or(|(nd, ni)| d < nd || (d == nd && item.data < ni)) {
                nearest = Some((d, item.data));
            }
        }
        if let Some((d, i)) = nearest {
            if d < best.as_ref().map_or(f64::INFINITY, |b| b.distance) {
                best = Some(Clearance {
                    distance: d,
                    from: q,
                    to: verts[i],
                });
            }
        }
        let limit = best.as_ref().map_or(f64::INFINITY, |b| b.distance);
        // Nearest segment that does not end in the vertex.
        let mut nearest: Option<(f64, usize)> = None;
        for (item, d2) in stree.nearest_neighbor_iter_with_distance_2(&probe) {
            let bound = nearest.map_or(limit, |(nd, _)| nd.min(limit));
            if d2.sqrt() > bound * (1.0 + 1e-9) + 1e-300
                || (nearest.is_none() && d2.sqrt() >= limit * (1.0 + 1e-9))
            {
                break;
            }
            let (a, b) = segs[item.data];
            if same(a, q) || same(b, q) {
                continue;
            }
            let d = point_segment_distance(q, a, b);
            if nearest.is_none_or(|(nd, ni)| d < nd || (d == nd && item.data < ni)) {
                nearest = Some((d, item.data));
            }
        }
        if let Some((d, i)) = nearest {
            if d < best.as_ref().map_or(f64::INFINITY, |b| b.distance) {
                let (a, b) = segs[i];
                best = Some(Clearance {
                    distance: d,
                    from: q,
                    to: closest_on_segment(q, a, b),
                });
            }
        }
    }
    best
}

impl GeoColumn {
    /// The minimum clearance of each geometry (see the module documentation); [`NO_CLEARANCE`]
    /// when none exists, null for null rows.
    pub fn minimum_clearance(&self) -> Vec<Option<f64>> {
        self.geo_rows()
            .iter()
            .map(|row| {
                row.as_ref().map(|g| {
                    guarded(|| clearance(g).map_or(NO_CLEARANCE, |c| c.distance))
                        .unwrap_or(NO_CLEARANCE)
                })
            })
            .collect()
    }

    /// The line between the two points that realise the minimum clearance of each geometry;
    /// `LINESTRING EMPTY` when none exists, null for null rows. Which of several equally close
    /// pairs is reported, and the direction of the line, can differ from GEOS.
    pub fn minimum_clearance_line(&self) -> Result<Self, String> {
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .map(|row| {
                row.as_ref().map(|g| {
                    let line = guarded(|| clearance(g)).flatten().map_or_else(
                        || LineString::new(vec![]),
                        |c| LineString::new(vec![c.from, c.to]),
                    );
                    Geometry::LineString(line)
                })
            })
            .collect();
        self.rebuild(&rows)
    }
}
