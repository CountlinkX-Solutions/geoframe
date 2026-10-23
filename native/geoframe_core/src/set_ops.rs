//! Element-wise binary set operations (overlay) and `union_all`, with GeoPandas names.
//!
//! Pairing, broadcast, CRS and null rules are those of the predicates
//! ([`GeoColumn::zip_pairs`]): the right column may have length 1 and broadcasts.
//!
//! # Supported combinations
//!
//! Geometries fall in three families: polygonal (`Polygon`, `MultiPolygon`, `Rect`,
//! `Triangle`), linear (`Line`, `LineString`, `MultiLineString`) and points (`Point`,
//! `MultiPoint`). A `GeometryCollection` input is only supported by `intersection` (member
//! by member, results collected and de-duplicated); the other operations give null for it.
//!
//! | left x right          | how it is computed                                              |
//! |-----------------------|-----------------------------------------------------------------|
//! | polygonal x polygonal | `BooleanOps` for the area; intersection adds shared edges and touch points |
//! | linear x linear       | noding (see [`crate::noding`]): crossings, overlaps, split lines |
//! | linear x polygonal    | noding against the polygon boundary, pieces classified inside / on / outside |
//! | points x anything     | point-in-geometry tests                                          |
//!
//! Every combination is supported for all four operations. Results follow GEOS: the parts of
//! the result are grouped by dimension, a lower-dimensional part contained in a higher one is
//! absorbed, and a result of several dimensions is a `GeometryCollection` with one member per
//! part (polygons, then lines, then points). A union (or symmetric difference) of a polygon
//! and a line is the polygon plus the pieces of the line outside it; of a polygon and a point,
//! the polygon plus the point when it is outside. The pieces of noded lines are only cut at
//! nodes, never at plain vertices.
//!
//! Two polygons sharing just an edge intersect in that `LineString`, sharing a vertex in a
//! `Point`; a line touching a polygon at one point intersects it in that point.
//!
//! # Empty results
//!
//! An empty result is an empty geometry of the natural type (`POLYGON EMPTY`,
//! `LINESTRING EMPTY`), never null, as in Shapely. `geo` points cannot be empty, so an empty
//! point result is `MULTIPOINT EMPTY`. Single-part results are simplified to the single type.

use geo::orient::{Direction, Orient};
use geo::{
    unary_union, BooleanOps, Geometry, Intersects, LineString, MultiLineString, MultiPoint,
    MultiPolygon, Point, Polygon,
};

use crate::mixed_ops::{self, Empty, Parts};
use crate::predicates::guarded;
use crate::GeoColumn;

/// A binary set operation (overlay).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOp {
    Intersection,
    Union,
    Difference,
    SymmetricDifference,
}

impl SetOp {
    /// Parses the GeoPandas-style operation name used by the NIF layer.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "intersection" => Self::Intersection,
            "union" => Self::Union,
            "difference" => Self::Difference,
            "symmetric_difference" => Self::SymmetricDifference,
            _ => return None,
        })
    }
}

/// A geometry classified into one dimension family.
enum Family {
    Polygonal(MultiPolygon<f64>),
    Linear(MultiLineString<f64>),
    Points(Vec<Point<f64>>),
}

impl Family {
    fn of(geom: &Geometry<f64>) -> Option<Self> {
        Some(match geom {
            Geometry::Polygon(p) => Self::Polygonal(MultiPolygon::new(vec![p.clone()])),
            Geometry::MultiPolygon(m) => Self::Polygonal(m.clone()),
            Geometry::Rect(r) => Self::Polygonal(MultiPolygon::new(vec![r.to_polygon()])),
            Geometry::Triangle(t) => Self::Polygonal(MultiPolygon::new(vec![t.to_polygon()])),
            Geometry::Line(l) => Self::Linear(MultiLineString::new(vec![LineString::from(*l)])),
            Geometry::LineString(l) => Self::Linear(MultiLineString::new(vec![l.clone()])),
            Geometry::MultiLineString(m) => Self::Linear(m.clone()),
            Geometry::Point(p) => Self::Points(vec![*p]),
            Geometry::MultiPoint(m) => Self::Points(m.0.clone()),
            Geometry::GeometryCollection(_) => return None,
        })
    }
}

pub(crate) fn polygons(m: MultiPolygon<f64>) -> Geometry<f64> {
    match m.0.len() {
        0 => Geometry::Polygon(Polygon::new(LineString::new(vec![]), vec![])),
        1 => Geometry::Polygon(m.0.into_iter().next().expect("one polygon")),
        _ => Geometry::MultiPolygon(m),
    }
}

