//! GeoJSON read and write.
//!
//! # Reading
//!
//! A `FeatureCollection` becomes one row per feature; a lone `Feature` or a bare geometry
//! object is accepted as a one-row file. The geometry goes to a [`GeoColumn`] and the
//! properties to an Arrow IPC stream, like every other format.
//!
//! * **Type inference** looks at the whole file, not the first feature. Per property: `null`
//!   (and a missing key) is ignored; all booleans give `Boolean`; all integers give `Int64`;
//!   integers mixed with other numbers give `Float64`; everything else (strings, mixed
//!   kinds, objects, arrays) gives `Utf8`. Nested objects and arrays are stored as compact
//!   JSON text; scalars in a mixed column are stored as their JSON text (`true`, `7`).
//!   A property that is `null` everywhere becomes an all-null `Utf8` column.
//! * **Column order** is the order in which properties are first seen.
//! * **Z**: a position with three or more values gives the geometry a Z ordinate (RFC 7946);
//!   values past the third are ignored, as GDAL does. Positions with two values inside such a
//!   geometry (or members of a collection with fewer) get a NaN Z, so the geometry keeps one
//!   dimension where GDAL would fill zeros. M has no GeoJSON form.
//! * **Feature `id`**: when every feature has one (string or number) and no property is
//!   named `id`, it becomes a leading `id` column.
//! * **CRS**: RFC 7946 fixes GeoJSON to WGS 84 longitude/latitude, so the geometry gets
//!   `OGC:CRS84`. The legacy `crs` member is ignored.
//!
//! # Writing
//!
//! GeoJSON cannot carry another CRS, so writing requires `OGC:CRS84` or `EPSG:4326` unless
//! `force` is set (GeoPandas instead writes whatever coordinates it has). Every attribute
//! becomes a property in column order; the feature `id` is not written. `NaN` and infinite
//! floats become `null` (JSON has no such numbers). Date, time, timestamp, decimal and
//! categorical columns are written as text; nested columns are rejected. Geometries
//! that GeoJSON cannot express (empty points) are written as `null`. Z is written as the
//! third ordinate (a NaN Z as a two-value position, since JSON has no NaN); M is dropped.
//! A failed write leaves no file behind.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::str::FromStr;
use std::sync::Arc;

use arrow_array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
use arrow_schema::{DataType, Field};
use geojson::{feature::Id, GeoJson};
use serde_json::{Number, Value};

use crate::attributes::{ipc_from_columns, Cell, Table};
use crate::column::ColumnBuilder;
use crate::{GeoColumn, VectorRead};

pub(crate) const CRS84: &str = "OGC:CRS84";

// -- read ---------------------------------------------------------------------------------

/// Reads a GeoJSON file; see the module documentation for the inference rules.
pub fn read_geojson(path: &str) -> Result<VectorRead, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    parse_geojson(&text, path, Some(CRS84))
}

/// Reads GeoJSON text with the same inference rules as [`read_geojson`], giving the geometry
/// the CRS `crs` (the file reader always uses `OGC:CRS84`).
pub fn read_geojson_str(text: &str, crs: Option<&str>) -> Result<VectorRead, String> {
    parse_geojson(text, "the given text", crs)
}

fn parse_geojson(text: &str, source: &str, crs: Option<&str>) -> Result<VectorRead, String> {
    let features = features_of(text, source)?;
    let mut builder = ColumnBuilder::new(crs);
    for (row, feature) in features.iter().enumerate() {
        push_geometry(&mut builder, row, feature)?;
    }

    let ids = feature_ids(&features);
    let properties = collect_properties(&features, ids.as_deref());
    let rows = features.len();
    let (fields, columns): (Vec<Field>, Vec<ArrayRef>) = properties
        .into_iter()
        .map(|(name, cells)| {
            let array = build_column(&cells);
            (Field::new(name, array.data_type().clone(), true), array)
        })
        .unzip();
    Ok(VectorRead {
        geometry: builder.finish(),
        attributes_ipc: ipc_from_columns(fields, columns, rows)?,
    })
}

