//! FlatGeobuf read and write, on the pure-Rust `flatgeobuf` crate.
//!
//! # Column types
//!
//! | FlatGeobuf                       | Arrow (read)   | Arrow (write)                  |
//! |----------------------------------|----------------|--------------------------------|
//! | Byte / UByte                     | Int8 / UInt8   | Int8 / UInt8                   |
//! | Short / UShort                   | Int16 / UInt16 | Int16 / UInt16                 |
//! | Int / UInt                       | Int32 / UInt32 | Int32 / UInt32                 |
//! | Long / ULong                     | Int64 / UInt64 | Int64 / UInt64                 |
//! | Float / Double                   | Float32 / 64   | Float32 / Float64              |
//! | Bool                             | Boolean        | Boolean                        |
//! | String, Json, DateTime           | Utf8           | Utf8                           |
//! | Binary                           | Binary         | Binary                         |
//!
//! Dates, times, timestamps, decimals and categoricals are written as text (`DateTime`
//! is ISO 8601 text in FlatGeobuf), so they read back as `Utf8`. Nested columns are rejected.
//!
//! # Geometry
//!
//! The header geometry type is the common type of all rows, or `Unknown` for a mix, so
//! polygons are never silently promoted to multipolygons. Coordinates are 2D. Null rows and
//! geometries `geo` cannot represent (such as empty points) are stored as features without
//! geometry (the encoder writes them as empty geometries, which read back as null). Curve geometries are rejected on read.
//!
//! # Index
//!
//! With `index` the file gets a packed Hilbert R-tree (the format's default node size) and
//! **features are stored in Hilbert order, so row order is not preserved** (GDAL behaves the
//! same). Without it the row order is kept. A bounding-box read needs the index.
//!
//! # CRS
//!
//! Read: `org:code` (organisation defaults to `EPSG`), `org:code_string` for textual codes,
//! the WKT text when only WKT is given, `None` when the header has no CRS. Write: `EPSG:n`
//! and any `AUTHORITY:CODE` map to header fields; `OGC:CRS84` is stored as `EPSG:4326`,
//! the code every reader understands, so it reads back as `EPSG:4326`; `None` writes no
//! CRS; WKT text is stored as WKT. Anything else (such as PROJJSON) is rejected.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, Int8Array, StringArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, Field};
use flatgeobuf::{
    ColumnType, FallibleStreamingIterator, FeatureProperties, FgbCrs, FgbFeature, FgbReader,
    FgbWriter, FgbWriterOptions, GeometryType, Header,
};
use geo::Geometry;
use geozero::error::Result as GeozeroResult;
use geozero::{ColumnValue, GeomProcessor, GeozeroGeometry, PropertyProcessor};

use crate::attributes::{ipc_from_columns, Cell, Table};
use crate::column::ColumnBuilder;
use crate::{GeoColumn, VectorRead};

// -- read ---------------------------------------------------------------------------------

/// Reads a FlatGeobuf file, optionally only the features whose bounding box intersects
/// `bbox` (`[minx, miny, maxx, maxy]`), found through the file's spatial index.
pub fn read_flatgeobuf(path: &str, bbox: Option<[f64; 4]>) -> Result<VectorRead, String> {
    // The crate indexes into flatbuffers it has verified; a panic would still be a crash of
    // the BEAM, so any that slips through becomes an error.
    catch_unwind(AssertUnwindSafe(|| read(path, bbox)))
        .unwrap_or_else(|_| Err(format!("{path} is not a readable FlatGeobuf file")))
}

