//! GeoParquet read and write.
//!
//! # Data movement
//!
//! A GeoParquet file is split into two halves that live in different places on the Elixir
//! side: the primary geometry column becomes a [`GeoColumn`] (kept in Rust memory), and every
//! other column travels as an **Arrow IPC stream** of plain bytes, which Explorer loads with
//! `Explorer.DataFrame.load_ipc_stream/2`. Explorer has no zero-copy NIF access to Polars
//! memory, so this costs one serialisation plus one copy per direction; that is acceptable by
//! design for the spike. Writing is the inverse: Explorer dumps its attributes to an IPC
//! stream, and Rust combines them with the geometry column into a Parquet file.
//!
//! # Encoding and CRS decisions
//!
//! * **Written:** GeoParquet 1.1.0 with `WKB` geometry encoding (the most portable one), no
//!   covering columns, Snappy compression. **Read:** WKB and every GeoArrow-native encoding.
//! * **CRS, written:** `OGC:CRS84` is the GeoParquet default, so the `crs` key is omitted. An
//!   unknown CRS is written as an explicit `"crs": null`. `EPSG:4326` gets a full PROJJSON
//!   definition; any other `AUTHORITY:CODE` string is written as a PROJJSON stub carrying
//!   only `name` and `id` (there is no PROJ database in the pure-Rust build to expand it).
//!   A PROJJSON object given as the CRS is written as is.
//! * **CRS, read (GeoParquet 1.1 semantics, as GeoPolars `dev`):** a missing `crs` key means
//!   `OGC:CRS84`; an explicit `null` means unknown. A PROJJSON object that carries an `id`
//!   (or `ids[0]`) is normalised to its `AUTHORITY:CODE` string; one without an identifier is
//!   kept as compact PROJJSON text.
//!
//! # Gaps
//!
//! The whole file is read into memory (no batched or streaming reads, no spatial filter),
//! column order is not preserved relative to the geometry (it becomes the last column),
//! only the primary geometry column is decoded, and covering columns are not generated.

use std::fs::File;
use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, RecordBatchOptions};
use arrow_schema::{Field, Schema};
use geoarrow_array::array::from_arrow_array;
use geoarrow_array::cast::to_wkb;
use geoarrow_array::{GeoArrowArray, IntoArrow};
use geoarrow_schema::CoordType;
use geoparquet::reader::{GeoParquetReaderBuilder, GeoParquetRecordBatchReader};
use geoparquet::writer::{
    GeoParquetRecordBatchEncoder, GeoParquetWriterEncoding, GeoParquetWriterOptionsBuilder,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use serde_json::{json, Value};

use crate::attributes::{decode_attributes, ipc_from_batches};
use crate::GeoColumn;

const CRS84: &str = "OGC:CRS84";
const BATCH_SIZE: usize = 65_536;

/// Result of reading a GeoParquet file.
pub struct GeoParquetRead {
    /// The primary geometry column, with the file's CRS.
    pub geometry: GeoColumn,
    /// Name of the primary geometry column in the file.
    pub geometry_column: String,
    /// All other (selected) columns as an Arrow IPC stream; an empty-schema stream when none.
    pub attributes_ipc: Vec<u8>,
}

// -- read ---------------------------------------------------------------------------------

/// Reads a GeoParquet file. `columns` optionally restricts the attribute columns (the
/// primary geometry column is always read); selected columns keep their file order.
pub fn read_geoparquet(path: &str, columns: Option<&[String]>) -> Result<GeoParquetRead, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| format!("cannot read {path} as Parquet: {e}"))?;

    let geo_meta = builder
        .geoparquet_metadata()
        .ok_or_else(|| {
            format!("{path} is not a GeoParquet file: the Parquet metadata has no `geo` key")
        })?
        .map_err(|e| format!("invalid GeoParquet metadata in {path}: {e}"))?;
    let primary = geo_meta.primary_column.clone();
    let crs = file_crs(&builder, &primary)?;

    let target_schema = builder
        .geoarrow_schema(&geo_meta, false, CoordType::default())
        .map_err(|e| format!("cannot interpret geometry columns of {path}: {e}"))?;
    let geometry_index = target_schema.index_of(&primary).map_err(|_| {
        format!("primary geometry column {primary:?} is missing from the schema of {path}")
    })?;
    let attribute_indices = select_attributes(&target_schema, geometry_index, columns)?;

    let reader = builder
        .with_batch_size(BATCH_SIZE)
        .build()
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let reader = GeoParquetRecordBatchReader::try_new(reader, target_schema.clone())
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("cannot read {path}: {e}"))?;

    let field = target_schema.field(geometry_index);
    let mut wkb_arrays = Vec::with_capacity(batches.len());
    for batch in &batches {
        let array = from_arrow_array(batch.column(geometry_index), field)
            .map_err(|e| format!("invalid geometry column in {path}: {e}"))?;
        wkb_arrays.push(
            to_wkb::<i32>(array.as_ref())
                .map_err(|e| format!("cannot decode geometry column in {path}: {e}"))?,
        );
    }
    let rows: Vec<Option<&[u8]>> = wkb_arrays
        .iter()
        .flat_map(|wkb| wkb.inner().iter())
        .collect();
    let geometry = GeoColumn::from_wkb(&rows, crs.as_deref())?;

    let attributes_ipc = attributes_to_ipc(&target_schema, &batches, &attribute_indices)?;
    Ok(GeoParquetRead {
        geometry,
        geometry_column: primary,
        attributes_ipc,
    })
}

