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
//! * **Written:** GeoParquet 1.1.0 with `WKB` geometry encoding (the most portable one) and
//!   Snappy compression by default; [`ParquetOptions`] selects the compression (and level), the
//!   row group size, GeoArrow-native geometry, a `bbox` covering column and the metadata
//!   version (`1.0.0` or `1.1.0`). **Read:** WKB and every GeoArrow-native encoding.
//! * **CRS, written:** `OGC:CRS84` is the GeoParquet default, so the `crs` key is omitted. An
//!   unknown CRS is written as an explicit `"crs": null`. `EPSG:4326`, `EPSG:4269`,
//!   `EPSG:3857` and the WGS 84 UTM zones get a full PROJJSON definition (`projjson.rs`, so
//!   GeoPandas/pyproj can parse the file); any other `AUTHORITY:CODE` string is written as a
//!   PROJJSON stub carrying only `name` and `id` (there is no PROJ database in the pure-Rust
//!   build to expand it).
//!   A PROJJSON object given as the CRS is written as is.
//! * **CRS, read (GeoParquet 1.1 semantics, as GeoPolars `dev`):** a missing `crs` key means
//!   `OGC:CRS84`; an explicit `null` means unknown. A PROJJSON object that carries an `id`
//!   (or `ids[0]`) is normalised to its `AUTHORITY:CODE` string; one without an identifier is
//!   kept as compact PROJJSON text.
//!
//! # Gaps
//!
//! The whole file is read into memory (no batched or streaming reads), column order is not
//! preserved relative to the geometry (it becomes the last column) and only the primary
//! geometry column is decoded.
//!
//! # Bounding box reads
//!
//! `read_geoparquet_filtered` keeps the rows whose bounding box intersects a query box. When
//! the file has a covering column (or a native geometry encoding) the `geoparquet` crate prunes
//! row groups from the column statistics and filters rows on the covering values; otherwise the
//! geometries are decoded and filtered after the read. The covering column is never an
//! attribute.

