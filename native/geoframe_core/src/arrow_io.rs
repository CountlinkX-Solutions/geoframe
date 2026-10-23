//! Arrow-based table interchange: Feather (Arrow IPC file), Arrow IPC streams and the encoding
//! shared with GeoParquet.
//!
//! # Model
//!
//! A GeoFrame table is a geometry column plus attributes (an Arrow IPC stream). Every format in
//! this module and in [`crate::parquet_io`] first builds one `RecordBatch` with
//! [`encode_table`]: the attributes, then the geometry as a GeoArrow extension column (WKB or a
//! native struct encoding), plus the GeoParquet-style `geo` metadata that GeoPandas writes next
//! to it. Reading is the inverse: [`decode_table`] finds the geometry column, decodes it to a
//! [`GeoColumn`] and returns everything else as an attribute IPC stream.
//!
//! # Decisions
//!
//! * **Geometry encodings written:** WKB (`geoarrow.wkb`, default, any geometry mix) or
//!   GeoArrow native (`geoarrow.point`, ..., `geoarrow.multipolygon`, separated coordinates).
//!   Native needs one geometry kind, like GeoPandas: `X` and `MultiX` mix and are promoted to
//!   `MultiX` (with their Z and M); anything else, including rows of different dimensions, is an
//!   error. `geometry_types` carries the dimension (`"Polygon Z"`, `"Point ZM"`, `"LineString
//!   M"`) and the `bbox` in the metadata and the covering column are 2D, like GeoPandas.
//! * **CRS:** the extension metadata carries the CRS as PROJJSON (`{"crs": {...}}`), as
//!   GeoPandas does; the `geo` metadata keeps the GeoParquet semantics (missing key =
//!   `OGC:CRS84`, explicit `null` = unknown) and wins over the extension metadata on read.
//! * **Read:** a geometry column is the one named by `geometry_column`, the `geo` metadata's
//!   primary column, the first `geoarrow.*` extension column, or a binary (WKB) / string (WKT)
//!   column named `geometry`. WKB, WKT and every native GeoArrow type decode.
//! * **No C Data Interface:** the BEAM has no Arrow C Data binding, so IPC bytes are the
//!   interchange (one serialisation and one copy per direction).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::{ArrayRef, RecordBatch, RecordBatchOptions};
use arrow_ipc::reader::{FileReader, StreamReader};
use arrow_ipc::writer::{FileWriter, IpcWriteOptions, StreamWriter};
use arrow_ipc::CompressionType;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use geo_traits::{GeometryTrait, LineStringTrait, PointTrait, PolygonTrait};
use geoarrow_array::array::from_arrow_array;
use geoarrow_array::array::GenericWkbArray;
use geoarrow_array::builder::{MultiLineStringBuilder, MultiPointBuilder, MultiPolygonBuilder};
use geoarrow_array::cast::{from_wkb, to_wkb};
use geoarrow_array::{GeoArrowArray, GeoArrowArrayAccessor, IntoArrow};
use geoarrow_schema::{
    Dimension, GeoArrowType, LineStringType, MultiLineStringType, MultiPointType, MultiPolygonType,
    PointType, PolygonType,
};
use geoparquet::writer::{
    GeoParquetRecordBatchEncoder, GeoParquetWriterEncoding, GeoParquetWriterOptionsBuilder,
};
use serde_json::{json, Value};

use crate::attributes::{decode_attributes, ipc_from_batches};
use crate::projjson::known_projjson;
use crate::GeoColumn;

pub(crate) const CRS84: &str = "OGC:CRS84";
const EXTENSION_NAME: &str = "ARROW:extension:name";
const EXTENSION_METADATA: &str = "ARROW:extension:metadata";

/// Result of reading a table: the geometry column plus everything else.
#[derive(Debug)]
pub struct TableRead {
    /// The geometry column, with the table's CRS.
    pub geometry: GeoColumn,
    /// Name of the geometry column in the table.
    pub geometry_column: String,
    /// All other (selected) columns as an Arrow IPC stream; an empty-schema stream when none.
    pub attributes_ipc: Vec<u8>,
}

