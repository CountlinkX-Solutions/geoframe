//! Secondary geometry columns in GeoParquet (issue #61): the `geo` metadata may declare several
//! geometry columns; each one is decoded, keeps its CRS, and is written back with its entry.

use std::fs::File;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use tempfile::TempDir;

use crate::{
    read_geoparquet, read_geoparquet_filtered, write_geoparquet_multi, GeoColumn, GeometryEncoding,
    ParquetOptions,
};

fn ids() -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
    let columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(vec![1, 2, 3]))];
    let batch = RecordBatch::try_new(schema, columns).unwrap();
    let mut out = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut out, &batch.schema()).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    out
}

fn primary() -> GeoColumn {
    GeoColumn::from_wkt(
        &[Some("POINT (0 0)"), Some("POINT (1 1)"), None],
        Some("EPSG:4326"),
    )
    .unwrap()
}

fn secondary() -> GeoColumn {
    GeoColumn::from_wkt(
        &[
            Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
            None,
            Some("POLYGON ((5 5, 6 5, 6 6, 5 5))"),
        ],
        Some("EPSG:3857"),
    )
    .unwrap()
}

fn file(dir: &TempDir) -> String {
    dir.path().join("m.parquet").to_str().unwrap().to_string()
}

fn geo_json(path: &str) -> serde_json::Value {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path).unwrap()).unwrap();
    let kv = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .unwrap()
        .iter()
        .find(|kv| kv.key == "geo")
        .expect("geo key");
    serde_json::from_str(kv.value.as_ref().unwrap()).unwrap()
}

fn wkt(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}

#[test]
fn secondary_geometry_round_trips_with_its_own_crs_and_metadata() {
    let dir = TempDir::new().unwrap();
    let path = file(&dir);
    let (a, b) = (primary(), secondary());
    write_geoparquet_multi(
        &path,
        &[("geometry", &a), ("footprint", &b)],
        &ids(),
        &ParquetOptions::default(),
    )
    .unwrap();

    let geo = geo_json(&path);
    assert_eq!(geo["primary_column"], "geometry");
    assert_eq!(geo["columns"]["footprint"]["encoding"], "WKB");
    assert!(geo["columns"]["footprint"]["crs"]["id"]["code"] == 3857);

    let read = read_geoparquet(&path, None).unwrap();
    assert_eq!(read.geometry_column, "geometry");
    assert_eq!(read.secondary_geometries.len(), 1);
    let (name, column) = &read.secondary_geometries[0];
    assert_eq!(name, "footprint");
    assert_eq!(wkt(column), wkt(&b));
    assert_eq!(column.crs().as_deref(), Some("EPSG:3857"));
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
}

#[test]
fn native_encoding_applies_to_every_column_and_covering_to_the_primary() {
    let dir = TempDir::new().unwrap();
    let path = file(&dir);
    let points = GeoColumn::from_wkt(
        &[
            Some("POINT (0 0)"),
            Some("POINT (1 1)"),
            Some("POINT (2 2)"),
        ],
        None,
    )
    .unwrap();
    let other = GeoColumn::from_wkt(
        &[Some("POINT (9 9)"), None, Some("POINT (8 8)")],
        Some("EPSG:4326"),
    )
    .unwrap();
    let options = ParquetOptions {
        geometry_encoding: GeometryEncoding::GeoArrow,
        covering_bbox: true,
        ..ParquetOptions::default()
    };
    write_geoparquet_multi(&path, &[("g", &points), ("h", &other)], &ids(), &options).unwrap();

    let read = read_geoparquet(&path, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&points));
    let (name, column) = &read.secondary_geometries[0];
    assert_eq!(name, "h");
    assert_eq!(wkt(column), wkt(&other));
    // Covering columns are never attributes, and only the primary column has one.
    assert!(!String::from_utf8_lossy(&read.attributes_ipc).contains("bbox"));
    let geo = geo_json(&path);
    assert!(geo["columns"]["g"]["covering"].is_object());
    assert!(geo["columns"]["h"]["covering"].is_null());
}

#[test]
fn bbox_filter_and_column_selection_apply_to_secondary_geometries() {
    let dir = TempDir::new().unwrap();
    let path = file(&dir);
    let (a, b) = (primary(), secondary());
    write_geoparquet_multi(
        &path,
        &[("geometry", &a), ("footprint", &b)],
        &ids(),
        &ParquetOptions::default(),
    )
    .unwrap();

    let read = read_geoparquet_filtered(&path, None, Some([0.5, 0.5, 2.0, 2.0])).unwrap();
    assert_eq!(read.geometry.len(), 1);
    assert_eq!(read.secondary_geometries[0].1.len(), 1);

    // Naming only attributes leaves secondary geometry columns out, like any unselected column.
    let only_id = vec!["id".to_string()];
    let read = read_geoparquet(&path, Some(&only_id)).unwrap();
    assert!(read.secondary_geometries.is_empty());
    let wanted = vec!["footprint".to_string()];
    let read = read_geoparquet(&path, Some(&wanted)).unwrap();
    assert_eq!(read.secondary_geometries.len(), 1);
}

#[test]
fn duplicate_or_clashing_geometry_names_are_rejected() {
    let dir = TempDir::new().unwrap();
    let path = file(&dir);
    let (a, b) = (primary(), secondary());
    let both = [("geometry", &a), ("geometry", &b)];
    assert!(write_geoparquet_multi(&path, &both, &ids(), &ParquetOptions::default()).is_err());
    let clash = [("geometry", &a), ("id", &b)];
    assert!(write_geoparquet_multi(&path, &clash, &ids(), &ParquetOptions::default()).is_err());
    assert!(!std::path::Path::new(&path).exists());
}