/// Indices of the attribute columns to keep, in file order.
fn select_attributes(
    schema: &Schema,
    geometry_index: usize,
    columns: Option<&[String]>,
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
        .filter(|&i| columns.is_none_or(|w| w.contains(schema.field(i).name())))
        .collect())
}

fn attributes_to_ipc(
    schema: &Schema,
    batches: &[RecordBatch],
    indices: &[usize],
) -> Result<Vec<u8>, String> {
    // Drop file-level metadata (the raw `geo` JSON, the embedded Arrow schema): only the
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

/// The CRS of the primary column as our string form, applying GeoParquet 1.1 semantics to
/// the raw `geo` JSON (the typed metadata cannot tell a missing key from an explicit null).
fn file_crs(
    builder: &ParquetRecordBatchReaderBuilder<File>,
    primary: &str,
) -> Result<Option<String>, String> {
    let raw = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .and_then(|kv| kv.iter().find(|kv| kv.key == "geo"))
        .and_then(|kv| kv.value.as_deref())
        .ok_or("missing `geo` metadata value")?;
    let json: Value = serde_json::from_str(raw).map_err(|e| format!("invalid `geo` JSON: {e}"))?;
    let column = json
        .get("columns")
        .and_then(|c| c.get(primary))
        .ok_or_else(|| format!("`geo` metadata has no entry for column {primary:?}"))?;
    Ok(match column.get("crs") {
        None => Some(CRS84.to_string()),
        Some(Value::Null) => None,
        Some(value) => Some(crs_to_string(value)),
    })
}

/// `AUTHORITY:CODE` for PROJJSON with an identifier, plain strings as given, else JSON text.
fn crs_to_string(value: &Value) -> String {
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

// -- write --------------------------------------------------------------------------------

/// Writes `attributes_ipc` (an Arrow IPC stream) plus `geometry` as GeoParquet 1.1 with WKB
/// encoding. The geometry column is appended after the attributes under `geometry_column`.
pub fn write_geoparquet(
    path: &str,
    geometry: &GeoColumn,
    geometry_column: &str,
    attributes_ipc: &[u8],
) -> Result<(), String> {
    let batch = combine(geometry, geometry_column, attributes_ipc)?;
    let crs = crs_for_file(geometry.crs().as_deref())?;
    let result = write_batch(path, &batch, geometry_column, crs);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn combine(
    geometry: &GeoColumn,
    geometry_column: &str,
    attributes_ipc: &[u8],
) -> Result<RecordBatch, String> {
    let (mut fields, mut columns) = decode_attributes(attributes_ipc, geometry.len())?;
    if fields.iter().any(|f| f.name() == geometry_column) {
        return Err(format!(
            "attributes already have a column named {geometry_column:?}"
        ));
    }
    let array = geometry.geometry_array();
    fields.push(array.extension_type().to_field(geometry_column, true));
    columns.push(array.to_array_ref());
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).map_err(|e| e.to_string())
}

fn write_batch(
    path: &str,
    batch: &RecordBatch,
    geometry_column: &str,
    crs: CrsOut,
) -> Result<(), String> {
    let options = GeoParquetWriterOptionsBuilder::default()
        .set_primary_column(geometry_column.to_string())
        .set_encoding(GeoParquetWriterEncoding::WKB)
        .build();
    let mut encoder = GeoParquetRecordBatchEncoder::try_new(&batch.schema(), &options)
        .map_err(|e| format!("cannot prepare GeoParquet encoding: {e}"))?;
    let encoded = encoder
        .encode_record_batch(batch)
        .map_err(|e| format!("cannot encode geometry as WKB: {e}"))?;

    let mut geo = serde_json::to_value(encoder.into_geoparquet_metadata())
        .map_err(|e| format!("cannot serialise GeoParquet metadata: {e}"))?;
    let column = geo
        .get_mut("columns")
        .and_then(|c| c.get_mut(geometry_column))
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
    match crs {
        CrsOut::Omit => {}
        CrsOut::Null => {
            column.insert("crs".to_string(), Value::Null);
        }
        CrsOut::Projjson(value) => {
            column.insert("crs".to_string(), value);
        }
    }

    let file = File::create(path).map_err(|e| format!("cannot create {path}: {e}"))?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, encoded.schema(), Some(props))
        .map_err(|e| format!("cannot write {path}: {e}"))?;
    writer
        .write(&encoded)
        .map_err(|e| format!("cannot write {path}: {e}"))?;
    writer.append_key_value_metadata(KeyValue::new("geo".to_string(), geo.to_string()));
    writer
        .close()
        .map_err(|e| format!("cannot write {path}: {e}"))?;
    Ok(())
}

/// How the CRS appears in the `geo` metadata.
enum CrsOut {
    /// Key omitted: the GeoParquet default, `OGC:CRS84`.
    Omit,
    /// Explicit `null`: unknown CRS.
    Null,
    Projjson(Value),
}

fn crs_for_file(crs: Option<&str>) -> Result<CrsOut, String> {
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
    if trimmed.eq_ignore_ascii_case("EPSG:4326") {
        return Ok(CrsOut::Projjson(wgs84_projjson()));
    }
    if let Some((authority, code)) = trimmed.split_once(':') {
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

fn wgs84_projjson() -> Value {
    json!({
        "$schema": "https://proj.org/schemas/v0.7/projjson.schema.json",
        "type": "GeographicCRS",
        "name": "WGS 84",
        "datum": {
            "type": "GeodeticReferenceFrame",
            "name": "World Geodetic System 1984",
            "ellipsoid": {
                "name": "WGS 84",
                "semi_major_axis": 6378137,
                "inverse_flattening": 298.257223563
            }
        },
        "coordinate_system": {
            "subtype": "ellipsoidal",
            "axis": [
                { "name": "Geodetic latitude", "abbreviation": "Lat", "direction": "north", "unit": "degree" },
                { "name": "Geodetic longitude", "abbreviation": "Lon", "direction": "east", "unit": "degree" }
            ]
        },
        "id": { "authority": "EPSG", "code": 4326 }
    })
}
