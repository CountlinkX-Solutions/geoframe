use std::fs::File;
use std::sync::Arc;

use arrow_array::{Array, ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use geoparquet::writer::{GeoParquetRecordBatchEncoder, GeoParquetWriterOptionsBuilder};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use tempfile::TempDir;

use crate::{read_geoparquet, write_geoparquet, GeoColumn};

fn attributes() -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])),
        Arc::new(StringArray::from(vec![Some("a"), Some("b"), None])),
        Arc::new(Float64Array::from(vec![Some(1.5), Some(2.5), None])),
    ];
    ipc(RecordBatch::try_new(schema, columns).unwrap())
}

fn ipc(batch: RecordBatch) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut out, &batch.schema()).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    out
}

fn read_ipc(bytes: &[u8]) -> Vec<RecordBatch> {
    StreamReader::try_new(bytes, None)
        .unwrap()
        .map(|b| b.unwrap())
        .collect()
}

fn geometry(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(
        &[
            Some("POINT (1 2)"),
            None,
            Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"),
        ],
        crs,
    )
    .unwrap()
}

fn path(dir: &TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_string()
}

/// The raw `geo` metadata JSON stored in a parquet file.
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

#[test]
fn round_trip_keeps_geometry_attributes_and_crs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.parquet");
    let geom = geometry(Some("EPSG:4326"));
    write_geoparquet(&file, &geom, "geometry", &attributes()).unwrap();

    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry_column, "geometry");
    assert_eq!(read.geometry.to_wkt().unwrap(), geom.to_wkt().unwrap());
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));

    let batches = read_ipc(&read.attributes_ipc);
    let batch = &batches[0];
    let names: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["id", "name", "score"]);
    let ids = batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(ids.iter().collect::<Vec<_>>(), [Some(1), None, Some(3)]);
    let scores = batch
        .column(2)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert_eq!(
        scores.iter().collect::<Vec<_>>(),
        [Some(1.5), Some(2.5), None]
    );
}

#[test]
fn writes_geoparquet_1_1_with_wkb_and_authority_crs_as_projjson() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "meta.parquet");
    write_geoparquet(
        &file,
        &geometry(Some("EPSG:3857")),
        "geometry",
        &attributes(),
    )
    .unwrap();

    let geo = geo_json(&file);
    assert_eq!(geo["version"], "1.1.0");
    assert_eq!(geo["primary_column"], "geometry");
    let col = &geo["columns"]["geometry"];
    assert_eq!(col["encoding"], "WKB");
    assert_eq!(col["crs"]["id"]["authority"], "EPSG");
    assert_eq!(col["crs"]["id"]["code"], 3857);
}

#[test]
fn crs84_is_written_as_missing_key() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "crs84.parquet");
    write_geoparquet(
        &file,
        &geometry(Some("OGC:CRS84")),
        "geometry",
        &attributes(),
    )
    .unwrap();
    assert!(geo_json(&file)["columns"]["geometry"].get("crs").is_none());
    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry.crs().as_deref(), Some("OGC:CRS84"));
}

#[test]
fn unknown_crs_is_written_as_explicit_null_and_read_back_as_unknown() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "unknown.parquet");
    write_geoparquet(&file, &geometry(None), "geometry", &attributes()).unwrap();
    let geo = geo_json(&file);
    assert!(geo["columns"]["geometry"]["crs"].is_null());
    assert!(geo["columns"]["geometry"].get("crs").is_some());
    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry.crs(), None);
}

#[test]
fn projjson_crs_round_trips() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "projjson.parquet");
    let projjson =
        r#"{"type":"ProjectedCRS","name":"custom","id":{"authority":"ESRI","code":102003}}"#;
    write_geoparquet(&file, &geometry(Some(projjson)), "geometry", &attributes()).unwrap();
    let read = read_geoparquet(&file, None).unwrap();
    // A PROJJSON object with an `id` is normalised to its authority code.
    assert_eq!(read.geometry.crs().as_deref(), Some("ESRI:102003"));
}