/// The features of GeoJSON text: a `FeatureCollection`'s, or a lone `Feature` or bare geometry
/// as a one-feature list.
pub(crate) fn features_of(text: &str, source: &str) -> Result<Vec<geojson::Feature>, String> {
    let parsed =
        GeoJson::from_str(text).map_err(|e| format!("invalid GeoJSON in {source}: {e}"))?;
    Ok(match parsed {
        GeoJson::FeatureCollection(collection) => collection.features,
        GeoJson::Feature(feature) => vec![feature],
        GeoJson::Geometry(geometry) => vec![geojson::Feature {
            geometry: Some(geometry),
            ..Default::default()
        }],
    })
}

/// Appends the geometry of `feature` (the `row`th of the file) to `builder`.
pub(crate) fn push_geometry(
    builder: &mut ColumnBuilder,
    row: usize,
    feature: &geojson::Feature,
) -> Result<(), String> {
    match &feature.geometry {
        None => builder.push(None::<&geo::Geometry<f64>>),
        Some(geometry) => {
            let invalid = |e: String| format!("invalid geometry at feature {row}: {e}");
            let bytes = crate::zm_io::geojson_to_wkb(geometry).map_err(invalid)?;
            let parsed = wkb::reader::read_wkb(&bytes).map_err(|e| invalid(e.to_string()))?;
            builder.push(Some(&parsed))
        }
    }
}

/// The `id` of every feature as a JSON value, or `None` unless all features have one.
fn feature_ids(features: &[geojson::Feature]) -> Option<Vec<Value>> {
    features
        .iter()
        .map(|f| match f.id.as_ref()? {
            Id::String(s) => Some(Value::String(s.clone())),
            Id::Number(n) => Some(Value::Number(n.clone())),
        })
        .collect()
}