/// How a geometry column is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GeometryEncoding {
    /// Well-known binary, any geometry mix. The default.
    #[default]
    Wkb,
    /// GeoArrow native struct/list encoding; needs a single geometry kind.
    GeoArrow,
}

impl GeometryEncoding {
    /// Parses `"wkb"` or `"geoarrow"` (case-insensitive).
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.to_ascii_lowercase().as_str() {
            "wkb" => Ok(Self::Wkb),
            "geoarrow" => Ok(Self::GeoArrow),
            other => Err(format!(
                "unknown geometry_encoding {other:?}; expected \"wkb\" or \"geoarrow\""
            )),
        }
    }
}

/// Compression of a Feather file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeatherCompression {
    Uncompressed,
    /// LZ4 frame, the pyarrow default.
    #[default]
    Lz4,
    Zstd,
}

impl FeatherCompression {
    /// Parses `"uncompressed"`, `"lz4"` or `"zstd"` (case-insensitive).
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.to_ascii_lowercase().as_str() {
            "uncompressed" | "none" => Ok(Self::Uncompressed),
            "lz4" => Ok(Self::Lz4),
            "zstd" => Ok(Self::Zstd),
            other => Err(format!(
                "unknown compression {other:?}; expected \"uncompressed\", \"lz4\" or \"zstd\""
            )),
        }
    }
}

// -- encode -------------------------------------------------------------------------------

/// What [`encode_table`] should produce.
pub(crate) struct EncodeOptions {
    pub encoding: GeometryEncoding,
    /// Add a `bbox` covering struct column (GeoParquet 1.1).
    pub covering_bbox: bool,
    /// GeoParquet metadata version, `"1.0.0"` or `"1.1.0"`.
    pub version: String,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            encoding: GeometryEncoding::Wkb,
            covering_bbox: false,
            version: "1.1.0".to_string(),
        }
    }
}

/// The encoded table and its `geo` metadata (already completed with the CRS).
pub(crate) struct Encoded {
    pub batch: RecordBatch,
    pub geo: Value,
}

