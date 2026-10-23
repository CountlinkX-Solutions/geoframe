//! Dimension-aware overlays that `BooleanOps` cannot express: line x line, the lower-
//! dimensional parts of polygon intersections, and unions that mix dimensions.
//!
//! Results are described by [`Parts`] (polygons, lines and points) and become one geometry:
//! a single or multi type when only one dimension is present, a `GeometryCollection` with one
//! member per part otherwise, as GEOS does. A lower-dimensional part that lies inside a
//! higher-dimensional one (closed sets) is absorbed, not reported.

use std::collections::HashSet;

use geo::coordinate_position::{CoordPos, CoordinatePosition};
use geo::{
    BooleanOps, BoundingRect, Coord, CoordsIter, Distance, Euclidean, Geometry, GeometryCollection,
    Intersects, LineString, MultiLineString, MultiPolygon, Point, Polygon,
};

use crate::noding::{key, node, probe, Piece};
use crate::set_ops::{lines, points, polygons};

/// Which empty geometry an empty result becomes.
#[derive(Clone, Copy)]
pub(crate) enum Empty {
    Polygon,
    Line,
}

/// The pieces of an overlay result, grouped by dimension.
#[derive(Default)]
pub(crate) struct Parts {
    pub polygons: Vec<Polygon<f64>>,
    pub lines: Vec<LineString<f64>>,
    pub points: Vec<Point<f64>>,
}

impl Parts {
    /// Adds the members of `geom` (collections are flattened).
    pub fn push_geometry(&mut self, geom: Geometry<f64>) {
        match geom {
            Geometry::Polygon(p) => self.polygons.push(p),
            Geometry::MultiPolygon(m) => self.polygons.extend(m.0),
            Geometry::Rect(r) => self.polygons.push(r.to_polygon()),
            Geometry::Triangle(t) => self.polygons.push(t.to_polygon()),
            Geometry::LineString(l) => self.lines.push(l),
            Geometry::Line(l) => self.lines.push(LineString::from(l)),
            Geometry::MultiLineString(m) => self.lines.extend(m.0),
            Geometry::Point(p) => self.points.push(p),
            Geometry::MultiPoint(m) => self.points.extend(m.0),
            Geometry::GeometryCollection(c) => c.0.into_iter().for_each(|g| self.push_geometry(g)),
        }
    }

    /// Merges overlapping lines and drops the lower-dimensional parts that lie on a
    /// higher-dimensional one, so the parts of a collected result do not repeat each other.
    pub fn absorb(self) -> Parts {
        let area = MultiPolygon::new(self.polygons);
        let mut merged: Vec<LineString<f64>> = Vec::new();
        for l in &self.lines {
            merged = lines_union(&merged, std::slice::from_ref(l));
        }
        let linework = MultiLineString::new(merged);
        let lines = if area.0.is_empty() {
            linework.0.clone()
        } else {
            line_outside_polygon(&linework, &area)
        };
        let mut points: Vec<Point<f64>> = Vec::new();
        for p in self.points {
            if !p.intersects(&area) && !p.intersects(&linework) && !points.contains(&p) {
                points.push(p);
            }
        }
        Parts {
            polygons: area.0,
            lines,
            points,
        }
    }

    pub fn into_geometry(self, empty: Empty) -> Geometry<f64> {
        let present = [
            !self.polygons.is_empty(),
            !self.lines.is_empty(),
            !self.points.is_empty(),
        ];
        match present {
            [false, false, false] => match empty {
                Empty::Polygon => polygons(MultiPolygon::new(vec![])),
                Empty::Line => lines(MultiLineString::new(vec![])),
            },
            [true, false, false] => polygons(MultiPolygon::new(self.polygons)),
            [false, true, false] => lines(MultiLineString::new(self.lines)),
            [false, false, true] => points(self.points),
            _ => {
                let mut members: Vec<Geometry<f64>> = Vec::new();
                members.extend(self.polygons.into_iter().map(Geometry::Polygon));
                members.extend(self.lines.into_iter().map(Geometry::LineString));
                members.extend(self.points.into_iter().map(Geometry::Point));
                Geometry::GeometryCollection(GeometryCollection::new_from(members))
            }
        }
    }
}

fn rings(m: &MultiPolygon<f64>) -> Vec<LineString<f64>> {
    m.0.iter()
        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
        .cloned()
        .collect()
}

fn line(piece: &Piece) -> LineString<f64> {
    LineString::new(piece.coords.clone())
}

/// Coordinate tolerance for "this point lies on that result" tests: `i_overlay` snaps its
/// output vertices, which can move them by about 1e-8 of the extent.
fn tolerance(m: &MultiPolygon<f64>) -> f64 {
    let scale = m
        .coords_iter()
        .fold(1.0_f64, |s, c| s.max(c.x.abs()).max(c.y.abs()));
    1e-7 * scale
}

fn covered(area: &MultiPolygon<f64>, c: Coord<f64>, tol: f64) -> bool {
    if area.0.is_empty() {
        return false;
    }
    let p = Point::from(c);
    area.intersects(&p)
        || Euclidean.distance(&Geometry::MultiPolygon(area.clone()), &Geometry::Point(p)) <= tol
}