/// An owned property value, widened like [`Cell`] but keeping its own storage.
#[derive(Clone)]
enum Value {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

/// Collects the properties of the current feature into per-column slots.
struct Slots(Vec<Option<Value>>);

impl PropertyProcessor for Slots {
    fn property(&mut self, i: usize, _name: &str, value: &ColumnValue) -> GeozeroResult<bool> {
        let converted = match *value {
            ColumnValue::Byte(v) => Value::Int(v.into()),
            ColumnValue::UByte(v) => Value::UInt(v.into()),
            ColumnValue::Bool(v) => Value::Bool(v),
            ColumnValue::Short(v) => Value::Int(v.into()),
            ColumnValue::UShort(v) => Value::UInt(v.into()),
            ColumnValue::Int(v) => Value::Int(v.into()),
            ColumnValue::UInt(v) => Value::UInt(v.into()),
            ColumnValue::Long(v) => Value::Int(v),
            ColumnValue::ULong(v) => Value::UInt(v),
            ColumnValue::Float(v) => Value::Float(v.into()),
            ColumnValue::Double(v) => Value::Float(v),
            ColumnValue::String(v) | ColumnValue::Json(v) | ColumnValue::DateTime(v) => {
                Value::Text(v.to_string())
            }
            ColumnValue::Binary(v) => Value::Bytes(v.to_vec()),
        };
        if let Some(slot) = self.0.get_mut(i) {
            *slot = Some(converted);
        }
        Ok(false)
    }
}

/// Checks a file before the `flatgeobuf` reader sees it, because the crate allocates
/// `u32 size + 4` bytes for every feature *before* checking the file holds them (a crafted
/// `FF FF FF FF` aborts the BEAM with a 4 GiB allocation). The check reads the header, skips
/// the index and walks the feature stream with seeks only, requiring each feature to fit in
/// the file. A bounding-box read is rejected unless the index node size is 16 (the crate's
/// `select_bbox` assumes it) and every index offset must land on a feature boundary.
pub(crate) fn prevalidate(path: &str, bbox: Option<[f64; 4]>) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("cannot read {path}: {e}"))?
        .len();
    let mut input = BufReader::new(file);
    let (count, node_size) = {
        let reader = FgbReader::open(&mut input)
            .map_err(|e| format!("cannot read {path} as FlatGeobuf: {e}"))?;
        let header = reader.header();
        (header.features_count(), header.index_node_size())
    };
    if count == 0 {
        return Ok(());
    }
    let io_err = |e: std::io::Error| format!("cannot read {path}: {e}");
    let truncated = || format!("{path} is truncated or has a corrupt feature size");
    // Every feature takes at least its 4-byte size prefix.
    if count > file_len / 4 {
        return Err(truncated());
    }
    let index_start = input.stream_position().map_err(io_err)?;
    let index_size = if node_size == 0 {
        0
    } else if node_size == 1 {
        return Err(format!("{path} has an unsupported index node size 1"));
    } else {
        flatgeobuf::packed_r_tree::PackedRTree::index_size(count as usize, node_size) as u64
    };
    let features_start = index_start
        .checked_add(index_size)
        .filter(|&start| start <= file_len)
        .ok_or_else(truncated)?;

    let mut allowed: Option<Vec<usize>> = None;
    if let Some([min_x, min_y, max_x, max_y]) = bbox {
        if node_size != 0 && node_size != flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE
        {
            return Err(format!(
                "{path} has index node size {node_size}; bbox reads support only 16"
            ));
        }
        if node_size != 0 {
            let hits = flatgeobuf::packed_r_tree::PackedRTree::stream_search(
                &mut input,
                count as usize,
                node_size,
                min_x,
                min_y,
                max_x,
                max_y,
            )
            .map_err(|e| format!("cannot read {path}: {e}"))?;
            allowed = Some(hits.into_iter().map(|h| h.offset).collect());
        }
    }
    input
        .seek(SeekFrom::Start(features_start))
        .map_err(io_err)?;

    let mut starts: Vec<usize> = Vec::new();
    let mut pos = features_start;
    for _ in 0..count {
        if allowed.is_some() {
            starts.push((pos - features_start) as usize);
        }
        let mut prefix = [0u8; 4];
        input.read_exact(&mut prefix).map_err(|_| truncated())?;
        let size = u64::from(u32::from_le_bytes(prefix));
        let next = pos
            .checked_add(4 + size)
            .filter(|&next| next <= file_len)
            .ok_or_else(truncated)?;
        input
            .seek_relative(i64::try_from(size).map_err(|_| truncated())?)
            .map_err(io_err)?;
        pos = next;
    }
    if let Some(offsets) = allowed {
        // `starts` is increasing by construction.
        if offsets.iter().any(|o| starts.binary_search(o).is_err()) {
            return Err(format!("{path} has a corrupt spatial index"));
        }
    }
    Ok(())
}