/// Builds the table batch for `geometry` (named `name`) next to `attributes_ipc` (empty bytes
/// mean no attributes).
pub(crate) fn encode_table(
    geometry: &GeoColumn,
    name: &str,
    attributes_ipc: &[u8],
    options: &EncodeOptions,
) -> Result<Encoded, String> {
    if options.version != "1.0.0" && options.version != "1.1.0" {
        return Err(format!(
            "schema_version must be \"1.0.0\" or \"1.1.0\", got {:?}",
            options.version
        ));
    }
    if options.version == "1.0.0"
        && (options.covering_bbox || options.encoding == GeometryEncoding::GeoArrow)
    {
        return Err(
            "schema_version \"1.0.0\" supports only WKB geometry without a covering bbox column; \
             use \"1.1.0\""
                .to_string(),
        );
    }

    let (mut fields, mut columns) = if attributes_ipc.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        decode_attributes(attributes_ipc, geometry.len())?
    };
    if fields.iter().any(|f| f.name() == name) {
        return Err(format!("attributes already have a column named {name:?}"));
    }
    if options.covering_bbox && fields.iter().any(|f| f.name() == "bbox") {
        return Err(
            "write_covering_bbox adds a column named \"bbox\" but the attributes already have one"
                .to_string(),
        );
    }

    let array: Arc<dyn GeoArrowArray> = match options.encoding {
        GeometryEncoding::Wkb => Arc::new(geometry.geometry_array().clone()),
        GeometryEncoding::GeoArrow => native_array(geometry)?,
    };
    fields.push(array.data_type().to_field(name, true));
    columns.push(array.to_array_ref());
    let batch =
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).map_err(|e| e.to_string())?;

    let mut builder = GeoParquetWriterOptionsBuilder::default()
        .set_primary_column(name.to_string())
        .set_encoding(match options.encoding {
            GeometryEncoding::Wkb => GeoParquetWriterEncoding::WKB,
            GeometryEncoding::GeoArrow => GeoParquetWriterEncoding::GeoArrow,
        });
    if options.covering_bbox {
        builder = builder
            .set_column_generate_covering(name.to_string(), true)
            .set_column_covering_name(name.to_string(), "bbox".to_string());
    }
    let mut encoder = GeoParquetRecordBatchEncoder::try_new(&batch.schema(), &builder.build())
        .map_err(|e| format!("cannot prepare GeoParquet encoding: {e}"))?;
    let encoded = encoder
        .encode_record_batch(&batch)
        .map_err(|e| format!("cannot encode geometry: {e}"))?;

    let mut geo = serde_json::to_value(encoder.into_geoparquet_metadata())
        .map_err(|e| format!("cannot serialise GeoParquet metadata: {e}"))?;
    geo["version"] = Value::from(options.version.clone());
    let column = geo
        .get_mut("columns")
        .and_then(|c| c.get_mut(name))
        .and_then(Value::as_object_mut)
        .ok_or("GeoParquet metadata lost the geometry column")?;
    // The encoder knows no CRS database and drops it; set it explicitly.
    column.remove("crs");
    // An all-null or empty column has no bounds: NaN serialises as null, which no reader
    // accepts. The key is optional, so leave it out.
    if column
        .get("bbox")
        .and_then(Value::as_array)
        .is_some_and(|bbox| bbox.iter().any(|v| !v.is_number()))
    {
        column.remove("bbox");
    }
    // The crate lists Z (and M) ranges in `bbox` for Z geometries; GeoPandas writes the 2D box
    // and so do we (the covering column is 2D as well).
    if column
        .get("bbox")
        .and_then(Value::as_array)
        .is_some_and(|bbox| bbox.len() != 4)
    {
        match geometry.total_bounds() {
            Some(bounds) => column.insert("bbox".to_string(), json!(bounds)),
            None => column.remove("bbox"),
        };
    }
    let crs = crs_for_file(geometry.crs().as_deref())?;
    let extension_metadata = match &crs {
        CrsOut::Omit => json!({ "crs": CRS84, "crs_type": "authority_code" }),
        CrsOut::Null => json!({}),
        CrsOut::Projjson(value) => json!({ "crs": value }),
    };
    match crs {
        CrsOut::Omit => {}
        CrsOut::Null => {
            column.insert("crs".to_string(), Value::Null);
        }
        CrsOut::Projjson(value) => {
            column.insert("crs".to_string(), value);
        }
    }

    // The extension metadata of the geometry field carries PROJJSON, like GeoPandas.
    let fields: Vec<Field> = encoded
        .schema()
        .fields()
        .iter()
        .map(|field| {
            if field.name() == name {
                let mut metadata = field.metadata().clone();
                metadata.insert(
                    EXTENSION_METADATA.to_string(),
                    extension_metadata.to_string(),
                );
                field.as_ref().clone().with_metadata(metadata)
            } else {
                field.as_ref().clone()
            }
        })
        .collect();
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), encoded.columns().to_vec())
        .map_err(|e| e.to_string())?;
    Ok(Encoded { batch, geo })
}

