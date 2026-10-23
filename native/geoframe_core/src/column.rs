//! GeoArrow-backed geometry column.
//!
//! # Representation
//!
//! A column is one `geoarrow.geometry` array ([`GeometryArray`]), the GeoArrow type for
//! heterogeneous data: a dense union over point, linestring, polygon, multi*, and
//! collection children, per dimension. It is the only native type that can hold whatever
//! WKT/WKB a user supplies without first inspecting or casting it, which matters for a
//! GeoPandas-like API where a column routinely mixes polygons and multipolygons.
//! Specialised single-type arrays (`geoarrow.point`, ...) can be introduced later as an
//! optimisation behind the same API.
//!
//! The CRS lives in the array's GeoArrow extension [`Metadata`], so it round-trips
//! losslessly to `ARROW:extension:metadata` JSON (`crs`, `crs_type`, `edges`).
//!
//! # Error policy
//!
//! Importing invalid WKT/WKB fails the whole call and names the failing row. Per-row
//! computations (measures, centroids) yield nulls, never empty geometries, for rows that
//! are null or cannot be expressed as a `geo` geometry (for example empty points).
//! Computations go through [`geo`] types and drop Z/M ordinates; storage keeps them.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::str::FromStr;
use std::sync::Arc;

use geo::{Area, BoundingRect, Centroid, Geometry, Validation};
use geoarrow_array::array::GeometryArray;
use geoarrow_array::builder::GeometryBuilder;
use geoarrow_array::cast::{to_wkb, to_wkt};
use geoarrow_array::scalar::Geometry as Scalar;
use geoarrow_array::{GeoArrowArray, GeoArrowArrayAccessor, IntoArrow};
use geoarrow_expr_geo::util::to_geo::geometry_to_geo;
use geoarrow_schema::{Crs, GeometryType, Metadata};

/// An immutable column of nullable geometries stored as a GeoArrow array.
#[derive(Debug, Clone)]
pub struct GeoColumn {
    array: GeometryArray,
}

/// Parses `text` as a PROJJSON object, if it is one.
fn as_projjson(text: &str) -> Option<serde_json::Value> {
    if !text.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .filter(serde_json::Value::is_object)
}

fn metadata_for(crs: Option<&str>) -> Arc<Metadata> {
    let crs = match crs {
        None => Crs::default(),
        Some(value) if as_projjson(value).is_some() => {
            Crs::from_projjson(as_projjson(value).unwrap())
        }
        Some(value) if value.contains(':') => Crs::from_authority_code(value.to_string()),
        Some(value) => Crs::from_unknown_crs_type(value.to_string()),
    };
    Arc::new(Metadata::new(crs, None))
}

/// Incremental [`GeoColumn`] construction for readers that produce one geometry at a time and
/// cannot hold them all (a streaming reader's geometries borrow the current record).
pub(crate) struct ColumnBuilder(GeometryBuilder);

impl ColumnBuilder {
    pub fn new(crs: Option<&str>) -> Self {
        Self(GeometryBuilder::new(GeometryType::new(metadata_for(crs))))
    }

    /// Appends one row; `None` is a null geometry.
    pub fn push(
        &mut self,
        geometry: Option<&impl geo_traits::GeometryTrait<T = f64>>,
    ) -> Result<(), String> {
        self.0.push_geometry(geometry).map_err(|e| e.to_string())
    }

    pub fn finish(self) -> GeoColumn {
        GeoColumn {
            array: self.0.finish(),
        }
    }
}

impl GeoColumn {
    fn from_geometries(
        geoms: &[Option<impl geo_traits::GeometryTrait<T = f64>>],
        crs: Option<&str>,
    ) -> Result<Self, String> {
        let mut builder = GeometryBuilder::new(GeometryType::new(metadata_for(crs)));
        for geom in geoms {
            builder
                .push_geometry(geom.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            array: builder.finish(),
        })
    }

    /// Builds a point column from coordinate columns. A row is a null geometry unless both
    /// `x` and `y` are present. The columns must have the same length.
    pub fn from_xy(
        x: &[Option<f64>],
        y: &[Option<f64>],
        crs: Option<&str>,
    ) -> Result<Self, String> {
        if x.len() != y.len() {
            return Err(format!(
                "x and y must have the same length, got {} and {}",
                x.len(),
                y.len()
            ));
        }
        let rows: Vec<Option<Geometry<f64>>> = x
            .iter()
            .zip(y)
            .map(|pair| match pair {
                (Some(x), Some(y)) => Some(Geometry::Point(geo::Point::new(*x, *y))),
                _ => None,
            })
            .collect();
        Self::from_geo_rows(&rows, metadata_for(crs))
    }