pub(crate) fn lines(m: MultiLineString<f64>) -> Geometry<f64> {
    match m.0.len() {
        0 => Geometry::LineString(LineString::new(vec![])),
        1 => Geometry::LineString(m.0.into_iter().next().expect("one line")),
        _ => Geometry::MultiLineString(m),
    }
}

pub(crate) fn points(pts: Vec<Point<f64>>) -> Geometry<f64> {
    match pts.len() {
        1 => Geometry::Point(pts[0]),
        _ => Geometry::MultiPoint(MultiPoint::new(pts)),
    }
}

/// Removes repeated points, keeping first occurrences (exact coordinate equality).
fn dedup_points(pts: Vec<Point<f64>>) -> Vec<Point<f64>> {
    let mut out: Vec<Point<f64>> = Vec::with_capacity(pts.len());
    for p in pts {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Points of `pts` that do (`keep = true`) or do not intersect `other`.
fn filter_points(pts: &[Point<f64>], other: &Geometry<f64>, keep: bool) -> Geometry<f64> {
    let kept = pts
        .iter()
        .filter(|p| p.intersects(other) == keep)
        .copied()
        .collect();
    points(dedup_points(kept))
}

pub(crate) fn overlay_pair(
    a: &Geometry<f64>,
    b: &Geometry<f64>,
    op: SetOp,
) -> Option<Geometry<f64>> {
    use SetOp::{Difference, Intersection, SymmetricDifference, Union};
    if op == Intersection && (is_collection(a) || is_collection(b)) {
        return intersect_collections(a, b);
    }
    match (Family::of(a)?, Family::of(b)?) {
        (Family::Polygonal(x), Family::Polygonal(y)) => Some(match op {
            Intersection => mixed_ops::polygonal_intersection(&x, &y),
            Union => polygons(x.union(&y)),
            Difference => polygons(x.difference(&y)),
            SymmetricDifference => polygons(x.xor(&y)),
        }),
        (Family::Linear(l), Family::Polygonal(p)) => Some(match op {
            Intersection => mixed_ops::line_polygon_intersection(&l, &p),
            Difference => lines(MultiLineString::new(mixed_ops::line_outside_polygon(
                &l, &p,
            ))),
            Union | SymmetricDifference => mixed_ops::polygon_line_union(&p, &l),
        }),
        (Family::Polygonal(p), Family::Linear(l)) => Some(match op {
            Intersection => mixed_ops::line_polygon_intersection(&l, &p),
            Difference => polygons(p),
            Union | SymmetricDifference => mixed_ops::polygon_line_union(&p, &l),
        }),
        (Family::Linear(x), Family::Linear(y)) => Some(mixed_ops::line_line(&x, &y, op)),
        (Family::Points(x), Family::Points(y)) => Some(match op {
            Intersection => filter_points(&x, b, true),
            Difference => filter_points(&x, b, false),
            Union => points(dedup_points([x, y].concat())),
            SymmetricDifference => {
                let mut out = x.clone();
                out.retain(|p| !y.contains(p));
                let mut rest = y.clone();
                rest.retain(|p| !x.contains(p));
                points(dedup_points([out, rest].concat()))
            }
        }),
        (Family::Points(x), other) => Some(match op {
            Intersection => filter_points(&x, b, true),
            Difference => filter_points(&x, b, false),
            Union | SymmetricDifference => union_with_points(other, b, &x),
        }),
        (other, Family::Points(y)) => Some(match op {
            Intersection => filter_points(&y, a, true),
            Difference => a.clone(),
            Union | SymmetricDifference => union_with_points(other, a, &y),
        }),
    }
}

fn is_collection(geom: &Geometry<f64>) -> bool {
    matches!(geom, Geometry::GeometryCollection(_))
}

fn members(geom: &Geometry<f64>) -> Vec<Geometry<f64>> {
    match geom {
        Geometry::GeometryCollection(c) => c.0.clone(),
        other => vec![other.clone()],
    }
}

/// Intersection where either side is a collection: every pair of members is intersected and
/// the pieces are collected (and de-duplicated) into one result.
fn intersect_collections(a: &Geometry<f64>, b: &Geometry<f64>) -> Option<Geometry<f64>> {
    let mut parts = Parts::default();
    for left in members(a) {
        for right in members(b) {
            parts.push_geometry(overlay_pair(&left, &right, SetOp::Intersection)?);
        }
    }
    Some(parts.absorb().into_geometry(Empty::Polygon))
}

/// `geom` (already classified as `family`) together with the points not already on it.
fn union_with_points(family: Family, geom: &Geometry<f64>, pts: &[Point<f64>]) -> Geometry<f64> {
    let outside: Vec<Point<f64>> = dedup_points(
        pts.iter()
            .filter(|p| !p.intersects(geom))
            .copied()
            .collect(),
    );
    let (parts, empty) = match family {
        Family::Polygonal(p) => (
            Parts {
                polygons: p.0,
                lines: vec![],
                points: outside,
            },
            Empty::Polygon,
        ),
        Family::Linear(l) => (
            Parts {
                polygons: vec![],
                lines: l.0,
                points: outside,
            },
            Empty::Line,
        ),
        Family::Points(_) => unreachable!("point x point is handled by the caller"),
    };
    parts.into_geometry(empty)
}

impl GeoColumn {
    /// Element-wise overlay of this column with `other`; see the module docs for the supported
    /// combinations. Unsupported, null or panicking rows are null.
    pub fn set_op(&self, other: &GeoColumn, op: SetOp) -> Result<GeoColumn, String> {
        let rows = self.zip_pairs(
            other,
            |a, b| guarded(|| overlay_pair(a, b, op)).flatten(),
            |scalar| scalar.clone(),
            |scalar, row| guarded(|| overlay_pair(row, scalar, op)).flatten(),
        )?;
        self.rebuild(&rows)
    }

    /// The union of every non-null row as a one-row column with the same CRS. Polygonal rows
    /// go through `unary_union`; point rows are merged and deduplicated; line rows are merged
    /// with identical lines removed. A mix of families (or any collection) gives a null row,
    /// and no non-null rows give `GEOMETRYCOLLECTION EMPTY`.
    pub fn union_all(&self) -> Result<GeoColumn, String> {
        let result =
            guarded(|| union_all_rows(self.geo_rows().into_iter().flatten().collect())).flatten();
        self.rebuild(&[result])
    }

    /// Grouped [`union_all`](Self::union_all): row `i` belongs to group `group_ids[i]`
    /// (`< n_groups`), and the result has one row per group with the same CRS. Null rows are
    /// skipped; a group without non-null rows gives `GEOMETRYCOLLECTION EMPTY`, and a group
    /// whose rows cannot be unioned gives null, exactly as `union_all` does.
    pub fn union_groups(&self, group_ids: &[u32], n_groups: usize) -> Result<GeoColumn, String> {
        if group_ids.len() != self.len() {
            return Err(format!(
                "group_ids has {} entries but the column has {} rows",
                group_ids.len(),
                self.len()
            ));
        }
        // Validate before allocating: every group needs at least one row to be non-trivial,
        // so more groups than rows is a caller error (and an allocation bomb otherwise).
        if n_groups > group_ids.len() {
            return Err(format!(
                "n_groups ({n_groups}) cannot exceed the number of group ids ({})",
                group_ids.len()
            ));
        }
        if let Some(&id) = group_ids.iter().find(|&&id| id as usize >= n_groups) {
            return Err(format!("group id {id} out of range for {n_groups} groups"));
        }
        let mut buckets: Vec<Vec<Geometry<f64>>> = vec![Vec::new(); n_groups];
        for (geom, &id) in self.geo_rows().into_iter().zip(group_ids) {
            buckets[id as usize].extend(geom);
        }
        let rows: Vec<Option<Geometry<f64>>> = buckets
            .into_iter()
            .map(|rows| guarded(|| union_all_rows(rows)).flatten())
            .collect();
        self.rebuild(&rows)
    }
}

fn union_all_rows(rows: Vec<Geometry<f64>>) -> Option<Geometry<f64>> {
    if rows.is_empty() {
        return Some(Geometry::GeometryCollection(
            geo::GeometryCollection::new_from(vec![]),
        ));
    }
    let families = rows.iter().map(Family::of).collect::<Option<Vec<_>>>()?;
    let mut polys = Vec::new();
    let mut lns: Vec<LineString<f64>> = Vec::new();
    let mut pts = Vec::new();
    for family in families {
        match family {
            Family::Polygonal(m) => polys.push(m.orient(Direction::Default)),
            Family::Linear(m) => lns = mixed_ops::lines_union(&lns, &m.0),
            Family::Points(p) => pts.extend(p),
        }
    }
    let area = if polys.is_empty() {
        MultiPolygon::new(vec![])
    } else {
        unary_union(&polys)
    };
    // Lower dimensions inside a higher one are absorbed, as in GEOS.
    let linework = MultiLineString::new(lns);
    let lns = if area.0.is_empty() {
        linework.0.clone()
    } else {
        mixed_ops::line_outside_polygon(&linework, &area)
    };
    let pts = dedup_points(pts)
        .into_iter()
        .filter(|p| !p.intersects(&area) && !p.intersects(&linework))
        .collect();
    Some(
        Parts {
            polygons: area.0,
            lines: lns,
            points: pts,
        }
        .into_geometry(Empty::Polygon),
    )
}