/// Intersection of two polygonal geometries: the area, plus the shared edges and isolated
/// touch points that the area does not already contain.
pub(crate) fn polygonal_intersection(
    x: &MultiPolygon<f64>,
    y: &MultiPolygon<f64>,
) -> Geometry<f64> {
    let area = x.intersection(y);
    let (Some(bx), Some(by)) = (x.bounding_rect(), y.bounding_rect()) else {
        return polygons(area);
    };
    if !bx.intersects(&by) {
        return polygons(area);
    }
    let all = MultiPolygon::new(x.0.iter().chain(&y.0).cloned().collect());
    let tol = tolerance(&all);
    let noded = node(&rings(x), &rings(y));

    let mut parts = Parts {
        polygons: area.0.clone(),
        ..Parts::default()
    };
    let mut ends = HashSet::new();
    for piece in noded.a.iter().filter(|p| p.shared) {
        if covered(&area, probe(piece), tol) {
            continue;
        }
        ends.insert(key(piece.coords[0]));
        ends.insert(key(*piece.coords.last().expect("two vertices")));
        parts.lines.push(line(piece));
    }
    parts.points = noded
        .nodes
        .iter()
        .filter(|c| !ends.contains(&key(**c)) && !covered(&area, **c, tol))
        .map(|c| Point::from(*c))
        .collect();
    parts.into_geometry(Empty::Polygon)
}

/// Where a line piece lies relative to a polygon.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum Rel {
    Inside,
    Boundary,
    Outside,
}

pub(crate) struct Classified {
    pub pieces: Vec<(Piece, Rel)>,
    pub nodes: Vec<Coord<f64>>,
}

/// Splits `lines` at the polygon boundary and classifies every piece.
pub(crate) fn classify(lines: &MultiLineString<f64>, polygon: &MultiPolygon<f64>) -> Classified {
    let noded = node(&lines.0, &rings(polygon));
    let pieces = noded
        .a
        .into_iter()
        .map(|piece| {
            let rel = if piece.shared {
                Rel::Boundary
            } else {
                match polygon.coordinate_position(&probe(&piece)) {
                    CoordPos::Inside => Rel::Inside,
                    CoordPos::OnBoundary => Rel::Boundary,
                    CoordPos::Outside => Rel::Outside,
                }
            };
            (piece, rel)
        })
        .collect();
    Classified {
        pieces,
        nodes: noded.nodes,
    }
}

/// Line x polygon intersection: pieces inside or on the polygon, plus isolated touch points.
pub(crate) fn line_polygon_intersection(
    l: &MultiLineString<f64>,
    p: &MultiPolygon<f64>,
) -> Geometry<f64> {
    let classified = classify(l, p);
    let mut parts = Parts::default();
    let mut ends = HashSet::new();
    for (piece, _) in classified.pieces.iter().filter(|(_, r)| *r != Rel::Outside) {
        ends.insert(key(piece.coords[0]));
        ends.insert(key(*piece.coords.last().expect("two vertices")));
        parts.lines.push(line(piece));
    }
    parts.points = classified
        .nodes
        .iter()
        .filter(|c| !ends.contains(&key(**c)))
        .map(|c| Point::from(*c))
        .collect();
    parts.into_geometry(Empty::Line)
}

/// The pieces of `l` outside the closed polygon `p`.
pub(crate) fn line_outside_polygon(
    l: &MultiLineString<f64>,
    p: &MultiPolygon<f64>,
) -> Vec<LineString<f64>> {
    classify(l, p)
        .pieces
        .iter()
        .filter(|(_, rel)| *rel == Rel::Outside)
        .map(|(piece, _)| line(piece))
        .collect()
}

/// Union (and symmetric difference) of a polygon and a line.
pub(crate) fn polygon_line_union(p: &MultiPolygon<f64>, l: &MultiLineString<f64>) -> Geometry<f64> {
    Parts {
        polygons: p.0.clone(),
        lines: line_outside_polygon(l, p),
        points: vec![],
    }
    .into_geometry(Empty::Polygon)
}

/// The noded union of two sets of lines (overlaps appear once).
pub(crate) fn lines_union(a: &[LineString<f64>], b: &[LineString<f64>]) -> Vec<LineString<f64>> {
    let noded = node(a, b);
    let mut out: Vec<LineString<f64>> = noded.a.iter().map(line).collect();
    out.extend(noded.b.iter().filter(|p| !p.shared).map(line));
    out
}

/// Line x line operations.
pub(crate) fn line_line(
    a: &MultiLineString<f64>,
    b: &MultiLineString<f64>,
    op: crate::SetOp,
) -> Geometry<f64> {
    use crate::SetOp::{Difference, Intersection, SymmetricDifference, Union};
    let noded = node(&a.0, &b.0);
    let mut parts = Parts::default();
    match op {
        Intersection => {
            let mut ends = HashSet::new();
            for piece in noded.a.iter().filter(|p| p.shared) {
                ends.insert(key(piece.coords[0]));
                ends.insert(key(*piece.coords.last().expect("two vertices")));
                parts.lines.push(line(piece));
            }
            parts.points = noded
                .nodes
                .iter()
                .filter(|c| !ends.contains(&key(**c)))
                .map(|c| Point::from(*c))
                .collect();
        }
        Difference => parts
            .lines
            .extend(noded.a.iter().filter(|p| !p.shared).map(line)),
        Union => {
            parts.lines.extend(noded.a.iter().map(line));
            parts
                .lines
                .extend(noded.b.iter().filter(|p| !p.shared).map(line));
        }
        SymmetricDifference => {
            parts
                .lines
                .extend(noded.a.iter().filter(|p| !p.shared).map(line));
            parts
                .lines
                .extend(noded.b.iter().filter(|p| !p.shared).map(line));
        }
    }
    parts.into_geometry(Empty::Line)
}