fn read(path: &str, bbox: Option<[f64; 4]>) -> Result<VectorRead, String> {
    prevalidate(path, bbox)?;
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let reader = FgbReader::open(BufReader::new(file))
        .map_err(|e| format!("cannot read {path} as FlatGeobuf: {e}"))?;

    let header = reader.header();
    let crs = header_crs(&header);
    let columns: Vec<(String, ColumnType)> = header
        .columns()
        .map(|columns| {
            columns
                .iter()
                .map(|c| (c.name().to_string(), c.type_()))
                .collect()
        })
        .unwrap_or_default();
    let empty = header.features_count() == 0;

    let mut builder = ColumnBuilder::new(crs.as_deref());
    let mut cells: Vec<Vec<Option<Value>>> = vec![Vec::new(); columns.len()];
    let mut slots = Slots(vec![None; columns.len()]);
    if !empty {
        let mut features = match bbox {
            None => reader.select_all(),
            Some([min_x, min_y, max_x, max_y]) => reader.select_bbox(min_x, min_y, max_x, max_y),
        }
        .map_err(|e| match e {
            flatgeobuf::Error::NoIndex => format!(
                "{path} has no spatial index (index missing), so it cannot be filtered by bbox"
            ),
            e => format!("cannot read {path}: {e}"),
        })?;
        while let Some(feature) = features
            .next()
            .map_err(|e| format!("cannot read {path}: {e}"))?
        {
            // The encoder stores a null geometry as an empty one (and gives it a stale index
            // box), so empty means null. A bbox query never returns geometry-less rows.
            if is_empty_geometry(feature) {
                if bbox.is_some() {
                    continue;
                }
                builder.push(None::<&Geometry<f64>>)?;
            } else {
                let geometry = feature
                    .geometry_trait()
                    .map_err(|e| format!("unsupported geometry in {path}: {e}"))?;
                builder.push(geometry.as_ref())?;
            }

            slots.0.iter_mut().for_each(|slot| *slot = None);
            feature
                .process_properties(&mut slots)
                .map_err(|e| format!("cannot read properties of {path}: {e}"))?;
            for (column, slot) in cells.iter_mut().zip(slots.0.iter_mut()) {
                column.push(slot.take());
            }
        }
    }

    let geometry = builder.finish();
    let rows = geometry.len();
    let (fields, arrays): (Vec<Field>, Vec<ArrayRef>) = columns
        .iter()
        .zip(&cells)
        .map(|((name, kind), cells)| {
            let array = build_column(*kind, cells);
            (Field::new(name, array.data_type().clone(), true), array)
        })
        .unzip();
    Ok(VectorRead {
        geometry,
        attributes_ipc: ipc_from_columns(fields, arrays, rows)?,
    })
}

fn is_empty_geometry(feature: &FgbFeature) -> bool {
    feature.geometry().is_none_or(|g| {
        g.xy().is_none_or(|xy| xy.is_empty()) && g.parts().is_none_or(|parts| parts.is_empty())
    })
}

fn build_column(kind: ColumnType, cells: &[Option<Value>]) -> ArrayRef {
    macro_rules! column {
        ($array:ident, $variant:ident, $ty:ty) => {
            Arc::new($array::from_iter(cells.iter().map(|c| match c {
                Some(Value::$variant(v)) => Some(*v as $ty),
                _ => None,
            })))
        };
    }
    match kind {
        ColumnType::Byte => column!(Int8Array, Int, i8),
        ColumnType::UByte => column!(UInt8Array, UInt, u8),
        ColumnType::Short => column!(Int16Array, Int, i16),
        ColumnType::UShort => column!(UInt16Array, UInt, u16),
        ColumnType::Int => column!(Int32Array, Int, i32),
        ColumnType::UInt => column!(UInt32Array, UInt, u32),
        ColumnType::Long => column!(Int64Array, Int, i64),
        ColumnType::ULong => column!(UInt64Array, UInt, u64),
        ColumnType::Float => column!(Float32Array, Float, f32),
        ColumnType::Double => column!(Float64Array, Float, f64),
        ColumnType::Bool => column!(BooleanArray, Bool, bool),
        ColumnType::Binary => Arc::new(BinaryArray::from_iter(cells.iter().map(|c| match c {
            Some(Value::Bytes(v)) => Some(v.as_slice()),
            _ => None,
        }))),
        // String, Json, DateTime and any type added to the format later
        _ => Arc::new(StringArray::from_iter(cells.iter().map(|c| match c {
            Some(Value::Text(v)) => Some(v.as_str()),
            _ => None,
        }))),
    }
}

fn header_crs(header: &Header<'_>) -> Option<String> {
    let crs = header.crs()?;
    let org = crs.org().filter(|o| !o.is_empty()).unwrap_or("EPSG");
    if crs.code() != 0 {
        return Some(format!("{org}:{}", crs.code()));
    }
    if let Some(code) = crs.code_string().filter(|c| !c.is_empty()) {
        return Some(format!("{org}:{code}"));
    }
    crs.wkt().filter(|w| !w.is_empty()).map(str::to_string)
}

