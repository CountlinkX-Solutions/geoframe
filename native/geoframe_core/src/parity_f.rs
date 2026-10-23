//! Batch F: geodesic measures on the WGS 84 ellipsoid.
//!
//! # CRS handling
//!
//! Every measure needs to know its coordinates are degrees: a column without a CRS is an
//! error (`set_crs first`), a geographic CRS is read as lon/lat as it is, and a projected CRS
//! is reprojected to `OGC:CRS84` first. The ellipsoid is always WGS 84, so a geographic CRS
//! on another datum (for example NAD27) is measured on WGS 84 too: a small error, not a
//! datum transformation.
//!
//! # Distance
//!
//! `Point` to `Point` distances are exact (Karney's geodesic, or the haversine sphere of
//! radius 6 371 008.8 m). For any other pair the nearest points are found *planar, in
//! lon/lat degrees* and the geodesic distance between those two points is returned. That
//! is an approximation: the planar nearest pair is not always the geodesic nearest pair
//! (it drifts at high latitudes and across the antimeridian). Intersecting pairs give 0.

use geo::orient::{Direction, Orient};
use geo::{
    Geodesic, GeodesicArea, Geometry, Haversine, Intersects, Length, LineString, Point, Polygon,
};

use crate::parity_c::nearest_pair;
use crate::predicates::guarded;
use crate::GeoColumn;

impl GeoColumn {
    /// This column in lon/lat degrees, ready for geodesic measuring.
    fn lonlat(&self) -> Result<GeoColumn, String> {
        match crate::crs_is_geographic(
            &self
                .crs()
                .ok_or_else(|| "geodesic measures need a CRS; set_crs first".to_string())?,
        ) {
            Some(true) => Ok(self.clone()),
            _ => self.to_crs("OGC:CRS84"),
        }
    }

    /// Geodesic area in square metres, unsigned, holes subtracted; zero for points and lines.
    pub fn area_geodesic(&self) -> Result<Vec<Option<f64>>, String> {
        Ok(self
            .lonlat()?
            .geo_rows()
            .iter()
            .map(|row| row.as_ref().and_then(|g| guarded(|| geodesic_area(g))))
            .collect())
    }

    /// Geodesic length in metres: line length, polygon perimeter (holes included), zero for
    /// points.
    pub fn length_geodesic(&self) -> Result<Vec<Option<f64>>, String> {
        Ok(self
            .lonlat()?
            .geo_rows()
            .iter()
            .map(|row| row.as_ref().and_then(|g| guarded(|| geodesic_length(g))))
            .collect())
    }

    /// Distance in metres between each pair on the WGS 84 geodesic, or on the haversine
    /// sphere when `haversine` is set. See the module docs for non-point pairs.
    pub fn distance_geodesic(
        &self,
        other: &GeoColumn,
        haversine: bool,
    ) -> Result<Vec<Option<f64>>, String> {
        self.check_crs(other)?;
        let (left, right) = (self.lonlat()?, other.lonlat()?);
        left.zip_pairs(
            &right,
            |a, b| guarded(|| pair_distance(a, b, haversine)).flatten(),
            |scalar| scalar.clone(),
            |scalar, row| guarded(|| pair_distance(row, scalar, haversine)).flatten(),
        )
    }
}

fn metres(a: Point<f64>, b: Point<f64>, haversine: bool) -> f64 {
    use geo::Distance;
    if haversine {
        Haversine.distance(a, b)
    } else {
        Geodesic.distance(a, b)
    }
}

fn pair_distance(a: &Geometry<f64>, b: &Geometry<f64>, haversine: bool) -> Option<f64> {
    if let (Geometry::Point(p), Geometry::Point(q)) = (a, b) {
        return Some(metres(*p, *q, haversine));
    }
    if a.intersects(b) {
        return Some(0.0);
    }
    let (p, q) = nearest_pair(a, b)?;
    Some(metres(p, q, haversine))
}

fn ring_length(ring: &LineString<f64>) -> f64 {
    Geodesic.length(ring)
}

fn polygon_perimeter(polygon: &Polygon<f64>) -> f64 {
    ring_length(polygon.exterior()) + polygon.interiors().iter().map(ring_length).sum::<f64>()
}

fn geodesic_length(geom: &Geometry<f64>) -> f64 {
    match geom {
        Geometry::Line(g) => Geodesic.length(g),
        Geometry::LineString(g) => Geodesic.length(g),
        Geometry::MultiLineString(g) => Geodesic.length(g),
        Geometry::Polygon(g) => polygon_perimeter(g),
        Geometry::MultiPolygon(g) => g.0.iter().map(polygon_perimeter).sum(),
        Geometry::Rect(g) => polygon_perimeter(&g.to_polygon()),
        Geometry::Triangle(g) => polygon_perimeter(&g.to_polygon()),
        Geometry::GeometryCollection(g) => g.0.iter().map(geodesic_length).sum(),
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0.0,
    }
}

/// Unsigned area. `geo` returns the complement on the ellipsoid for a clockwise exterior, so
/// rings are oriented (exterior counter-clockwise, holes clockwise) first.
fn polygon_area(polygon: &Polygon<f64>) -> f64 {
    polygon.orient(Direction::Default).geodesic_area_unsigned()
}

fn geodesic_area(geom: &Geometry<f64>) -> f64 {
    match geom {
        Geometry::Polygon(g) => polygon_area(g),
        Geometry::MultiPolygon(g) => g.0.iter().map(polygon_area).sum(),
        Geometry::Rect(g) => polygon_area(&g.to_polygon()),
        Geometry::Triangle(g) => polygon_area(&g.to_polygon()),
        Geometry::GeometryCollection(g) => g.0.iter().map(geodesic_area).sum(),
        _ => 0.0,
    }
}
