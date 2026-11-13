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
use std::sync::Arc;

use geo::{Area, BoundingRect, Centroid, Geometry};
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
        push_guarded(&mut self.0, geometry)
    }

    pub fn finish(self) -> GeoColumn {
        GeoColumn {
            array: self.0.finish(),
        }
    }
}

/// Pushes one parsed geometry. The GeoArrow builder stores one dimension per collection (and
/// panics instead of returning an error on a collection whose members differ), so such a
/// collection is first promoted to the dimension GEOS gives it (`mixed_dims`); the builder is
/// dropped by the caller on error.
fn push_guarded(
    builder: &mut GeometryBuilder,
    geometry: Option<&impl geo_traits::GeometryTrait<T = f64>>,
) -> Result<(), String> {
    if let Some(g) = geometry.filter(|g| mixes_dimensions(*g)) {
        let bytes = crate::mixed_dims::promoted_wkb(g)?;
        let promoted = wkb::reader::read_wkb(&bytes).map_err(|e| e.to_string())?;
        return push_caught(builder, Some(&promoted));
    }
    push_caught(builder, geometry)
}

/// Last-resort guard: the promotion above mirrors the known panic, not every possible one.
fn push_caught(
    builder: &mut GeometryBuilder,
    geometry: Option<&impl geo_traits::GeometryTrait<T = f64>>,
) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| builder.push_geometry(geometry)))
        .map_err(|_| MIXED_DIMENSIONS_ERROR.to_string())?
        .map_err(|e| e.to_string())
}

const MIXED_DIMENSIONS_ERROR: &str = "unsupported geometry: a collection cannot mix dimensions \
     (XY, XYZ, XYM, XYZM) among its members";

/// True when pushing `geometry` would make the GeoArrow builder panic: a collection (with
/// other than one member; a single member is unwrapped and pushed on its own) holding a
/// member whose dimension differs from the collection's own. Checked recursively.
fn mixes_dimensions<G: geo_traits::GeometryTrait<T = f64>>(geometry: &G) -> bool {
    use geo_traits::{GeometryCollectionTrait, GeometryTrait as _, GeometryType};
    let GeometryType::GeometryCollection(gc) = geometry.as_type() else {
        return false;
    };
    if gc.num_geometries() == 1 {
        return gc.geometries().next().is_some_and(|g| mixes_dimensions(&g));
    }
    let dim = gc.dim();
    gc.geometries()
        .any(|g| !dimensions_agree(dim, g.dim()) || mixes_dimensions(&g))
}

/// Whether the GeoArrow builder accepts a `member` dimension inside a `collection` of the
/// given dimension (it also accepts an unnamed `Unknown(n)` by ordinate count).
fn dimensions_agree(collection: geo_traits::Dimensions, member: geo_traits::Dimensions) -> bool {
    use geo_traits::Dimensions::{Unknown, Xy, Xym, Xyz, Xyzm};
    match (collection, member) {
        (a, b) if a == b => true,
        (Xy, Unknown(2)) | (Unknown(2), Xy) => true,
        (Xyz | Xym, Unknown(3)) | (Unknown(3), Xyz | Xym) => true,
        (Xyzm, Unknown(4)) | (Unknown(4), Xyzm) => true,
        _ => false,
    }
}

/// Maximum parenthesis nesting accepted in WKT; `wkt` parses recursively without a limit.
pub const MAX_WKT_NESTING_DEPTH: usize = 64;