/// Columns in first-seen order, each with one optional value per feature (`None` = missing
/// or JSON `null`).
fn collect_properties<'a>(
    features: &'a [geojson::Feature],
    ids: Option<&'a [Value]>,
) -> Vec<(String, Vec<Option<&'a Value>>)> {
    let mut columns: Vec<(String, Vec<Option<&Value>>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (row, feature) in features.iter().enumerate() {
        if let Some(properties) = &feature.properties {
            for (key, value) in properties {
                let slot = *index.entry(key.as_str()).or_insert_with(|| {
                    columns.push((key.clone(), vec![None; row]));
                    columns.len() - 1
                });
                columns[slot].1.push((!value.is_null()).then_some(value));
            }
        }
        for (_, cells) in &mut columns {
            if cells.len() == row {
                cells.push(None);
            }
        }
    }
    if let Some(ids) = ids {
        if !index.contains_key("id") && !ids.is_empty() {
            columns.insert(0, ("id".to_string(), ids.iter().map(Some).collect()));
        }
    }
    columns
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Unseen,
    Bool,
    Int,
    Float,
    Text,
}

pub(crate) fn kind_of(value: &Value) -> Kind {
    match value {
        Value::Bool(_) => Kind::Bool,
        Value::Number(n) if n.is_i64() => Kind::Int,
        Value::Number(_) => Kind::Float,
        _ => Kind::Text,
    }
}

pub(crate) fn merge(a: Kind, b: Kind) -> Kind {
    match (a, b) {
        (Kind::Unseen, other) | (other, Kind::Unseen) => other,
        (a, b) if a == b => a,
        (Kind::Int, Kind::Float) | (Kind::Float, Kind::Int) => Kind::Float,
        _ => Kind::Text,
    }
}

fn build_column(cells: &[Option<&Value>]) -> ArrayRef {
    let kind = cells
        .iter()
        .flatten()
        .fold(Kind::Unseen, |kind, value| merge(kind, kind_of(value)));
    build_column_as(kind, cells)
}

/// A column of type `kind` from `cells` (`None` = null); a streamed file fixes `kind` from a
/// first pass over the whole file.
pub(crate) fn build_column_as(kind: Kind, cells: &[Option<&Value>]) -> ArrayRef {
    let present = || cells.iter().map(|c| c.as_deref());
    match kind {
        Kind::Bool => Arc::new(BooleanArray::from_iter(
            present().map(|c| c.and_then(Value::as_bool)),
        )),
        Kind::Int => Arc::new(Int64Array::from_iter(
            present().map(|c| c.and_then(Value::as_i64)),
        )),
        Kind::Float => Arc::new(Float64Array::from_iter(
            present().map(|c| c.and_then(Value::as_f64)),
        )),
        Kind::Text | Kind::Unseen => Arc::new(StringArray::from_iter(present().map(|c| {
            c.map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
        }))),
    }
}

// -- write --------------------------------------------------------------------------------

/// Writes a `FeatureCollection`; see the module documentation for the CRS rule and value
/// conversions. `force` skips the lon/lat CRS check.
pub fn write_geojson(
    path: &str,
    geometry: &GeoColumn,
    attributes_ipc: &[u8],
    force: bool,
) -> Result<(), String> {
    if !force {
        check_crs(geometry.crs().as_deref())?;
    }
    let table = Table::from_ipc(attributes_ipc, geometry.len())?;
    crate::atomic::write_atomic(path, |file| {
        write_features(file, geometry, &table).map_err(|m| format!("cannot write {path}: {m}"))
    })
}

fn write_features(file: File, geometry: &GeoColumn, table: &Table) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let geometries = geometry.to_geojson();
    let keys: Vec<String> = table
        .names
        .iter()
        .map(|n| Value::String(n.clone()).to_string())
        .collect();
    let mut out = BufWriter::new(file);
    out.write_all(br#"{"type":"FeatureCollection","features":["#)
        .map_err(io)?;
    for (row, geometry) in geometries.iter().enumerate() {
        if row > 0 {
            out.write_all(b",").map_err(io)?;
        }
        out.write_all(br#"{"type":"Feature","properties":{"#)
            .map_err(io)?;
        for (column, key) in keys.iter().enumerate() {
            if column > 0 {
                out.write_all(b",").map_err(io)?;
            }
            let value = cell_to_json(&table.names[column], table.cell(column, row))?;
            write!(out, "{key}:{value}").map_err(io)?;
        }
        out.write_all(br#"},"geometry":"#).map_err(io)?;
        out.write_all(geometry.as_deref().unwrap_or("null").as_bytes())
            .map_err(io)?;
        out.write_all(b"}").map_err(io)?;
    }
    out.write_all(b"]}").map_err(io)?;
    out.flush().map_err(io)
}

fn cell_to_json(column: &str, cell: Cell<'_>) -> Result<Value, String> {
    Ok(match cell {
        Cell::Null => Value::Null,
        Cell::Bool(b) => Value::Bool(b),
        Cell::Int(i) => Value::from(i),
        Cell::UInt(u) => Value::from(u),
        Cell::Float(f) => Number::from_f64(f).map_or(Value::Null, Value::Number),
        Cell::Str(s) => Value::String(s.to_string()),
        Cell::Bytes(_) => {
            return Err(format!(
                "column {column:?} has type {}, which GeoJSON cannot store",
                DataType::Binary
            ))
        }
    })
}

/// Lon/lat identifiers GeoJSON (RFC 7946) can carry.
fn check_crs(crs: Option<&str>) -> Result<(), String> {
    let lon_lat = crs.is_some_and(|crs| {
        let crs = crs.trim();
        [
            "OGC:CRS84",
            "EPSG:4326",
            "urn:ogc:def:crs:OGC:1.3:CRS84",
            "urn:ogc:def:crs:EPSG::4326",
        ]
        .iter()
        .any(|known| crs.eq_ignore_ascii_case(known))
    });
    if lon_lat {
        return Ok(());
    }
    Err(format!(
        "GeoJSON holds only WGS 84 longitude/latitude (RFC 7946) but the CRS is {}; \
         reproject with to_crs(\"EPSG:4326\") or pass force: true to write the coordinates as they are",
        crs.map_or("unknown".to_string(), |c| format!("{c:?}"))
    ))
}