use std::collections::HashSet;
use std::fs::File;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use arrow_array::{BooleanArray, RecordBatch};
use arrow_schema::Schema;
use arrow_select::filter::filter_record_batch;
use geo::{coord, Rect};
use geoarrow_array::array::from_arrow_array;
use geoarrow_array::cast::to_wkb;
use geoarrow_schema::{CoordType, Dimension};
use geoparquet::metadata::{
    GeoParquetColumnEncoding, GeoParquetGeometryType, GeoParquetGeometryTypeAndDimension,
    GeoParquetMetadata,
};
use geoparquet::reader::{GeoParquetReaderBuilder, GeoParquetRecordBatchReader};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::{BrotliLevel, Compression, GzipLevel, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use serde_json::Value;

use crate::arrow_io::{
    attributes_to_ipc, covering_columns, crs_to_string, encode_table, select_attributes,
    EncodeOptions, CRS84,
};
use crate::{GeoColumn, GeometryEncoding};

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

/// GeoParquet write options. The default reproduces the historical output: GeoParquet 1.1.0,
/// WKB geometry, Snappy, no covering column, the library's default row group size.
#[derive(Debug, Clone)]
pub struct ParquetOptions {
    /// `snappy`, `zstd`, `gzip`, `lz4`, `brotli` or `none` (case-insensitive).
    pub compression: String,
    /// Level for `zstd` (1..=22), `gzip` (0..=10) and `brotli` (0..=11); rejected otherwise.
    pub compression_level: Option<i32>,
    /// Maximum rows per row group.
    pub row_group_size: Option<usize>,
    pub geometry_encoding: GeometryEncoding,
    /// Add a `bbox` covering struct column and the `covering` metadata (GeoParquet 1.1).
    pub covering_bbox: bool,
    /// `"1.0.0"` (WKB only, no covering) or `"1.1.0"`.
    pub schema_version: String,
}

impl Default for ParquetOptions {
    fn default() -> Self {
        Self {
            compression: "snappy".to_string(),
            compression_level: None,
            row_group_size: None,
            geometry_encoding: GeometryEncoding::Wkb,
            covering_bbox: false,
            schema_version: "1.1.0".to_string(),
        }
    }
}

impl ParquetOptions {
    fn parquet_compression(&self) -> Result<Compression, String> {
        let name = self.compression.to_ascii_lowercase();
        let level = self.compression_level;
        let no_level = |compression: Compression| match level {
            None => Ok(compression),
            Some(_) => Err(format!(
                "compression_level is not supported for compression {name:?}"
            )),
        };
        let bad_level =
            |detail: String| format!("invalid compression_level for {name:?}: {detail}");
        match name.as_str() {
            "snappy" => no_level(Compression::SNAPPY),
            "none" | "uncompressed" => no_level(Compression::UNCOMPRESSED),
            "lz4" => no_level(Compression::LZ4_RAW),
            "zstd" => match level {
                None => Ok(Compression::ZSTD(ZstdLevel::default())),
                Some(l) => ZstdLevel::try_new(l)
                    .map(Compression::ZSTD)
                    .map_err(|e| bad_level(e.to_string())),
            },
            "gzip" => match level {
                None => Ok(Compression::GZIP(GzipLevel::default())),
                Some(l) => u32::try_from(l)
                    .map_err(|e| e.to_string())
                    .and_then(|l| GzipLevel::try_new(l).map_err(|e| e.to_string()))
                    .map(Compression::GZIP)
                    .map_err(bad_level),
            },
            "brotli" => match level {
                None => Ok(Compression::BROTLI(BrotliLevel::default())),
                Some(l) => u32::try_from(l)
                    .map_err(|e| e.to_string())
                    .and_then(|l| BrotliLevel::try_new(l).map_err(|e| e.to_string()))
                    .map(Compression::BROTLI)
                    .map_err(bad_level),
            },
            other => Err(format!(
                "unknown compression {other:?}; expected snappy, zstd, gzip, lz4, brotli or none"
            )),
        }
    }

    fn writer_properties(&self) -> Result<WriterProperties, String> {
        if self.row_group_size == Some(0) {
            return Err("row_group_size must be a positive number of rows".to_string());
        }
        Ok(WriterProperties::builder()
            .set_compression(self.parquet_compression()?)
            .set_max_row_group_row_count(self.row_group_size)
            .build())
    }
}

// -- read ---------------------------------------------------------------------------------

/// Reads a GeoParquet file. `columns` optionally restricts the attribute columns (the
/// primary geometry column is always read); selected columns keep their file order.
pub fn read_geoparquet(path: &str, columns: Option<&[String]>) -> Result<GeoParquetRead, String> {
    read_geoparquet_filtered(path, columns, None)
}

/// Like [`read_geoparquet`], keeping only the rows whose geometry bounding box intersects
/// `bbox` (`[minx, miny, maxx, maxy]`, in the file's CRS; touching counts as intersecting).
pub fn read_geoparquet_filtered(
    path: &str,
    columns: Option<&[String]>,
    bbox: Option<[f64; 4]>,
) -> Result<GeoParquetRead, String> {
    // The `geoparquet` crate asserts on some inputs; a panic must not take the VM's NIF down.
    catch_unwind(AssertUnwindSafe(|| read_filtered(path, columns, bbox))).unwrap_or_else(|_| {
        Err(format!(
            "cannot read {path}: the GeoParquet reader failed on this file's metadata"
        ))
    })
}

fn read_filtered(
    path: &str,
    columns: Option<&[String]>,
    bbox: Option<[f64; 4]>,
) -> Result<GeoParquetRead, String> {
    if let Some([minx, miny, maxx, maxy]) = bbox {
        if !(minx <= maxx && miny <= maxy) {
            return Err("bbox minimum exceeds its maximum (or contains NaN)".to_string());
        }
    }
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| format!("cannot read {path} as Parquet: {e}"))?;

    let mut geo_meta = builder
        .geoparquet_metadata()
        .ok_or_else(|| {
            format!("{path} is not a GeoParquet file: the Parquet metadata has no `geo` key")
        })?
        .map_err(|e| format!("invalid GeoParquet metadata in {path}: {e}"))?;
    fix_native_geometry_types(&mut geo_meta);
    let primary = geo_meta.primary_column.clone();
    let raw = raw_geo_metadata(&builder)?;
    let crs = file_crs(&raw, &primary)?;
    let hidden = covering_columns(Some(&raw));

    let target_schema = builder
        .geoarrow_schema(&geo_meta, false, CoordType::default())
        .map_err(|e| format!("cannot interpret geometry columns of {path}: {e}"))?;
    let geometry_index = target_schema.index_of(&primary).map_err(|_| {
        format!("primary geometry column {primary:?} is missing from the schema of {path}")
    })?;
    let attribute_indices = select_attributes(&target_schema, geometry_index, columns, &hidden)?;

    // A covering column (or a native encoding) lets the crate prune row groups from the
    // statistics and filter rows on the covering values; otherwise filter after decoding.
    let pushed_down = bbox.is_some() && has_covering_or_native(&raw, &primary);
    if let (Some([minx, miny, maxx, maxy]), true) = (bbox, pushed_down) {
        let rect = Rect::new(coord! { x: minx, y: miny }, coord! { x: maxx, y: maxy });
        builder = builder
            .with_intersecting_row_groups(rect, &geo_meta, None)
            .map_err(|e| format!("cannot filter {path} by bbox: {e}"))?;
        builder = builder
            .with_intersecting_row_filter(rect, &geo_meta, None)
            .map_err(|e| format!("cannot filter {path} by bbox: {e}"))?;
    }

    let reader = builder
        .with_batch_size(BATCH_SIZE)
        .build()
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let reader = GeoParquetRecordBatchReader::try_new(reader, target_schema.clone())
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut batches = reader
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
    let mut geometry = GeoColumn::from_wkb(&rows, crs.as_deref())?;

    if let (Some(query), false) = (bbox, pushed_down) {
        let keep: Vec<bool> = geometry
            .bounds()
            .into_iter()
            .map(|b| {
                b.is_some_and(|b| {
                    b[0] <= query[2] && b[2] >= query[0] && b[1] <= query[3] && b[3] >= query[1]
                })
            })
            .collect();
        if keep.iter().any(|k| !k) {
            let indices: Vec<usize> = (0..keep.len()).filter(|&i| keep[i]).collect();
            geometry = geometry.take(&indices)?;
            let mut offset = 0;
            for batch in &mut batches {
                let mask = BooleanArray::from(keep[offset..offset + batch.num_rows()].to_vec());
                offset += batch.num_rows();
                *batch = filter_record_batch(batch, &mask).map_err(|e| e.to_string())?;
            }
        }
    }

    let attributes_ipc = attributes_to_ipc(&target_schema, &batches, &attribute_indices)?;
    Ok(GeoParquetRead {
        geometry,
        geometry_column: primary,
        attributes_ipc,
    })
}

