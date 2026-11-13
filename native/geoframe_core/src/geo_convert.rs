//! `geo` geometries of stored rows.
//!
//! `geoarrow_expr_geo`'s conversion rejects a whole `MULTIPOINT` or collection when one member
//! is an empty point (`geo` has no empty point), so every `geo`-based function used to treat
//! such a row as missing. GEOS (3.14, Shapely 2.2) gives the member no part in a computation
//! (area, bounds, centroid, predicates, `is_empty`, coordinate counts) but keeps it as a
//! member (`get_num_geometries`, `get_geometry`, `get_parts`). The two conversions here follow
//! that: [`to_geo`] leaves an empty member point out, and [`to_geo_keeping_empty_points`]
//! keeps it as a point with NaN coordinates, which GeoArrow stores back as the empty point.
//! Rows the upstream conversion accepts are converted by it, unchanged.

use geo::{Geometry, GeometryCollection, MultiPoint, Point};
use geo_traits::{
    CoordTrait, GeometryCollectionTrait, GeometryTrait, GeometryType, MultiPointTrait, PointTrait,
};
use geoarrow_expr_geo::util::to_geo::geometry_to_geo;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EmptyPoints {
    Dropped,
    Kept,
}

/// The row as `geo`, an empty member point left out (computations).
pub(crate) fn to_geo<G: GeometryTrait<T = f64>>(geometry: &G) -> Result<Geometry<f64>, String> {
    convert(geometry, EmptyPoints::Dropped)
}

/// The row as `geo`, an empty member point kept as a NaN point (structure functions).
pub(crate) fn to_geo_keeping_empty_points<G: GeometryTrait<T = f64>>(
    geometry: &G,
) -> Result<Geometry<f64>, String> {
    convert(geometry, EmptyPoints::Kept)
}

fn nan_point() -> Point<f64> {
    Point::new(f64::NAN, f64::NAN)
}

fn member_point<P: PointTrait<T = f64>>(point: &P, empties: EmptyPoints) -> Option<Point<f64>> {
    match point.coord() {
        Some(c) => Some(Point::new(c.x(), c.y())),
        None => (empties == EmptyPoints::Kept).then(nan_point),
    }
}

fn convert<G: GeometryTrait<T = f64>>(
    geometry: &G,
    empties: EmptyPoints,
) -> Result<Geometry<f64>, String> {
    let error = match geometry_to_geo(geometry) {
        Ok(g) => return Ok(g),
        Err(error) => error.to_string(),
    };
    match geometry.as_type() {
        GeometryType::MultiPoint(multi) => Ok(Geometry::MultiPoint(MultiPoint::new(
            multi
                .points()
                .filter_map(|p| member_point(&p, empties))
                .collect(),
        ))),
        GeometryType::GeometryCollection(collection) => {
            let mut members = Vec::new();
            for member in collection.geometries() {
                match member.as_type() {
                    GeometryType::Point(point) if point.coord().is_none() => {
                        members.extend(member_point(point, empties).map(Geometry::Point));
                    }
                    _ => members.push(convert(&member, empties)?),
                }
            }
            Ok(Geometry::GeometryCollection(GeometryCollection(members)))
        }
        _ => Err(error),
    }
}
