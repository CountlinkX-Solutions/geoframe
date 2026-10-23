//! Batch H: structural equality, bounding-box queries.
//!
//! # Structural equality
//!
//! `equals_exact` and `equals_identical` compare geometry *structure*: same type, same
//! number of parts and vertices, in the same order. Unlike topological `geom_equals` a
//! reversed ring or a different start vertex is a difference. `equals_exact` accepts
//! coordinates within `tolerance` (Euclidean distance, inclusive; NaN never equals NaN, as
//! in GEOS). `equals_identical` needs bit-for-bit equal coordinates, but treats NaN as equal
//! to NaN (Shapely `equals_identical`). Rows `geo` cannot represent are null.

use geo::{Coord, Geometry, LineString, Polygon};

use crate::GeoColumn;

/// How two coordinates are compared.
#[derive(Clone, Copy)]
enum Match {
    Within(f64),
    Identical,
}

impl Match {
    fn coords(self, a: Coord<f64>, b: Coord<f64>) -> bool {
        match self {
            Self::Within(tolerance) => (a.x - b.x).hypot(a.y - b.y) <= tolerance,
            Self::Identical => same_bits(a.x, b.x) && same_bits(a.y, b.y),
        }
    }
}

fn same_bits(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

fn all_pairs<T>(a: &[T], b: &[T], same: impl Fn(&T, &T) -> bool) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
}

fn line_strings(a: &LineString<f64>, b: &LineString<f64>, m: Match) -> bool {
    all_pairs(&a.0, &b.0, |x, y| m.coords(*x, *y))
}

fn polygons(a: &Polygon<f64>, b: &Polygon<f64>, m: Match) -> bool {
    line_strings(a.exterior(), b.exterior(), m)
        && all_pairs(a.interiors(), b.interiors(), |x, y| line_strings(x, y, m))
}

fn structurally_equal(a: &Geometry<f64>, b: &Geometry<f64>, m: Match) -> bool {
    use Geometry::*;
    match (a, b) {
        (Point(x), Point(y)) => m.coords(x.0, y.0),
        (Line(x), Line(y)) => m.coords(x.start, y.start) && m.coords(x.end, y.end),
        (LineString(x), LineString(y)) => line_strings(x, y, m),
        (Polygon(x), Polygon(y)) => polygons(x, y, m),
        (MultiPoint(x), MultiPoint(y)) => all_pairs(&x.0, &y.0, |p, q| m.coords(p.0, q.0)),
        (MultiLineString(x), MultiLineString(y)) => {
            all_pairs(&x.0, &y.0, |p, q| line_strings(p, q, m))
        }
        (MultiPolygon(x), MultiPolygon(y)) => all_pairs(&x.0, &y.0, |p, q| polygons(p, q, m)),
        (GeometryCollection(x), GeometryCollection(y)) => {
            all_pairs(&x.0, &y.0, |p, q| structurally_equal(p, q, m))
        }
        (Rect(x), Rect(y)) => m.coords(x.min(), y.min()) && m.coords(x.max(), y.max()),
        (Triangle(x), Triangle(y)) => {
            m.coords(x.v1(), y.v1()) && m.coords(x.v2(), y.v2()) && m.coords(x.v3(), y.v3())
        }
        _ => false,
    }
}

impl GeoColumn {
    fn equals_with(&self, other: &GeoColumn, m: Match) -> Result<Vec<Option<bool>>, String> {
        self.zip_pairs(
            other,
            |a, b| Some(structurally_equal(a, b, m)),
            |scalar| scalar.clone(),
            |scalar, row| Some(structurally_equal(row, scalar, m)),
        )
    }

    /// Same structure and vertex order, coordinates within `tolerance` (Shapely
    /// `equals_exact`). The tolerance must be finite and non-negative.
    pub fn equals_exact(
        &self,
        other: &GeoColumn,
        tolerance: f64,
    ) -> Result<Vec<Option<bool>>, String> {
        if !(tolerance.is_finite() && tolerance >= 0.0) {
            return Err(format!(
                "tolerance must be finite and non-negative, got {tolerance}"
            ));
        }
        self.equals_with(other, Match::Within(tolerance))
    }

    /// Exactly the same structure and coordinates; NaN equals NaN (Shapely
    /// `equals_identical`).
    pub fn equals_identical(&self, other: &GeoColumn) -> Result<Vec<Option<bool>>, String> {
        self.equals_with(other, Match::Identical)
    }

    /// Positions of rows whose bounding box intersects `bbox = [minx, miny, maxx, maxy]`
    /// (edges inclusive), in row order. Null and empty rows never match.
    pub fn bbox_intersection(&self, bbox: [f64; 4]) -> Result<Vec<u32>, String> {
        let [minx, miny, maxx, maxy] = bbox;
        if bbox.iter().any(|v| v.is_nan()) || minx > maxx || miny > maxy {
            return Err(format!(
                "invalid bounding box {bbox:?}: expected [minx, miny, maxx, maxy] with min <= max"
            ));
        }
        Ok(self
            .bounds()
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.is_some_and(|b| b[0] <= maxx && b[2] >= minx && b[1] <= maxy && b[3] >= miny)
            })
            .map(|(i, _)| i as u32)
            .collect())
    }
}