#[test]
fn files_from_other_writers_without_crs_key_default_to_crs84() {
    // Written with the geoparquet crate directly: a default (absent) CRS omits the key.
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "foreign.parquet");
    let geom = geometry(None);
    let batch = foreign_batch(&geom);
    let options = GeoParquetWriterOptionsBuilder::default().build();
    let mut encoder = GeoParquetRecordBatchEncoder::try_new(&batch.schema(), &options).unwrap();
    let encoded = encoder.encode_record_batch(&batch).unwrap();
    let mut writer =
        ArrowWriter::try_new(File::create(&file).unwrap(), encoder.target_schema(), None).unwrap();
    writer.write(&encoded).unwrap();
    writer.append_key_value_metadata(encoder.into_keyvalue().unwrap());
    writer.close().unwrap();

    assert!(geo_json(&file)["columns"]["geometry"].get("crs").is_none());
    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry.crs().as_deref(), Some("OGC:CRS84"));
    assert_eq!(read.geometry.len(), 3);
}

fn foreign_batch(geom: &GeoColumn) -> RecordBatch {
    use geoarrow_array::{GeoArrowArray, IntoArrow};
    let array = geom.geometry_array();
    let field = array.extension_type().to_field("geometry", true);
    let ids: ArrayRef = Arc::new(Int64Array::from(vec![1, 2, 3]));
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        field,
    ]));
    RecordBatch::try_new(schema, vec![ids, array.to_array_ref()]).unwrap()
}

#[test]
fn plain_parquet_is_rejected_with_a_clear_error() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "plain.parquet");
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let batch =
        RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(vec![1]))]).unwrap();
    let mut writer = ArrowWriter::try_new(File::create(&file).unwrap(), schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let err = read_geoparquet(&file, None).err().unwrap();
    assert!(err.contains("not a GeoParquet file"), "unexpected: {err}");
}

#[test]
fn missing_file_names_the_path() {
    let err = read_geoparquet("/nonexistent/dir/x.parquet", None)
        .err()
        .unwrap();
    assert!(
        err.contains("/nonexistent/dir/x.parquet"),
        "unexpected: {err}"
    );
}

#[test]
fn non_parquet_file_is_an_error() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "junk.parquet");
    std::fs::write(&file, b"this is not parquet").unwrap();
    assert!(read_geoparquet(&file, None).is_err());
}

#[test]
fn column_selection_keeps_requested_attributes_in_file_order() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "cols.parquet");
    write_geoparquet(&file, &geometry(None), "geometry", &attributes()).unwrap();
    let wanted = vec!["score".to_string(), "id".to_string()];
    let read = read_geoparquet(&file, Some(&wanted)).unwrap();
    let batches = read_ipc(&read.attributes_ipc);
    let names: Vec<_> = batches[0]
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["id", "score"]);
    assert_eq!(read.geometry.len(), 3);

    let missing = vec!["nope".to_string()];
    let err = read_geoparquet(&file, Some(&missing)).err().unwrap();
    assert!(err.contains("nope"), "unexpected: {err}");
}

#[test]
fn custom_geometry_column_name_round_trips() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "named.parquet");
    write_geoparquet(&file, &geometry(None), "geom", &attributes()).unwrap();
    assert_eq!(geo_json(&file)["primary_column"], "geom");
    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry_column, "geom");
}

#[test]
fn row_count_mismatch_is_an_error() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "bad.parquet");
    let short = GeoColumn::from_wkt(&[Some("POINT (0 0)")], None).unwrap();
    let err = write_geoparquet(&file, &short, "geometry", &attributes()).unwrap_err();
    assert!(err.contains("rows"), "unexpected: {err}");
}

#[test]
fn writes_with_no_attribute_columns() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "geomonly.parquet");
    let empty = ipc(RecordBatch::new_empty(Arc::new(Schema::empty())));
    write_geoparquet(&file, &geometry(None), "geometry", &empty).unwrap();
    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry.len(), 3);
}

#[test]
fn all_null_geometry_column_round_trips_without_bbox() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "nulls.parquet");
    let nulls = GeoColumn::from_wkt::<&str>(&[None, None], None).unwrap();
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
    let batch = RecordBatch::try_new(
        schema,
        vec![Arc::new(Int64Array::from(vec![Some(1), None])) as ArrayRef],
    )
    .unwrap();
    write_geoparquet(&file, &nulls, "geometry", &ipc(batch)).unwrap();
    assert!(geo_json(&file)["columns"]["geometry"].get("bbox").is_none());
    assert_eq!(read_geoparquet(&file, None).unwrap().geometry.len(), 2);
}
