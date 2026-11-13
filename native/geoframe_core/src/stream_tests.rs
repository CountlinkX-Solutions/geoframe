use std::sync::{Arc, Mutex};

use arrow_array::{ArrayRef, Int64Array, StringArray};
use tempfile::TempDir;

use crate::parquet_io::ParquetOptions;
use crate::test_support::{column, ipc, path, wkt};
use crate::{
    open_batch_readers, read_flatgeobuf, read_geoparquet, read_geoparquet_filtered,
    write_flatgeobuf, write_geoparquet_with, BatchReader, GeoColumn, StreamBatch,
};

/// The open-reader counter is process wide, so the tests that open readers run one at a time.
static COUNTER: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    COUNTER.lock().unwrap_or_else(|e| e.into_inner())
}

fn grid(rows: usize, crs: Option<&str>) -> (GeoColumn, Vec<u8>) {
    let wkt: Vec<Option<String>> = (0..rows)
        .map(|i| Some(format!("POINT ({} {})", i, i * 2)))
        .collect();
    let refs: Vec<Option<&str>> = wkt.iter().map(|w| w.as_deref()).collect();
    let attributes = ipc(vec![
        (
            "id",
            Arc::new(Int64Array::from_iter_values(0..rows as i64)) as ArrayRef,
        ),
        (
            "name",
            Arc::new(StringArray::from_iter(
                (0..rows).map(|i| Some(format!("n{i}"))),
            )),
        ),
    ]);
    (GeoColumn::from_wkt(&refs, crs).unwrap(), attributes)
}

fn parquet_file(dir: &TempDir, rows: usize, row_group: usize, covering: bool) -> String {
    let (geometry, attributes) = grid(rows, Some("EPSG:3857"));
    let file = path(dir, "s.parquet");
    let options = ParquetOptions {
        row_group_size: Some(row_group),
        covering_bbox: covering,
        ..Default::default()
    };
    write_geoparquet_with(&file, &geometry, "geometry", &attributes, &options).unwrap();
    file
}

fn fgb_file(dir: &TempDir, rows: usize, index: bool) -> String {
    let (geometry, attributes) = grid(rows, Some("EPSG:3857"));
    let file = path(dir, "s.fgb");
    write_flatgeobuf(&file, &geometry, &attributes, "s", index).unwrap();
    file
}

fn drain(mut reader: BatchReader) -> Vec<StreamBatch> {
    let mut batches = Vec::new();
    while let Some(batch) = reader.next_batch().unwrap() {
        batches.push(batch);
    }
    batches
}

fn sizes(batches: &[StreamBatch]) -> Vec<usize> {
    batches.iter().map(|b| b.geometry.len()).collect()
}

fn all_wkt(batches: &[StreamBatch]) -> Vec<Option<String>> {
    batches.iter().flat_map(|b| wkt(&b.geometry)).collect()
}

fn all_ids(batches: &[StreamBatch]) -> Vec<i64> {
    use arrow_array::cast::AsArray;
    use arrow_array::types::Int64Type;
    batches
        .iter()
        .flat_map(|b| {
            let (_, array) = column(&b.attributes_ipc, "id");
            array.as_primitive::<Int64Type>().values().to_vec()
        })
        .collect()
}

#[test]
fn parquet_batches_concatenate_to_the_whole_read() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = parquet_file(&dir, 10, 4, false);
    let whole = read_geoparquet(&file, None).unwrap();

    let batches = drain(BatchReader::open_geoparquet(&file, None, None, 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 3, 3, 1]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    for batch in &batches {
        assert_eq!(batch.geometry.crs().as_deref(), Some("EPSG:3857"));
        assert_eq!(batch.geometry_column.as_deref(), Some("geometry"));
    }
}

#[test]
fn parquet_batch_size_ignores_row_group_size() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = parquet_file(&dir, 10, 4, false);
    let batches = drain(BatchReader::open_geoparquet(&file, None, None, 6).unwrap());
    assert_eq!(sizes(&batches), vec![6, 4]);
    let batches = drain(BatchReader::open_geoparquet(&file, None, None, 100).unwrap());
    assert_eq!(sizes(&batches), vec![10]);
}