    /// Replaces null rows with the single geometry of `fill`; every other row, including
    /// empty geometries, is kept as stored. Fails unless `fill` has exactly one row and a
    /// compatible CRS.
    pub fn fill_null(&self, fill: &GeoColumn) -> Result<Self, String> {
        if fill.len() != 1 {
            return Err(format!(
                "fill value must be a single geometry, got {} rows",
                fill.len()
            ));
        }
        self.check_crs(fill)?;
        let mut builder = GeometryBuilder::new(self.array.extension_type().clone());
        for i in 0..self.len() {
            let row = match self.array.get(i).map_err(|e| e.to_string())? {
                Some(scalar) => Some(scalar),
                None => fill.array.get(0).map_err(|e| e.to_string())?,
            };
            builder
                .push_geometry(row.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            array: builder.finish(),
        })
    }

    /// Parses WKT rows; `None` rows become null geometries.
    pub fn from_wkt<S: AsRef<str>>(rows: &[Option<S>], crs: Option<&str>) -> Result<Self, String> {
        let parsed = rows
            .iter()
            .enumerate()
            .map(|(i, row)| match row {
                None => Ok(None),
                Some(text) => wkt::Wkt::<f64>::from_str(text.as_ref())
                    .map(Some)
                    .map_err(|e| format!("invalid WKT at row {i}: {e}")),
            })
            .collect::<Result<Vec<_>, String>>()?;
        Self::from_geometries(&parsed, crs)
    }

    /// Parses WKB rows; `None` rows become null geometries.
    pub fn from_wkb<B: AsRef<[u8]>>(rows: &[Option<B>], crs: Option<&str>) -> Result<Self, String> {
        let parsed = rows
            .iter()
            .enumerate()
            .map(|(i, row)| match row {
                None => Ok(None),
                Some(bytes) => wkb::reader::read_wkb(bytes.as_ref())
                    .map(Some)
                    .map_err(|e| format!("invalid WKB at row {i}: {e}")),
            })
            .collect::<Result<Vec<_>, String>>()?;
        Self::from_geometries(&parsed, crs)
    }

    /// The underlying GeoArrow array.
    pub fn geometry_array(&self) -> &GeometryArray {
        &self.array
    }