/// The column as one native GeoArrow array: `X` and `MultiX` rows are promoted to `MultiX`.
fn native_array(geometry: &GeoColumn) -> Result<Arc<dyn GeoArrowArray>, String> {
    #[derive(Hash, PartialEq, Eq, Clone, Copy)]
    enum Kind {
        Point,
        Line,
        Polygon,
    }
    let wkb = to_wkb::<i32>(geometry.geometry_array()).map_err(|e| e.to_string())?;
    let mut kinds: HashSet<(Kind, bool)> = HashSet::new();
    let mut dimensions: HashSet<Dimension> = HashSet::new();
    for item in wkb.iter().flatten() {
        let item = item.map_err(|e| e.to_string())?;
        let entry =
            match item.as_type() {
                geo_traits::GeometryType::Point(_) => (Kind::Point, false),
                geo_traits::GeometryType::MultiPoint(_) => (Kind::Point, true),
                geo_traits::GeometryType::LineString(_) => (Kind::Line, false),
                geo_traits::GeometryType::MultiLineString(_) => (Kind::Line, true),
                geo_traits::GeometryType::Polygon(_) => (Kind::Polygon, false),
                geo_traits::GeometryType::MultiPolygon(_) => (Kind::Polygon, true),
                _ => return Err(
                    "geometry_encoding \"geoarrow\" supports points, lines, polygons and their \
                     multi variants; use \"wkb\" for geometry collections"
                        .to_string(),
                ),
            };
        kinds.insert(entry);
        dimensions.insert(Dimension::try_from(item.dim()).map_err(|e| e.to_string())?);
    }
    let distinct: HashSet<Kind> = kinds.iter().map(|(kind, _)| *kind).collect();
    if distinct.len() > 1 || dimensions.len() > 1 {
        return Err(
            "geometry_encoding \"geoarrow\" needs one geometry kind (a type and its multi \
             variant may mix) and one dimension; use \"wkb\" for mixed geometries"
                .to_string(),
        );
    }
    let kind = distinct.into_iter().next().unwrap_or(Kind::Point);
    let multi = kinds.iter().any(|(_, multi)| *multi);
    let dim = dimensions.into_iter().next().unwrap_or(Dimension::XY);
    let metadata = geometry
        .geometry_array()
        .extension_type()
        .metadata()
        .clone();

    let target = match (kind, multi) {
        (Kind::Point, false) => GeoArrowType::Point(PointType::new(dim, metadata)),
        (Kind::Point, true) => GeoArrowType::MultiPoint(MultiPointType::new(dim, metadata)),
        (Kind::Line, false) => GeoArrowType::LineString(LineStringType::new(dim, metadata)),
        (Kind::Line, true) => {
            GeoArrowType::MultiLineString(MultiLineStringType::new(dim, metadata))
        }
        (Kind::Polygon, false) => GeoArrowType::Polygon(PolygonType::new(dim, metadata)),
        (Kind::Polygon, true) => GeoArrowType::MultiPolygon(MultiPolygonType::new(dim, metadata)),
    };
    // geoarrow-array 0.9 mishandles validity when a single geometry is pushed into a multi
    // builder, so rows that need promotion are promoted here and pushed as multi geometries.
    if multi && kinds.iter().any(|(_, is_multi)| !*is_multi) {
        return promoted_array(&wkb, target);
    }
    from_wkb(&wkb, target).map_err(|e| format!("cannot encode geometry as GeoArrow: {e}"))
}