/// GeoPandas lists `["Polygon", "MultiPolygon"]` for a `multipolygon` column (the spec asks
/// for exactly the stored types), but the `geoparquet` crate asserts a single type for native
/// encodings. The encoding alone decides the stored type, so state just that.
fn fix_native_geometry_types(meta: &mut GeoParquetMetadata) {
    for column in meta.columns.values_mut() {
        let geometry_type = match column.encoding {
            GeoParquetColumnEncoding::WKB => continue,
            GeoParquetColumnEncoding::Point => GeoParquetGeometryType::Point,
            GeoParquetColumnEncoding::LineString => GeoParquetGeometryType::LineString,
            GeoParquetColumnEncoding::Polygon => GeoParquetGeometryType::Polygon,
            GeoParquetColumnEncoding::MultiPoint => GeoParquetGeometryType::MultiPoint,
            GeoParquetColumnEncoding::MultiLineString => GeoParquetGeometryType::MultiLineString,
            GeoParquetColumnEncoding::MultiPolygon => GeoParquetGeometryType::MultiPolygon,
        };
        let dimension = column
            .geometry_types
            .iter()
            .map(|t| t.dimension())
            .next()
            .unwrap_or(Dimension::XY);
        column.geometry_types = HashSet::from([GeoParquetGeometryTypeAndDimension::new(
            geometry_type,
            dimension,
        )]);
    }
}