// -- write --------------------------------------------------------------------------------

/// Writes `geometry` and the attributes as a FlatGeobuf file named `layer_name` in its
/// header. `index` adds the spatial index (and Hilbert-sorts the rows).
pub fn write_flatgeobuf(
    path: &str,
    geometry: &GeoColumn,
    attributes_ipc: &[u8],
    layer_name: &str,
    index: bool,
) -> Result<(), String> {
    let table = Table::from_ipc(attributes_ipc, geometry.len())?;
    let column_types = table
        .arrays
        .iter()
        .zip(&table.names)
        .map(|(array, name)| fgb_type(name, array.data_type()))
        .collect::<Result<Vec<_>, _>>()?;
    let crs = crs_fields(geometry.crs().as_deref())?;
    let rows = geometry.geo_rows();

    // Build in memory first: the crate writes through a temp file and only emits the output
    // at the end, so a failure here costs nothing and leaves no partial file.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        build(&rows, &table, &column_types, &crs, layer_name, index)
    }))
    .unwrap_or_else(|_| Err("FlatGeobuf encoder failed".to_string()))?;

    crate::atomic::write_atomic(path, |mut file| {
        use std::io::Write;
        file.write_all(&outcome)
            .map_err(|e| format!("cannot write {path}: {e}"))
    })
}