/// True when `text` nests parentheses deeper than `max` (single O(n) scan).
pub(crate) fn wkt_nesting_exceeds(text: &str, max: usize) -> bool {
    let mut depth = 0usize;
    for b in text.bytes() {
        match b {
            b'(' => {
                depth += 1;
                if depth > max {
                    return true;
                }
            }
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

impl GeoColumn {
    fn from_geometries(
        geoms: &[Option<impl geo_traits::GeometryTrait<T = f64>>],
        crs: Option<&str>,
    ) -> Result<Self, String> {
        let mut builder = GeometryBuilder::new(GeometryType::new(metadata_for(crs)));
        for (i, geom) in geoms.iter().enumerate() {
            push_guarded(&mut builder, geom.as_ref()).map_err(|e| format!("row {i}: {e}"))?;
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
                Some(text) => {
                    if wkt_nesting_exceeds(text.as_ref(), MAX_WKT_NESTING_DEPTH) {
                        return Err(format!("invalid WKT at row {i}: nesting too deep"));
                    }
                    let parsed = crate::wkt_read::parse(text.as_ref())
                        .map_err(|e| format!("invalid WKT at row {i}: {e}"))?;
                    if crate::mixed_dims::tagged_collection_conflict(&parsed) {
                        return Err(format!(
                            "invalid WKT at row {i}: cannot mix dimensionality in a geometry \
                             (a member of a GEOMETRYCOLLECTION Z, M or ZM has another dimension)"
                        ));
                    }
                    Ok(Some(parsed))
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        Self::from_geometries(&parsed, crs)
    }

    /// Parses WKB rows; `None` rows become null geometries. EWKB is accepted (SRID and Z/M
    /// flags). Without an explicit `crs`, an embedded SRID becomes `"EPSG:<srid>"`; rows with
    /// different SRIDs are rejected unless `crs` is given. SRID 0 means unknown.
    pub fn from_wkb<B: AsRef<[u8]>>(rows: &[Option<B>], crs: Option<&str>) -> Result<Self, String> {
        let mut srid: Option<i32> = None;
        let mut converted: Vec<Option<Vec<u8>>> = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let Some(bytes) = row else {
                converted.push(None);
                continue;
            };
            // A rewrite failure falls through so the reader reports the problem.
            match crate::ewkb::ewkb_to_iso(bytes.as_ref()) {
                Ok((iso, row_srid)) => {
                    if let Some(s) = row_srid.filter(|&s| s > 0) {
                        match srid {
                            Some(prev) if prev != s && crs.is_none() => {
                                return Err(format!(
                                    "mixed SRIDs in EWKB input ({prev} and {s} at row {i}); pass a crs to override"
                                ))
                            }
                            _ => srid = Some(s),
                        }
                    }
                    converted.push(Some(iso));
                }
                // Depth errors are fatal: handing the bytes to the reader would only trade
                // one recursion limit for another.
                Err(e) if e == crate::ewkb::NESTING_ERROR => {
                    return Err(format!("invalid WKB at row {i}: {e}"))
                }
                // A malformed curve cannot be handed to the reader, which has no curve types.
                Err(e)
                    if crate::curves::peek_base(bytes.as_ref(), 0)
                        .is_ok_and(|b| (8..=12).contains(&b)) =>
                {
                    return Err(format!("invalid WKB at row {i}: {e}"))
                }
                Err(_) => converted.push(Some(bytes.as_ref().to_vec())),
            }
        }
        let parsed = converted
            .iter()
            .enumerate()
            .map(|(i, row)| match row {
                None => Ok(None),
                Some(bytes) => wkb::reader::read_wkb(bytes)
                    .map(Some)
                    .map_err(|e| format!("invalid WKB at row {i}: {e}")),
            })
            .collect::<Result<Vec<_>, String>>()?;
        let inferred = srid.map(|s| format!("EPSG:{s}"));
        Self::from_geometries(&parsed, crs.or(inferred.as_deref()))
    }

    /// The GeoArrow extension type (CRS metadata included) new arrays are built with.
    pub(crate) fn array_type(&self) -> GeometryType {
        self.array.extension_type().clone()
    }

    /// Wraps a freshly built array, which must carry this column's extension type.
    pub(crate) fn with_array(&self, array: GeometryArray) -> Self {
        Self { array }
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
        let same = |a: Option<String>, b: Option<String>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => crate::crs_equivalent(&a, &b),
            _ => false,
        };
        if let Some(other) = columns.iter().find(|c| !same(c.crs(), first.crs())) {
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
        // The batch writer panics on a MULTIPOINT with an empty member point and writes an
        // empty member line or ring as `()`; a column holding one is written row by row
        // (`wkt_write`), which gives the same text for the rest.
        let special = self.map_scalars(|s| crate::wkt_write::has_empty_member(s));
        if special.contains(&Some(true)) {
            return self
                .map_scalars(|s| crate::wkt_write::geometry_wkt(s))
                .into_iter()
                .map(Option::transpose)
                .collect();
        }
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

    /// Like [`to_wkb`](Self::to_wkb) but PostGIS EWKB carrying `srid`.
    pub fn to_ewkb(&self, srid: i32) -> Result<Vec<Option<Vec<u8>>>, String> {
        self.to_wkb()?
            .into_iter()
            .map(|row| row.map(|b| crate::ewkb::wkb_to_ewkb(&b, srid)).transpose())
            .collect()
    }

    /// GeoJSON geometry objects as JSON text, one per row; `None` for null rows and empty
    /// points (GeoJSON cannot say them). Z is the third ordinate and M is dropped (RFC 7946).
    /// GeoJSON has no CRS member, so coordinates are emitted as stored.
    pub fn to_geojson(&self) -> Vec<Option<String>> {
        self.map_scalars(|scalar| crate::zm_io::geometry_to_geojson(scalar))
            .into_iter()
            .map(Option::flatten)
            .collect()
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
        self.map_geo(|g| Some(crate::validity::is_valid(g)))
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

#[cfg(test)]
mod mixed_dimension_tests {
    use super::*;
    use std::str::FromStr;

    fn mixed(text: &str) -> bool {
        mixes_dimensions(&wkt::Wkt::<f64>::from_str(text).unwrap())
    }

    #[test]
    fn mixed_member_dimensions_are_detected_before_the_builder_runs() {
        assert!(mixed("GEOMETRYCOLLECTION (POINT (1 2), POINT Z (1 2 3))"));
        assert!(mixed("GEOMETRYCOLLECTION (POINT Z (1 2 3), POINT (1 2))"));
        assert!(mixed(
            "GEOMETRYCOLLECTION (POINT M (1 2 3), POINT Z (1 2 3))"
        ));
        assert!(mixed(
            "GEOMETRYCOLLECTION (POINT (1 2), LINESTRING Z (0 0 1, 1 1 1))"
        ));
        assert!(mixed(
            "GEOMETRYCOLLECTION (POINT (1 2), GEOMETRYCOLLECTION (POINT Z (1 2 3), POINT Z (4 5 6)))"
        ));
    }

    #[test]
    fn uniform_collections_and_plain_geometries_are_not_flagged() {
        assert!(!mixed("POINT Z (1 2 3)"));
        assert!(!mixed(
            "GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))"
        ));
        assert!(!mixed(
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), POINT Z (4 5 6))"
        ));
        assert!(!mixed("GEOMETRYCOLLECTION EMPTY"));
    }

    fn read_wkt(text: &str) -> Result<String, String> {
        let column = GeoColumn::from_wkt(&[Some(text)], None)?;
        Ok(column.to_wkt()?[0].clone().unwrap())
    }

    fn read_wkb(bytes: &[u8]) -> String {
        let column = GeoColumn::from_wkb(&[Some(bytes)], None).unwrap();
        column.to_wkt().unwrap()[0].clone().unwrap()
    }

    /// ISO WKB, little endian: a point with the given ordinates (2 to 4 values; `zm` names
    /// them) and a collection of members under a header type code.
    fn point_wkb(type_code: u32, ordinates: &[f64]) -> Vec<u8> {
        let mut out = vec![1];
        out.extend(type_code.to_le_bytes());
        ordinates.iter().for_each(|v| out.extend(v.to_le_bytes()));
        out
    }

    fn collection_wkb(type_code: u32, members: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![1];
        out.extend(type_code.to_le_bytes());
        out.extend((members.len() as u32).to_le_bytes());
        members.iter().for_each(|m| out.extend(m));
        out
    }

    // Shapely 2.2 / GEOS 3.14 read these collections; the members keep their own dimension
    // there and `get_coordinates` reports a missing ordinate as NaN, which is what a member
    // promoted to the collection's dimension holds here.
    #[test]
    fn a_collection_with_two_and_three_dimensional_members_is_promoted_with_nan_z() {
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION (POINT (1 2), POINT Z (3 4 5))").unwrap(),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),POINT Z(3 4 5))"
        );
        assert_eq!(
            read_wkt(
                "GEOMETRYCOLLECTION (LINESTRING Z (0 0 1, 1 1 2), POLYGON ((0 0, 1 0, 1 1, 0 0)))"
            )
            .unwrap(),
            "GEOMETRYCOLLECTION Z(LINESTRING Z(0 0 1,1 1 2),\
             POLYGON Z((0 0 NaN,1 0 NaN,1 1 NaN,0 0 NaN)))"
        );
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION (POINT (1 2), MULTIPOINT Z ((3 4 5)))").unwrap(),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),MULTIPOINT Z((3 4 5)))"
        );
    }

    #[test]
    fn measured_and_three_dimensional_members_make_an_xyzm_collection() {
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION (POINT M (1 2 3), POINT Z (3 4 5))").unwrap(),
            "GEOMETRYCOLLECTION ZM(POINT ZM(1 2 NaN 3),POINT ZM(3 4 5 NaN))"
        );
    }

    #[test]
    fn a_nested_mixed_collection_reports_that_geoarrow_cannot_nest_collections() {
        // GeoArrow stores no collection inside a collection, whatever the dimensions.
        for text in [
            "GEOMETRYCOLLECTION (GEOMETRYCOLLECTION (POINT (1 2), POINT (5 6)), POINT M (3 4 9))",
            "GEOMETRYCOLLECTION (GEOMETRYCOLLECTION (POINT (1 2), POINT (5 6)), POINT (3 4))",
        ] {
            let err = read_wkt(text).unwrap_err();
            assert!(err.contains("nested geometry collections"), "{text}: {err}");
        }
    }

    #[test]
    fn empty_members_count_like_geos() {
        // An empty point or line keeps its dimension; an empty multi geometry adds none.
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION (POINT (1 2), POINT Z EMPTY)").unwrap(),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),POINT Z EMPTY)"
        );
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING Z EMPTY)").unwrap(),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),LINESTRING Z EMPTY)"
        );
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION (POINT (1 2), MULTIPOINT Z EMPTY)").unwrap(),
            "GEOMETRYCOLLECTION(POINT(1 2),MULTIPOINT EMPTY)"
        );
    }

    #[test]
    fn the_wkb_collection_header_does_not_decide_the_dimension() {
        // GEOS: a GC Z header around 2D points gives a 2D collection; an M header around a 2D
        // and a Z point gives a Z collection.
        let xy = || [point_wkb(1, &[1.0, 2.0]), point_wkb(1, &[3.0, 4.0])];
        assert_eq!(
            read_wkb(&collection_wkb(1007, &xy())),
            "GEOMETRYCOLLECTION(POINT(1 2),POINT(3 4))"
        );
        let mixed = [point_wkb(1, &[1.0, 2.0]), point_wkb(1001, &[3.0, 4.0, 5.0])];
        assert_eq!(
            read_wkb(&collection_wkb(2007, &mixed)),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),POINT Z(3 4 5))"
        );
        assert_eq!(
            read_wkb(&collection_wkb(7, &mixed)),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),POINT Z(3 4 5))"
        );
    }

    #[test]
    fn a_member_of_another_dimension_in_a_tagged_wkt_collection_is_an_error_as_in_geos() {
        for text in [
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), POINT (3 4))",
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), POINT M (1 2 3))",
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), POINT EMPTY)",
            "GEOMETRYCOLLECTION ZM (POINT ZM (1 2 3 4), POINT Z (1 2 3))",
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), GEOMETRYCOLLECTION (POINT Z (1 2 3)))",
            "GEOMETRYCOLLECTION (POINT (1 2), GEOMETRYCOLLECTION Z (POINT (1 2)))",
        ] {
            let err = read_wkt(text).unwrap_err();
            assert!(err.contains("cannot mix dimensionality"), "{text}: {err}");
        }
        assert_eq!(
            read_wkt("GEOMETRYCOLLECTION Z (POINT Z (1 2 3), POINT Z (4 5 6))").unwrap(),
            "GEOMETRYCOLLECTION Z(POINT Z(1 2 3),POINT Z(4 5 6))"
        );
    }
}