/// The raw `geo` JSON of the file.
fn raw_geo_metadata(builder: &ParquetRecordBatchReaderBuilder<File>) -> Result<Value, String> {
    let raw = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .and_then(|kv| kv.iter().find(|kv| kv.key == "geo"))
        .and_then(|kv| kv.value.as_deref())
        .ok_or("missing `geo` metadata value")?;
    serde_json::from_str(raw).map_err(|e| format!("invalid `geo` JSON: {e}"))
}

/// Whether the primary column has a covering declaration or a native (non-WKB) encoding.
fn has_covering_or_native(raw: &Value, primary: &str) -> bool {
    let Some(column) = raw.get("columns").and_then(|c| c.get(primary)) else {
        return false;
    };
    column.get("covering").is_some()
        || column
            .get("encoding")
            .and_then(Value::as_str)
            .is_some_and(|e| !e.eq_ignore_ascii_case("WKB"))
}

/// The CRS of the primary column as our string form, applying GeoParquet 1.1 semantics to
/// the raw `geo` JSON (the typed metadata cannot tell a missing key from an explicit null).
fn file_crs(json: &Value, primary: &str) -> Result<Option<String>, String> {
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

// -- write --------------------------------------------------------------------------------

/// Writes `attributes_ipc` (an Arrow IPC stream) plus `geometry` as GeoParquet 1.1 with WKB
/// encoding and Snappy compression. The geometry column is appended after the attributes
/// under `geometry_column`.
pub fn write_geoparquet(
    path: &str,
    geometry: &GeoColumn,
    geometry_column: &str,
    attributes_ipc: &[u8],
) -> Result<(), String> {
    write_geoparquet_with(
        path,
        geometry,
        geometry_column,
        attributes_ipc,
        &ParquetOptions::default(),
    )
}

/// Like [`write_geoparquet`] with explicit [`ParquetOptions`]. Invalid options fail before the
/// file is created; a failure while writing removes the partial file.
pub fn write_geoparquet_with(
    path: &str,
    geometry: &GeoColumn,
    geometry_column: &str,
    attributes_ipc: &[u8],
    options: &ParquetOptions,
) -> Result<(), String> {
    let props = options.writer_properties()?;
    let encode = EncodeOptions {
        encoding: options.geometry_encoding,
        covering_bbox: options.covering_bbox,
        version: options.schema_version.clone(),
    };
    let encoded = encode_table(geometry, geometry_column, attributes_ipc, &encode)?;
    crate::atomic::write_atomic(path, |file| {
        write_batch(path, file, &encoded.batch, &encoded.geo, props)
    })
}

fn write_batch(
    path: &str,
    file: File,
    batch: &RecordBatch,
    geo: &Value,
    props: WriterProperties,
) -> Result<(), String> {
    let schema: Arc<Schema> = batch.schema();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|e| format!("cannot write {path}: {e}"))?;
    writer
        .write(batch)
        .map_err(|e| format!("cannot write {path}: {e}"))?;
    writer.append_key_value_metadata(KeyValue::new("geo".to_string(), geo.to_string()));
    writer
        .close()
        .map_err(|e| format!("cannot write {path}: {e}"))?;
    Ok(())
}