#[test]
fn parquet_columns_projection() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = parquet_file(&dir, 5, 2, false);
    let columns = vec!["name".to_string()];
    let batches = drain(BatchReader::open_geoparquet(&file, Some(&columns), None, 2).unwrap());
    for batch in &batches {
        let (_, array) = column(&batch.attributes_ipc, "name");
        assert!(!array.is_empty());
        assert_eq!(crate::test_support::names(&batch.attributes_ipc), ["name"]);
    }
    assert_eq!(all_wkt(&batches).len(), 5);
}

fn parquet_bbox_matches(covering: bool) {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = parquet_file(&dir, 20, 4, covering);
    let bbox = [3.0, 0.0, 9.0, 100.0];
    let whole = read_geoparquet_filtered(&file, None, Some(bbox)).unwrap();
    assert_eq!(whole.geometry.len(), 7);
    let batches = drain(BatchReader::open_geoparquet(&file, None, Some(bbox), 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 3, 1]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), (3..10).collect::<Vec<i64>>());
}

#[test]
fn parquet_bbox_with_covering_column() {
    parquet_bbox_matches(true);
}

#[test]
fn parquet_bbox_without_covering_column() {
    parquet_bbox_matches(false);
}

#[test]
fn parquet_invalid_bbox_and_missing_file_fail_on_open() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = parquet_file(&dir, 3, 2, false);
    assert!(BatchReader::open_geoparquet(&file, None, Some([5.0, 0.0, 1.0, 1.0]), 2).is_err());
    assert!(BatchReader::open_geoparquet(&path(&dir, "nope.parquet"), None, None, 2).is_err());
    assert!(BatchReader::open_geoparquet(&file, None, None, 0).is_err());
}

#[test]
fn flatgeobuf_batches_concatenate_to_the_whole_read() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = fgb_file(&dir, 10, false);
    let whole = read_flatgeobuf(&file, None).unwrap();

    let batches = drain(BatchReader::open_flatgeobuf(&file, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 4, 2]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    for batch in &batches {
        assert_eq!(batch.geometry.crs().as_deref(), Some("EPSG:3857"));
        assert_eq!(batch.geometry_column, None);
    }
}

#[test]
fn flatgeobuf_hilbert_ordered_file_streams_in_file_order() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = fgb_file(&dir, 50, true);
    let whole = read_flatgeobuf(&file, None).unwrap();
    let batches = drain(BatchReader::open_flatgeobuf(&file, None, 7).unwrap());
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
}

#[test]
fn flatgeobuf_bbox_uses_the_index() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = fgb_file(&dir, 30, true);
    let bbox = [3.0, 0.0, 9.0, 100.0];
    let whole = read_flatgeobuf(&file, Some(bbox)).unwrap();
    assert_eq!(whole.geometry.len(), 7);
    let batches = drain(BatchReader::open_flatgeobuf(&file, Some(bbox), 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 3, 1]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));

    let unindexed = fgb_file(&dir, 5, false);
    assert!(BatchReader::open_flatgeobuf(&unindexed, Some(bbox), 3).is_err());
}

#[test]
fn flatgeobuf_empty_file_yields_no_batches() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = fgb_file(&dir, 0, true);
    let mut reader = BatchReader::open_flatgeobuf(&file, None, 4).unwrap();
    assert!(reader.next_batch().unwrap().is_none());
}

#[test]
fn readers_are_counted_until_dropped() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let parquet = parquet_file(&dir, 5, 2, false);
    let fgb = fgb_file(&dir, 5, true);
    let before = open_batch_readers();
    let a = BatchReader::open_geoparquet(&parquet, None, None, 2).unwrap();
    let b = BatchReader::open_flatgeobuf(&fgb, None, 2).unwrap();
    assert_eq!(open_batch_readers(), before + 2);
    drop(a);
    assert_eq!(open_batch_readers(), before + 1);
    drop(b);
    assert_eq!(open_batch_readers(), before);
    // A failed open leaves nothing behind.
    let _ = BatchReader::open_geoparquet(&path(&dir, "nope.parquet"), None, None, 2);
    assert_eq!(open_batch_readers(), before);
}