/// Native multi array from a column that mixes `X` and `MultiX` rows. Each single geometry is
/// wrapped into a one-member multi geometry of the same dimension (an empty one into an empty
/// multi), so Z and M survive.
fn promoted_array(
    wkb: &GenericWkbArray<i32>,
    target: GeoArrowType,
) -> Result<Arc<dyn GeoArrowArray>, String> {
    let multi_base: u32 = match &target {
        GeoArrowType::MultiPoint(_) => 4,
        GeoArrowType::MultiLineString(_) => 5,
        _ => 6,
    };
    let wrapped: Vec<Option<Vec<u8>>> = wkb
        .inner()
        .iter()
        .map(|row| {
            row.map(|bytes| wrap_in_multi(bytes, multi_base))
                .transpose()
        })
        .collect::<Result<_, _>>()?;
    let parsed = wrapped
        .iter()
        .map(|row| {
            row.as_deref()
                .map(wkb::reader::read_wkb)
                .transpose()
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let failed = |e: geoarrow_schema::error::GeoArrowError| e.to_string();
    Ok(match target {
        GeoArrowType::MultiPoint(t) => Arc::new(
            MultiPointBuilder::from_nullable_geometries(&parsed, t)
                .map_err(failed)?
                .finish(),
        ),
        GeoArrowType::MultiLineString(t) => Arc::new(
            MultiLineStringBuilder::from_nullable_geometries(&parsed, t)
                .map_err(failed)?
                .finish(),
        ),
        GeoArrowType::MultiPolygon(t) => Arc::new(
            MultiPolygonBuilder::from_nullable_geometries(&parsed, t)
                .map_err(failed)?
                .finish(),
        ),
        _ => unreachable!("promotion only targets multi types"),
    })
}

/// ISO WKB of `bytes` as the multi geometry with base type `multi_base` (4, 5 or 6): the bytes
/// themselves when already multi, an empty multi for an empty single geometry, else a multi
/// holding the geometry. The dimension code carries over.
fn wrap_in_multi(bytes: &[u8], multi_base: u32) -> Result<Vec<u8>, String> {
    let geometry = wkb::reader::read_wkb(bytes).map_err(|e| e.to_string())?;
    let empty = match geometry.as_type() {
        geo_traits::GeometryType::Point(p) => p.coord().is_none(),
        geo_traits::GeometryType::LineString(l) => l.num_coords() == 0,
        geo_traits::GeometryType::Polygon(p) => p.exterior().is_none_or(|r| r.num_coords() == 0),
        _ => return Ok(bytes.to_vec()),
    };
    let raw: [u8; 4] = bytes
        .get(1..5)
        .and_then(|b| b.try_into().ok())
        .ok_or("truncated WKB")?;
    let code = if bytes[0] == 1 {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    };
    let mut out = vec![1];
    out.extend_from_slice(&(code - code % 1000 + multi_base).to_le_bytes());
    out.extend_from_slice(&u32::from(!empty).to_le_bytes());
    if !empty {
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

// -- decode -------------------------------------------------------------------------------

/// Decodes a table: finds the geometry column, converts it and packs the other columns.
pub(crate) fn decode_table(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    columns: Option<&[String]>,
    geometry_column: Option<&str>,
) -> Result<TableRead, String> {
    let geo_meta = schema
        .metadata()
        .get("geo")
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let index = find_geometry_index(schema, geo_meta.as_ref(), geometry_column)?;
    let field = schema.field(index);
    let name = field.name().clone();

    let (rows, extension_crs) = geometry_rows(field, batches, index)?;
    let crs = match geo_meta
        .as_ref()
        .and_then(|meta| meta.get("columns")?.get(&name))
    {
        Some(column) => match column.get("crs") {
            None => Some(CRS84.to_string()),
            Some(Value::Null) => None,
            Some(value) => Some(crs_to_string(value)),
        },
        None => extension_crs,
    };
    let geometry = match rows {
        Rows::Wkb(rows) => {
            let refs: Vec<Option<&[u8]>> = rows.iter().map(|r| r.as_deref()).collect();
            GeoColumn::from_wkb(&refs, crs.as_deref())?
        }
        Rows::Wkt(rows) => GeoColumn::from_wkt(&rows, crs.as_deref())?,
    };

    let hidden = covering_columns(geo_meta.as_ref());
    let indices = select_attributes(schema, index, columns, &hidden)?;
    let attributes_ipc = attributes_to_ipc(schema, batches, &indices)?;
    Ok(TableRead {
        geometry,
        geometry_column: name,
        attributes_ipc,
    })
}

enum Rows {
    Wkb(Vec<Option<Vec<u8>>>),
    Wkt(Vec<Option<String>>),
}

fn extension_name(field: &Field) -> Option<&str> {
    field.metadata().get(EXTENSION_NAME).map(String::as_str)
}

fn find_geometry_index(
    schema: &Schema,
    geo_meta: Option<&Value>,
    requested: Option<&str>,
) -> Result<usize, String> {
    if let Some(name) = requested {
        return schema
            .index_of(name)
            .map_err(|_| format!("geometry column {name:?} not found in the table"));
    }
    if let Some(primary) = geo_meta
        .and_then(|meta| meta.get("primary_column"))
        .and_then(Value::as_str)
    {
        if let Ok(index) = schema.index_of(primary) {
            return Ok(index);
        }
    }
    if let Some(index) = schema
        .fields()
        .iter()
        .position(|f| extension_name(f).is_some_and(|n| n.starts_with("geoarrow.")))
    {
        return Ok(index);
    }
    if let Ok(index) = schema.index_of("geometry") {
        if matches!(
            schema.field(index).data_type(),
            DataType::Binary | DataType::LargeBinary | DataType::Utf8 | DataType::LargeUtf8
        ) {
            return Ok(index);
        }
    }
    Err(
        "no geometry column found: expected a GeoArrow extension column, `geo` metadata, or a \
         binary (WKB) or string (WKT) column named \"geometry\"; pass geometry_column:"
            .to_string(),
    )
}

/// The geometry rows (as WKB or WKT) and the CRS from the extension metadata, if any.
fn geometry_rows(
    field: &Field,
    batches: &[RecordBatch],
    index: usize,
) -> Result<(Rows, Option<String>), String> {
    if extension_name(field).is_some_and(|n| n.starts_with("geoarrow.")) {
        let mut rows: Vec<Option<Vec<u8>>> = Vec::new();
        let mut crs = None;
        for batch in batches {
            let array = from_arrow_array(batch.column(index), field)
                .map_err(|e| format!("invalid geometry column {:?}: {e}", field.name()))?;
            crs = array
                .data_type()
                .metadata()
                .crs()
                .crs_value()
                .map(crs_to_string);
            let wkb = to_wkb::<i32>(array.as_ref())
                .map_err(|e| format!("cannot decode geometry column {:?}: {e}", field.name()))?;
            rows.extend(wkb.inner().iter().map(|r| r.map(<[u8]>::to_vec)));
        }
        if batches.is_empty() {
            crs = extension_crs(field);
        }
        return Ok((Rows::Wkb(rows), crs));
    }
    match field.data_type() {
        DataType::Binary | DataType::LargeBinary => {
            let mut rows = Vec::new();
            for batch in batches {
                let array = batch.column(index);
                if let DataType::Binary = array.data_type() {
                    rows.extend(
                        array
                            .as_binary::<i32>()
                            .iter()
                            .map(|r| r.map(<[u8]>::to_vec)),
                    );
                } else {
                    rows.extend(
                        array
                            .as_binary::<i64>()
                            .iter()
                            .map(|r| r.map(<[u8]>::to_vec)),
                    );
                }
            }
            Ok((Rows::Wkb(rows), None))
        }
        DataType::Utf8 | DataType::LargeUtf8 => {
            let mut rows = Vec::new();
            for batch in batches {
                let array = batch.column(index);
                if let DataType::Utf8 = array.data_type() {
                    rows.extend(
                        array
                            .as_string::<i32>()
                            .iter()
                            .map(|r| r.map(str::to_string)),
                    );
                } else {
                    rows.extend(
                        array
                            .as_string::<i64>()
                            .iter()
                            .map(|r| r.map(str::to_string)),
                    );
                }
            }
            Ok((Rows::Wkt(rows), None))
        }
        other => Err(format!(
            "column {:?} has type {other}, which is not a geometry encoding",
            field.name()
        )),
    }
}

/// CRS from the extension metadata JSON of an (empty) geometry field.
fn extension_crs(field: &Field) -> Option<String> {
    let raw = field.metadata().get(EXTENSION_METADATA)?;
    let meta: Value = serde_json::from_str(raw).ok()?;
    meta.get("crs").map(crs_to_string)
}

/// Names of the covering (bbox) columns declared by the `geo` metadata.
pub(crate) fn covering_columns(geo_meta: Option<&Value>) -> Vec<String> {
    let mut hidden = Vec::new();
    let columns = geo_meta
        .and_then(|meta| meta.get("columns"))
        .and_then(Value::as_object);
    for column in columns.into_iter().flat_map(|c| c.values()) {
        let xmin = column
            .get("covering")
            .and_then(|c| c.get("bbox"))
            .and_then(|b| b.get("xmin"))
            .and_then(|p| p.get(0))
            .and_then(Value::as_str);
        if let Some(name) = xmin {
            hidden.push(name.to_string());
        }
    }
    hidden
}

/// Indices of the attribute columns to keep, in table order.
pub(crate) fn select_attributes(
    schema: &Schema,
    geometry_index: usize,
    columns: Option<&[String]>,
    hidden: &[String],
) -> Result<Vec<usize>, String> {
    if let Some(wanted) = columns {
        for name in wanted {
            match schema.index_of(name) {
                Err(_) => return Err(format!("column {name:?} not found in file")),
                Ok(i) if i == geometry_index => {
                    return Err(format!("{name:?} is the geometry column, not an attribute"))
                }
                Ok(_) => {}
            }
        }
    }
    Ok((0..schema.fields().len())
        .filter(|&i| i != geometry_index)
        .filter(|&i| !hidden.contains(schema.field(i).name()))
        .filter(|&i| columns.is_none_or(|w| w.contains(schema.field(i).name())))
        .collect())
}

/// Packs the selected columns of `batches` as an attribute IPC stream.
pub(crate) fn attributes_to_ipc(
    schema: &Schema,
    batches: &[RecordBatch],
    indices: &[usize],
) -> Result<Vec<u8>, String> {
    // Drop table-level metadata (the raw `geo` JSON, the embedded Arrow schema): only the
    // fields matter to the consumer.
    let fields: Vec<Field> = indices
        .iter()
        .map(|&i| schema.field(i).as_ref().clone())
        .collect();
    let projected = Arc::new(Schema::new(fields));
    let projected_batches = batches
        .iter()
        .map(|batch| {
            let columns: Vec<ArrayRef> = indices.iter().map(|&i| batch.column(i).clone()).collect();
            let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
            RecordBatch::try_new_with_options(projected.clone(), columns, &options)
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    ipc_from_batches(&projected, &projected_batches)
}

// -- Feather and IPC stream ---------------------------------------------------------------

fn with_geo_metadata(batch: &RecordBatch, geo: &Value) -> Result<RecordBatch, String> {
    let mut metadata: HashMap<String, String> = batch.schema().metadata().clone();
    metadata.insert("geo".to_string(), geo.to_string());
    let schema = Arc::new(batch.schema().as_ref().clone().with_metadata(metadata));
    RecordBatch::try_new(schema, batch.columns().to_vec()).map_err(|e| e.to_string())
}

/// Writes a Feather v2 (Arrow IPC file). The geometry column is appended after the
/// attributes under `geometry_column`; the schema carries GeoPandas-style `geo` metadata.
pub fn write_feather(
    path: &str,
    geometry: &GeoColumn,
    geometry_column: &str,
    attributes_ipc: &[u8],
    encoding: GeometryEncoding,
    compression: FeatherCompression,
) -> Result<(), String> {
    let options = EncodeOptions {
        encoding,
        ..EncodeOptions::default()
    };
    let encoded = encode_table(geometry, geometry_column, attributes_ipc, &options)?;
    let batch = with_geo_metadata(&encoded.batch, &encoded.geo)?;

    let write_options = match compression {
        FeatherCompression::Uncompressed => IpcWriteOptions::default(),
        FeatherCompression::Lz4 => IpcWriteOptions::default()
            .try_with_compression(Some(CompressionType::LZ4_FRAME))
            .map_err(|e| e.to_string())?,
        FeatherCompression::Zstd => IpcWriteOptions::default()
            .try_with_compression(Some(CompressionType::ZSTD))
            .map_err(|e| e.to_string())?,
    };
    crate::atomic::write_atomic(path, |file| {
        let mut writer = FileWriter::try_new_with_options(file, &batch.schema(), write_options)
            .map_err(|e| format!("cannot write {path}: {e}"))?;
        writer
            .write(&batch)
            .map_err(|e| format!("cannot write {path}: {e}"))?;
        writer
            .finish()
            .map_err(|e| format!("cannot write {path}: {e}"))
    })
}

/// Reads a Feather v2 file written by GeoFrame or GeoPandas `to_feather`.
pub fn read_feather(
    path: &str,
    columns: Option<&[String]>,
    geometry_column: Option<&str>,
) -> Result<TableRead, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let reader = FileReader::try_new(file, None)
        .map_err(|e| format!("cannot read {path} as Feather: {e}"))?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    decode_table(&schema, &batches, columns, geometry_column).map_err(|e| format!("{path}: {e}"))
}

/// Encodes the table as an Arrow IPC **stream** (uncompressed) with GeoArrow extension
/// metadata on the geometry field.
pub fn to_arrow_ipc(
    geometry: &GeoColumn,
    geometry_column: &str,
    attributes_ipc: &[u8],
    encoding: GeometryEncoding,
) -> Result<Vec<u8>, String> {
    let options = EncodeOptions {
        encoding,
        ..EncodeOptions::default()
    };
    let encoded = encode_table(geometry, geometry_column, attributes_ipc, &options)?;
    let batch = with_geo_metadata(&encoded.batch, &encoded.geo)?;
    let mut out = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new(&mut out, &batch.schema()).map_err(|e| e.to_string())?;
        writer.write(&batch).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Decodes an Arrow IPC stream holding a geometry column (WKB, WKT or native GeoArrow).
pub fn from_arrow_ipc(
    bytes: &[u8],
    columns: Option<&[String]>,
    geometry_column: Option<&str>,
) -> Result<TableRead, String> {
    let reader =
        StreamReader::try_new(bytes, None).map_err(|e| format!("invalid Arrow IPC data: {e}"))?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid Arrow IPC data: {e}"))?;
    decode_table(&schema, &batches, columns, geometry_column)
}

// -- CRS ----------------------------------------------------------------------------------

/// `AUTHORITY:CODE` for PROJJSON with an identifier, plain strings as given, else JSON text.
pub(crate) fn crs_to_string(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let id = value
        .get("id")
        .or_else(|| value.get("ids").and_then(|ids| ids.get(0)));
    if let Some(id) = id {
        let authority = id.get("authority").and_then(Value::as_str);
        let code = match id.get("code") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        if let (Some(authority), Some(code)) = (authority, code) {
            return format!("{authority}:{code}");
        }
    }
    value.to_string()
}

/// How the CRS appears in the `geo` metadata.
pub(crate) enum CrsOut {
    /// Key omitted: the GeoParquet default, `OGC:CRS84`.
    Omit,
    /// Explicit `null`: unknown CRS.
    Null,
    Projjson(Value),
}

pub(crate) fn crs_for_file(crs: Option<&str>) -> Result<CrsOut, String> {
    let Some(text) = crs else {
        return Ok(CrsOut::Null);
    };
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(CRS84)
        || trimmed.eq_ignore_ascii_case("urn:ogc:def:crs:OGC:1.3:CRS84")
    {
        return Ok(CrsOut::Omit);
    }
    if trimmed.starts_with('{') {
        return serde_json::from_str::<Value>(trimmed)
            .ok()
            .filter(Value::is_object)
            .map(CrsOut::Projjson)
            .ok_or_else(|| "CRS looks like PROJJSON but is not a valid JSON object".to_string());
    }
    if let Some((authority, code)) = trimmed.split_once(':') {
        if let Some(full) = code
            .parse::<u64>()
            .ok()
            .and_then(|code| known_projjson(authority, code))
        {
            return Ok(CrsOut::Projjson(full));
        }
        let code = code
            .parse::<u64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::from(code));
        return Ok(CrsOut::Projjson(json!({
            "name": trimmed,
            "id": { "authority": authority, "code": code },
        })));
    }
    Err(format!(
        "CRS {text:?} cannot be written to GeoParquet; use an AUTHORITY:CODE string or PROJJSON"
    ))
}
