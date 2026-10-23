//! Element-wise binary set operations (overlay) and `union_all`, with GeoPandas names.
//!
//! Pairing, broadcast, CRS and null rules are those of the predicates
//! ([`GeoColumn::zip_pairs`]): the right column may have length 1 and broadcasts.
//!
//! # Supported combinations
//!
//! Geometries fall in three families: polygonal (`Polygon`, `MultiPolygon`, `Rect`,
//! `Triangle`), linear (`Line`, `LineString`, `MultiLineString`) and points (`Point`,
//! `MultiPoint`). `GeometryCollection` is unsupported and gives null.
//!
//! | left x right      | intersection | difference | union / symmetric difference |
//! |-------------------|--------------|------------|------------------------------|
//! | polygonal x polygonal | `BooleanOps` | `BooleanOps` | `BooleanOps`           |
//! | linear x polygonal    | `clip`       | `clip` (inverted) | null              |
//! | polygonal x linear    | `clip`       | left unchanged    | null              |
//! | points x anything     | points kept by `Intersects` | points not intersecting | points x points only |
//! | polygonal/linear x points | points kept | left unchanged | null              |
//! | linear x linear       | null         | null       | null                         |
//!
//! Polygonal results contain only the 2-D part: polygons sharing just an edge intersect in
//! `POLYGON EMPTY`, where Shapely returns the edge. Line results contain only the 1-D part: a line touching a polygon at a single point gives
//! an empty line, where Shapely returns that point. Mixed-dimension union and symmetric
//! difference would be geometry collections and are not provided.
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

fn polygons(m: MultiPolygon<f64>) -> Geometry<f64> {
    match m.0.len() {
        0 => Geometry::Polygon(Polygon::new(LineString::new(vec![]), vec![])),
        1 => Geometry::Polygon(m.0.into_iter().next().expect("one polygon")),
        _ => Geometry::MultiPolygon(m),
    }
}

fn lines(m: MultiLineString<f64>) -> Geometry<f64> {
    match m.0.len() {
        0 => Geometry::LineString(LineString::new(vec![])),
        1 => Geometry::LineString(m.0.into_iter().next().expect("one line")),
        _ => Geometry::MultiLineString(m),
    }
}

fn points(pts: Vec<Point<f64>>) -> Geometry<f64> {
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

fn overlay(a: &Geometry<f64>, b: &Geometry<f64>, op: SetOp) -> Option<Geometry<f64>> {
    use SetOp::{Difference, Intersection, SymmetricDifference, Union};
    match (Family::of(a)?, Family::of(b)?) {
        (Family::Polygonal(x), Family::Polygonal(y)) => Some(polygons(match op {
            Intersection => x.intersection(&y),
            Union => x.union(&y),
            Difference => x.difference(&y),
            SymmetricDifference => x.xor(&y),
        })),
        (Family::Linear(l), Family::Polygonal(p)) => match op {
            Intersection => Some(lines(p.clip(&l, false))),
            Difference => Some(lines(p.clip(&l, true))),
            _ => None,
        },
        (Family::Polygonal(_), Family::Linear(l)) => match op {
            Intersection => Some(lines(a_polygons(a)?.clip(&l, false))),
            Difference => Some(a.clone()),
            _ => None,
        },
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
        (Family::Points(x), _) => match op {
            Intersection => Some(filter_points(&x, b, true)),
            Difference => Some(filter_points(&x, b, false)),
            _ => None,
        },
        (_, Family::Points(y)) => match op {
            Intersection => Some(filter_points(&y, a, true)),
            Difference => Some(a.clone()),
            _ => None,
        },
        (Family::Linear(_), Family::Linear(_)) => None,
    }
}

/// The polygonal family of `geom` as a multipolygon.
fn a_polygons(geom: &Geometry<f64>) -> Option<MultiPolygon<f64>> {
    match Family::of(geom)? {
        Family::Polygonal(m) => Some(m),
        _ => None,
    }
}

impl GeoColumn {
    /// Element-wise overlay of this column with `other`; see the module docs for the supported
    /// combinations. Unsupported, null or panicking rows are null.
    pub fn set_op(&self, other: &GeoColumn, op: SetOp) -> Result<GeoColumn, String> {
        let rows = self.zip_pairs(
            other,
            |a, b| guarded(|| overlay(a, b, op)).flatten(),
            |scalar| scalar.clone(),
            |scalar, row| guarded(|| overlay(row, scalar, op)).flatten(),
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
        let mut buckets: Vec<Vec<Geometry<f64>>> = vec![Vec::new(); n_groups];
        for (geom, &id) in self.geo_rows().into_iter().zip(group_ids) {
            let bucket = buckets
                .get_mut(id as usize)
                .ok_or_else(|| format!("group id {id} out of range for {n_groups} groups"))?;
            bucket.extend(geom);
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
            Family::Linear(m) => {
                for line in m.0 {
                    if !lns.contains(&line) {
                        lns.push(line);
                    }
                }
            }
            Family::Points(p) => pts.extend(p),
        }
    }
    match (polys.is_empty(), lns.is_empty(), pts.is_empty()) {
        (false, true, true) => Some(polygons(unary_union(&polys))),
        (true, false, true) => Some(lines(MultiLineString::new(lns))),
        (true, true, false) => Some(points(dedup_points(pts))),
        _ => None,
    }
}