    /// Number of rows, nulls included.
    pub fn len(&self) -> usize {
        GeoArrowArray::len(&self.array)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The CRS as a string: the authority code or opaque string as given, or PROJJSON text.
    pub fn crs(&self) -> Option<String> {
        self.array
            .extension_type()
            .metadata()
            .crs()
            .crs_value()
            .map(|value| match value.as_str() {
                Some(text) => text.to_string(),
                None => value.to_string(),
            })
    }

    /// Replaces the CRS metadata; coordinates are untouched.
    pub fn set_crs(&self, crs: Option<&str>) -> Self {
        Self {
            array: self.array.clone().with_metadata(metadata_for(crs)),
        }
    }

    /// Selects rows by position (repeats and reordering allowed).
    pub fn take(&self, indices: &[usize]) -> Result<Self, String> {
        if let Some(bad) = indices.iter().find(|&&i| i >= self.len()) {
            return Err(format!(
                "index {bad} out of bounds for column of length {}",
                self.len()
            ));
        }
        let scalars = indices
            .iter()
            .map(|&i| self.array.get(i).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, String>>()?;
        let mut builder = GeometryBuilder::new(self.array.extension_type().clone());
        for scalar in &scalars {
            builder
                .push_geometry(scalar.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            array: builder.finish(),
        })
    }

    /// Appends the rows of `columns` in order. Every column must have the same CRS (both
    /// unset counts as equal); a mismatch or an empty list is an error.
    pub fn concat(columns: &[&GeoColumn]) -> Result<Self, String> {
        let first = columns
            .first()
            .ok_or_else(|| "concat needs at least one column".to_string())?;
        if let Some(other) = columns.iter().find(|c| c.crs() != first.crs()) {
            return Err(format!(
                "CRS mismatch: {:?} and {:?}; reproject or set the CRS first",
                first.crs(),
                other.crs()
            ));
        }
        let mut builder = GeometryBuilder::new(first.array.extension_type().clone());
        for column in columns {
            for i in 0..column.len() {
                let scalar = column.array.get(i).map_err(|e| e.to_string())?;
                builder
                    .push_geometry(scalar.as_ref())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(Self {
            array: builder.finish(),
        })
    }

    pub fn to_wkt(&self) -> Result<Vec<Option<String>>, String> {
        let wkt = to_wkt::<i32>(&self.array).map_err(|e| e.to_string())?;
        Ok(wkt
            .inner()
            .iter()
            .map(|row| row.map(str::to_string))
            .collect())
    }

    pub fn to_wkb(&self) -> Result<Vec<Option<Vec<u8>>>, String> {
        let wkb = to_wkb::<i32>(&self.array).map_err(|e| e.to_string())?;
        Ok(wkb
            .inner()
            .iter()
            .map(|row| row.map(<[u8]>::to_vec))
            .collect())
    }

    /// GeoJSON geometry objects as JSON text, one per row; `None` for null rows and rows `geo`
    /// cannot represent (the same policy as the measures). GeoJSON has no CRS member
    /// (RFC 7946), so coordinates are emitted as stored.
    pub fn to_geojson(&self) -> Vec<Option<String>> {
        self.map_geo(|g| Some(geojson::Geometry::from(g).to_string()))
    }

    /// Per-row `geo` geometry; `None` for null rows and rows `geo` cannot represent.
    pub(crate) fn geo_rows(&self) -> Vec<Option<Geometry<f64>>> {
        (0..self.len())
            .map(|i| match self.array.get(i) {
                Ok(Some(scalar)) => geometry_to_geo(&scalar).ok(),
                _ => None,
            })
            .collect()
    }

    fn map_geo<T>(&self, f: impl Fn(&Geometry<f64>) -> Option<T>) -> Vec<Option<T>> {
        self.geo_rows()
            .iter()
            .map(|row| row.as_ref().and_then(&f))
            .collect()
    }

    /// Planar unsigned area; zero for points and lines.
    pub fn area(&self) -> Vec<Option<f64>> {
        self.map_geo(|g| Some(g.unsigned_area()))
    }

    /// Planar length: line length, polygon perimeter, zero for points (GeoPandas semantics).
    pub fn length(&self) -> Vec<Option<f64>> {
        self.map_geo(|g| Some(euclidean_length(g)))
    }

    pub fn is_valid(&self) -> Vec<Option<bool>> {
        self.map_geo(|g| Some(g.is_valid()))
    }

    /// `[minx, miny, maxx, maxy]` per row; `None` for null and empty geometries.
    pub fn bounds(&self) -> Vec<Option<[f64; 4]>> {
        self.map_geo(|g| {
            g.bounding_rect()
                .map(|r| [r.min().x, r.min().y, r.max().x, r.max().y])
        })
    }

    /// Bounds over all non-null, non-empty rows.
    pub fn total_bounds(&self) -> Option<[f64; 4]> {
        self.bounds().into_iter().flatten().reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
    }

    /// Centroids as a point column with the same CRS; null where undefined.
    pub fn centroid(&self) -> Result<Self, String> {
        self.map_geometry(|g| g.centroid().map(Geometry::Point))
    }

    /// Builds a column from per-row `geo` geometries (`None` = null) with the given metadata.
    fn from_geo_rows(
        rows: &[Option<Geometry<f64>>],
        metadata: Arc<Metadata>,
    ) -> Result<Self, String> {
        let mut builder = GeometryBuilder::new(GeometryType::new(metadata));
        for row in rows {
            builder
                .push_geometry(row.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            array: builder.finish(),
        })
    }

    /// Builds a column from per-row `geo` geometries carrying this column's CRS.
    pub(crate) fn rebuild(&self, rows: &[Option<Geometry<f64>>]) -> Result<Self, String> {
        Self::from_geo_rows(rows, self.array.extension_type().metadata().clone())
    }

    /// The generic constructive path: maps every convertible row through `f` and rebuilds a
    /// column with the same CRS. Rows that are null, unrepresentable, make `f` return `None`
    /// or make `f` panic (geometry algorithms can on degenerate input) become null. Output
    /// is 2D: Z/M ordinates are not carried through `geo`.
    pub(crate) fn map_geometry(
        &self,
        f: impl Fn(&Geometry<f64>) -> Option<Geometry<f64>>,
    ) -> Result<Self, String> {
        let rows = self.map_geo(|g| catch_unwind(AssertUnwindSafe(|| f(g))).ok().flatten());
        let metadata = self.array.extension_type().metadata().clone();
        Self::from_geo_rows(&rows, metadata)
    }

    /// Maps each non-null stored scalar through `f`, including rows `geo` cannot represent
    /// (such as empty points), which `geo_rows` hides. Null rows give `None`.
    pub(crate) fn map_scalars<T>(&self, f: impl Fn(&Scalar<'_>) -> T) -> Vec<Option<T>> {
        (0..self.len())
            .map(|i| match self.array.get(i) {
                Ok(Some(scalar)) => Some(f(&scalar)),
                _ => None,
            })
            .collect()
    }
}

fn euclidean_length(geom: &Geometry<f64>) -> f64 {
    use geo::{Euclidean, Length};
    match geom {
        Geometry::Line(g) => Euclidean.length(g),
        Geometry::LineString(g) => Euclidean.length(g),
        Geometry::MultiLineString(g) => Euclidean.length(g),
        Geometry::Polygon(g) => polygon_perimeter(g),
        Geometry::MultiPolygon(g) => g.0.iter().map(polygon_perimeter).sum(),
        Geometry::Rect(g) => polygon_perimeter(&g.to_polygon()),
        Geometry::Triangle(g) => polygon_perimeter(&g.to_polygon()),
        Geometry::GeometryCollection(g) => g.0.iter().map(euclidean_length).sum(),
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0.0,
    }
}

fn polygon_perimeter(polygon: &geo::Polygon<f64>) -> f64 {
    use geo::{Euclidean, Length};
    Euclidean.length(polygon.exterior())
        + polygon
            .interiors()
            .iter()
            .map(|ring| Euclidean.length(ring))
            .sum::<f64>()
}