fn build(
    rows: &[Option<Geometry<f64>>],
    table: &Table,
    column_types: &[ColumnType],
    crs: &OwnedCrs,
    layer_name: &str,
    index: bool,
) -> Result<Vec<u8>, String> {
    let options = FgbWriterOptions {
        write_index: index,
        detect_type: false,
        promote_to_multi: false,
        crs: FgbCrs {
            org: crs.org.as_deref(),
            code: crs.code,
            code_string: crs.code_string.as_deref(),
            wkt: crs.wkt.as_deref(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut writer = FgbWriter::create_with_options(layer_name, common_type(rows), options)
        .map_err(|e| e.to_string())?;
    for (name, kind) in table.names.iter().zip(column_types) {
        writer.add_column(name, *kind, |_, column| column.nullable = true);
    }

    for (row, geometry) in rows.iter().enumerate() {
        let mut failure = None;
        writer
            .add_feature_geom(RowGeometry(geometry.as_ref()), |feature| {
                for (column, kind) in column_types.iter().enumerate() {
                    let Some(value) = column_value(*kind, table.cell(column, row)) else {
                        continue;
                    };
                    if let Err(e) = feature.property(column, &table.names[column], &value) {
                        failure = Some(e.to_string());
                        return;
                    }
                }
            })
            .map_err(|e| format!("cannot encode row {row}: {e}"))?;
        if let Some(message) = failure {
            return Err(format!("cannot encode row {row}: {message}"));
        }
    }

    let mut out = Vec::new();
    writer.write(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// A row's geometry for the FlatGeobuf encoder; a null row emits nothing.
struct RowGeometry<'a>(Option<&'a Geometry<f64>>);

impl GeozeroGeometry for RowGeometry<'_> {
    fn process_geom<P: GeomProcessor>(&self, processor: &mut P) -> GeozeroResult<()> {
        match self.0 {
            Some(geometry) => geometry.process_geom(processor),
            None => Ok(()),
        }
    }
}

/// The header geometry type: the one type all rows share, else `Unknown`.
fn common_type(rows: &[Option<Geometry<f64>>]) -> GeometryType {
    let mut types = rows.iter().flatten().map(|g| match g {
        Geometry::Point(_) => GeometryType::Point,
        Geometry::Line(_) | Geometry::LineString(_) => GeometryType::LineString,
        Geometry::Polygon(_) | Geometry::Rect(_) | Geometry::Triangle(_) => GeometryType::Polygon,
        Geometry::MultiPoint(_) => GeometryType::MultiPoint,
        Geometry::MultiLineString(_) => GeometryType::MultiLineString,
        Geometry::MultiPolygon(_) => GeometryType::MultiPolygon,
        Geometry::GeometryCollection(_) => GeometryType::GeometryCollection,
    });
    match types.next() {
        Some(first) if types.all(|t| t == first) => first,
        _ => GeometryType::Unknown,
    }
}

fn fgb_type(name: &str, data_type: &DataType) -> Result<ColumnType, String> {
    Ok(match data_type {
        DataType::Boolean => ColumnType::Bool,
        DataType::Int8 => ColumnType::Byte,
        DataType::UInt8 => ColumnType::UByte,
        DataType::Int16 => ColumnType::Short,
        DataType::UInt16 => ColumnType::UShort,
        DataType::Int32 => ColumnType::Int,
        DataType::UInt32 => ColumnType::UInt,
        DataType::Int64 => ColumnType::Long,
        DataType::UInt64 => ColumnType::ULong,
        DataType::Float32 => ColumnType::Float,
        DataType::Float64 => ColumnType::Double,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => ColumnType::String,
        DataType::Binary | DataType::LargeBinary => ColumnType::Binary,
        other => {
            return Err(format!(
                "column {name:?} has type {other}, which FlatGeobuf cannot store"
            ))
        }
    })
}

/// The value to store for `cell` in a column of type `kind`; `None` for nulls (FlatGeobuf
/// stores a null as an absent property). Narrowing is lossless: the cell came from that type.
fn column_value(kind: ColumnType, cell: Cell<'_>) -> Option<ColumnValue<'_>> {
    Some(match (kind, cell) {
        (_, Cell::Null) => return None,
        (ColumnType::Bool, Cell::Bool(v)) => ColumnValue::Bool(v),
        (ColumnType::Byte, Cell::Int(v)) => ColumnValue::Byte(v as i8),
        (ColumnType::Short, Cell::Int(v)) => ColumnValue::Short(v as i16),
        (ColumnType::Int, Cell::Int(v)) => ColumnValue::Int(v as i32),
        (ColumnType::Long, Cell::Int(v)) => ColumnValue::Long(v),
        (ColumnType::UByte, Cell::UInt(v)) => ColumnValue::UByte(v as u8),
        (ColumnType::UShort, Cell::UInt(v)) => ColumnValue::UShort(v as u16),
        (ColumnType::UInt, Cell::UInt(v)) => ColumnValue::UInt(v as u32),
        (ColumnType::ULong, Cell::UInt(v)) => ColumnValue::ULong(v),
        (ColumnType::Float, Cell::Float(v)) => ColumnValue::Float(v as f32),
        (ColumnType::Double, Cell::Float(v)) => ColumnValue::Double(v),
        (ColumnType::String, Cell::Str(v)) => ColumnValue::String(v),
        (ColumnType::Binary, Cell::Bytes(v)) => ColumnValue::Binary(v),
        _ => return None,
    })
}

#[derive(Default)]
struct OwnedCrs {
    org: Option<String>,
    code: i32,
    code_string: Option<String>,
    wkt: Option<String>,
}

fn crs_fields(crs: Option<&str>) -> Result<OwnedCrs, String> {
    let Some(text) = crs.map(str::trim) else {
        return Ok(OwnedCrs::default());
    };
    if text.eq_ignore_ascii_case("OGC:CRS84")
        || text.eq_ignore_ascii_case("urn:ogc:def:crs:OGC:1.3:CRS84")
    {
        return Ok(OwnedCrs {
            org: Some("EPSG".into()),
            code: 4326,
            ..Default::default()
        });
    }
    let wkt_roots = [
        "GEOGCS[",
        "PROJCS[",
        "GEOGCRS[",
        "PROJCRS[",
        "BOUNDCRS[",
        "COMPOUNDCRS[",
        "COMPD_CS[",
        "GEOCCS[",
    ];
    let upper = text.to_ascii_uppercase();
    if wkt_roots.iter().any(|root| upper.starts_with(root)) {
        return Ok(OwnedCrs {
            wkt: Some(text.to_string()),
            ..Default::default()
        });
    }
    if !text.starts_with('{') {
        if let Some((authority, code)) = text.split_once(':') {
            if !authority.is_empty() && !code.is_empty() && !authority.contains(char::is_whitespace)
            {
                return Ok(match code.parse::<i32>() {
                    Ok(code) if code > 0 => OwnedCrs {
                        org: Some(authority.to_string()),
                        code,
                        ..Default::default()
                    },
                    _ => OwnedCrs {
                        org: Some(authority.to_string()),
                        code_string: Some(code.to_string()),
                        ..Default::default()
                    },
                });
            }
        }
    }
    Err(format!(
        "CRS {text:?} cannot be written to FlatGeobuf; use an AUTHORITY:CODE string or WKT"
    ))
}
