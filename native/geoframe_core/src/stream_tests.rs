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

// -- Feather / Arrow IPC ------------------------------------------------------------------

use crate::{read_feather, write_feather, FeatherCompression, GeometryEncoding};

fn feather_file(dir: &TempDir, rows: usize, record_batch: Option<usize>) -> String {
    let (geometry, attributes) = grid(rows, Some("EPSG:3857"));
    let file = path(dir, "s.feather");
    write_feather(
        &file,
        &geometry,
        "geometry",
        &attributes,
        GeometryEncoding::Wkb,
        FeatherCompression::Uncompressed,
    )
    .unwrap();
    if let Some(size) = record_batch {
        rechunk(&file, size);
    }
    file
}

/// Rewrites a Feather file with record batches of `size` rows (the writer emits one).
fn rechunk(file: &str, size: usize) {
    use arrow_ipc::reader::FileReader;
    use arrow_ipc::writer::FileWriter;
    let reader = FileReader::try_new(std::fs::File::open(file).unwrap(), None).unwrap();
    let schema = reader.schema();
    let batches: Vec<_> = reader.map(|b| b.unwrap()).collect();
    let mut writer = FileWriter::try_new(std::fs::File::create(file).unwrap(), &schema).unwrap();
    for batch in &batches {
        let mut offset = 0;
        while offset < batch.num_rows() {
            let length = size.min(batch.num_rows() - offset);
            writer.write(&batch.slice(offset, length)).unwrap();
            offset += length;
        }
    }
    writer.finish().unwrap();
}

#[test]
fn feather_batches_concatenate_to_the_whole_read() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = feather_file(&dir, 10, None);
    let whole = read_feather(&file, None, None).unwrap();

    let batches = drain(BatchReader::open_feather(&file, None, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 4, 2]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    for batch in &batches {
        assert_eq!(batch.geometry.crs().as_deref(), Some("EPSG:3857"));
        assert_eq!(batch.geometry_column.as_deref(), Some("geometry"));
    }
}

#[test]
fn feather_batch_size_ignores_the_record_batch_size() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = feather_file(&dir, 10, Some(3));
    // Larger than a record batch: batches are stitched together.
    let batches = drain(BatchReader::open_feather(&file, None, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 4, 2]);
    assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    // Smaller than a record batch: batches are re-sliced.
    let batches = drain(BatchReader::open_feather(&file, None, None, 2).unwrap());
    assert_eq!(sizes(&batches), vec![2, 2, 2, 2, 2]);
    let batches = drain(BatchReader::open_feather(&file, None, None, 100).unwrap());
    assert_eq!(sizes(&batches), vec![10]);
}

#[test]
fn feather_columns_projection_and_errors() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = feather_file(&dir, 5, Some(2));
    let columns = vec!["name".to_string()];
    let batches = drain(BatchReader::open_feather(&file, Some(&columns), None, 2).unwrap());
    for batch in &batches {
        assert_eq!(crate::test_support::names(&batch.attributes_ipc), ["name"]);
    }
    assert_eq!(all_wkt(&batches).len(), 5);

    let missing = vec!["nope".to_string()];
    assert!(BatchReader::open_feather(&file, Some(&missing), None, 2).is_err());
    assert!(BatchReader::open_feather(&file, None, Some("nope"), 2).is_err());
    assert!(BatchReader::open_feather(&path(&dir, "nope.feather"), None, None, 2).is_err());
    assert!(BatchReader::open_feather(&file, None, None, 0).is_err());
}

#[test]
fn feather_empty_file_yields_no_batches_and_is_counted() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = feather_file(&dir, 0, None);
    let before = open_batch_readers();
    let mut reader = BatchReader::open_feather(&file, None, None, 4).unwrap();
    assert_eq!(open_batch_readers(), before + 1);
    assert!(reader.next_batch().unwrap().is_none());
    drop(reader);
    assert_eq!(open_batch_readers(), before);
}

// -- Shapefile ----------------------------------------------------------------------------

use crate::{read_shapefile, write_shapefile};

fn shp_file(dir: &TempDir, name: &str, rows: usize) -> String {
    let (geometry, attributes) = grid(rows, Some("EPSG:3857"));
    let file = path(dir, name);
    write_shapefile(&file, &geometry, &attributes, None).unwrap();
    file
}

/// Directories this process's shapefile streams extracted a zip into and have not removed.
fn scratch_dirs() -> Vec<String> {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("geoframe-shp-"))
        .collect()
}

#[test]
fn shapefile_batches_concatenate_to_the_whole_read() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "s.shp", 10);
    let whole = read_shapefile(&file, None).unwrap();

    let batches = drain(BatchReader::open_shapefile(&file, None, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 4, 2]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    for batch in &batches {
        assert_eq!(batch.geometry.crs(), whole.geometry.crs());
        assert_eq!(batch.geometry_column, None);
    }
    let batches = drain(BatchReader::open_shapefile(&file, None, None, 100).unwrap());
    assert_eq!(sizes(&batches), vec![10]);
}

#[test]
fn shapefile_polygons_and_a_missing_dbf_stream_like_the_whole_read() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "p.shp");
    let rows = [
        Some("POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0), (1 1, 2 1, 2 2, 1 2, 1 1))"),
        None,
        Some("MULTIPOLYGON (((10 10, 10 11, 11 11, 11 10, 10 10)), ((20 20, 20 21, 21 21, 21 20, 20 20)))"),
        Some("POLYGON ((5 5, 5 6, 6 6, 6 5, 5 5))"),
    ];
    let geometry = GeoColumn::from_wkt(&rows, None).unwrap();
    let attributes = ipc(vec![(
        "id",
        Arc::new(Int64Array::from_iter_values(0..4)) as ArrayRef,
    )]);
    write_shapefile(&file, &geometry, &attributes, None).unwrap();
    let whole = read_shapefile(&file, None).unwrap();
    let batches = drain(BatchReader::open_shapefile(&file, None, None, 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 1]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_wkt(&batches)[1], None);

    std::fs::remove_file(dir.path().join("p.dbf")).unwrap();
    let whole = read_shapefile(&file, None).unwrap();
    let batches = drain(BatchReader::open_shapefile(&file, None, None, 3).unwrap());
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert!(crate::test_support::names(&batches[0].attributes_ipc).is_empty());
}

#[test]
fn shapefile_deleted_records_are_skipped_and_batches_stay_full() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "d.shp", 10);
    // Flag records 1, 2 and 6 as deleted.
    let dbf_path = dir.path().join("d.dbf");
    let mut dbf = std::fs::read(&dbf_path).unwrap();
    let header_len = u16::from_le_bytes([dbf[8], dbf[9]]) as usize;
    let record_len = u16::from_le_bytes([dbf[10], dbf[11]]) as usize;
    for row in [1, 2, 6] {
        dbf[header_len + row * record_len] = b'*';
    }
    std::fs::write(&dbf_path, dbf).unwrap();

    let whole = read_shapefile(&file, None).unwrap();
    assert_eq!(whole.geometry.len(), 7);
    let batches = drain(BatchReader::open_shapefile(&file, None, None, 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 3, 1]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), vec![0, 3, 4, 5, 7, 8, 9]);
}

#[test]
fn shapefile_bbox_filters_record_by_record() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "b.shp", 30);
    let bbox = [3.0, 0.0, 9.0, 100.0];
    let batches = drain(BatchReader::open_shapefile(&file, None, Some(bbox), 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 3, 1]);
    assert_eq!(all_ids(&batches), (3..10).collect::<Vec<i64>>());
    let none =
        drain(BatchReader::open_shapefile(&file, None, Some([500.0, 0.0, 600.0, 1.0]), 3).unwrap());
    assert!(none.is_empty());
    assert!(BatchReader::open_shapefile(&file, None, Some([5.0, 0.0, 1.0, 1.0]), 3).is_err());
}

#[test]
fn shapefile_encoding_follows_the_cpg_and_can_be_overridden() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "e.shp");
    let text = ipc(vec![(
        "t",
        Arc::new(StringArray::from(vec![Some("é"), Some("ü")])) as ArrayRef,
    )]);
    let geometry = GeoColumn::from_wkt(&[Some("POINT (0 0)"), Some("POINT (1 1)")], None).unwrap();
    write_shapefile(&file, &geometry, &text, Some("ISO-8859-1")).unwrap();

    let strings = |batches: &[StreamBatch]| -> Vec<String> {
        use arrow_array::cast::AsArray;
        batches
            .iter()
            .flat_map(|b| {
                let (_, a) = column(&b.attributes_ipc, "t");
                a.as_string::<i32>()
                    .iter()
                    .map(|s| s.unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let by_cpg = drain(BatchReader::open_shapefile(&file, None, None, 1).unwrap());
    assert_eq!(strings(&by_cpg), ["é", "ü"]);
    // An explicit encoding wins over the .cpg (the bytes are not UTF-8).
    let forced = drain(BatchReader::open_shapefile(&file, Some("UTF-8"), None, 1).unwrap());
    assert_ne!(strings(&forced), ["é", "ü"]);
    let whole = read_shapefile(&file, Some("UTF-8")).unwrap();
    let (_, a) = column(&whole.attributes_ipc, "t");
    use arrow_array::cast::AsArray;
    let whole_strings: Vec<String> = a
        .as_string::<i32>()
        .iter()
        .map(|s| s.unwrap().to_string())
        .collect();
    assert_eq!(strings(&forced), whole_strings);
    assert!(BatchReader::open_shapefile(&file, Some("KLINGON"), None, 1).is_err());
}

#[test]
fn shapefile_zip_streams_through_a_scratch_dir_removed_on_drop() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "z.zip", 10);
    let whole = read_shapefile(&file, None).unwrap();
    let before = scratch_dirs();
    let mut reader =
        BatchReader::open_shapefile(&file, None, Some([0.0, 0.0, 6.0, 100.0]), 3).unwrap();
    let first = reader.next_batch().unwrap().unwrap();
    assert_eq!(first.geometry.len(), 3);
    assert_ne!(
        scratch_dirs(),
        before,
        "the members are extracted while the reader is open"
    );
    drop(reader);
    assert_eq!(scratch_dirs(), before);

    let batches = drain(BatchReader::open_shapefile(&file, None, None, 4).unwrap());
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(
        scratch_dirs(),
        before,
        "a drained reader keeps nothing until it is dropped"
    );
    let bad = path(&dir, "bad.zip");
    std::fs::write(&bad, "not a zip").unwrap();
    assert!(BatchReader::open_shapefile(&bad, None, None, 4).is_err());
    assert_eq!(scratch_dirs(), before);
}

#[test]
fn shapefile_errors_are_reported_and_readers_are_counted() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "c.shp", 6);
    let before = open_batch_readers();
    assert!(BatchReader::open_shapefile(&path(&dir, "nope.shp"), None, None, 2).is_err());
    assert!(BatchReader::open_shapefile(&file, None, None, 0).is_err());
    assert_eq!(open_batch_readers(), before);
    let reader = BatchReader::open_shapefile(&file, None, None, 2).unwrap();
    assert_eq!(open_batch_readers(), before + 1);
    drop(reader);
    assert_eq!(open_batch_readers(), before);

    // Fewer attribute records than shapes.
    let dbf_path = dir.path().join("c.dbf");
    let mut dbf = std::fs::read(&dbf_path).unwrap();
    dbf[4..8].copy_from_slice(&3u32.to_le_bytes());
    let record_len = u16::from_le_bytes([dbf[10], dbf[11]]) as usize;
    let header_len = u16::from_le_bytes([dbf[8], dbf[9]]) as usize;
    dbf.truncate(header_len + 3 * record_len);
    std::fs::write(&dbf_path, dbf).unwrap();
    let mut reader = BatchReader::open_shapefile(&file, None, None, 2).unwrap();
    let mut outcome = Ok(());
    loop {
        match reader.next_batch() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                outcome = Err(e);
                break;
            }
        }
    }
    assert!(outcome.unwrap_err().contains("different number"));

    let junk = path(&dir, "junk.shp");
    std::fs::write(&junk, "not a shapefile").unwrap();
    assert!(BatchReader::open_shapefile(&junk, None, None, 2).is_err());
}

#[test]
fn shapefile_empty_file_yields_no_batches() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "n.shp", 0);
    let mut reader = BatchReader::open_shapefile(&file, None, None, 4).unwrap();
    assert!(reader.next_batch().unwrap().is_none());
}

// -- GeoPackage ---------------------------------------------------------------------------

use crate::{read_gpkg, write_gpkg, GpkgWriteOptions};

fn gpkg_options(spatial_index: bool) -> GpkgWriteOptions {
    GpkgWriteOptions {
        append: false,
        spatial_index,
    }
}

fn gpkg_file(dir: &TempDir, rows: usize, layer: &str, spatial_index: bool) -> String {
    let (geometry, attributes) = grid(rows, Some("EPSG:3857"));
    let file = path(dir, "s.gpkg");
    write_gpkg(
        &file,
        &geometry,
        &attributes,
        layer,
        gpkg_options(spatial_index),
    )
    .unwrap();
    file
}

fn sql(file: &str, statements: &str) {
    let conn = rusqlite::Connection::open(file).unwrap();
    // The layer's triggers call the GeoPackage SQL functions.
    crate::gpkg_io::register_functions(&conn).unwrap();
    conn.execute_batch(statements).unwrap();
}

/// The dtypes of the attribute columns of every batch (they must agree to concatenate).
fn batch_types(batches: &[StreamBatch]) -> Vec<Vec<(String, arrow_schema::DataType)>> {
    batches
        .iter()
        .map(|b| {
            crate::test_support::columns(&b.attributes_ipc)
                .into_iter()
                .map(|(name, data_type, _)| (name, data_type))
                .collect()
        })
        .collect()
}

#[test]
fn gpkg_batches_concatenate_to_the_whole_read() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 10, "pts", true);
    let whole = read_gpkg(&file, None).unwrap().vector;

    let batches = drain(BatchReader::open_gpkg(&file, None, None, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 4, 2]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    for batch in &batches {
        assert_eq!(batch.geometry.crs(), whole.geometry.crs());
        assert_eq!(batch.geometry_column, None);
    }
    let whole_types = batch_types(&[StreamBatch {
        geometry: whole.geometry,
        geometry_column: None,
        secondary_geometries: Vec::new(),
        attributes_ipc: whole.attributes_ipc,
    }]);
    assert!(batch_types(&batches).iter().all(|t| *t == whole_types[0]));
    let batches = drain(BatchReader::open_gpkg(&file, None, None, None, 100).unwrap());
    assert_eq!(sizes(&batches), vec![10]);
}

#[test]
fn gpkg_follows_the_feature_id_across_gaps_and_keeps_null_geometries() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 12, "pts", true);
    sql(
        &file,
        "DELETE FROM pts WHERE fid IN (2, 3, 7, 12); UPDATE pts SET geom = NULL WHERE fid = 5;",
    );
    let whole = read_gpkg(&file, None).unwrap().vector;
    assert_eq!(whole.geometry.len(), 8);
    let batches = drain(BatchReader::open_gpkg(&file, None, None, None, 3).unwrap());
    assert_eq!(sizes(&batches), vec![3, 3, 2]);
    assert_eq!(all_wkt(&batches), wkt(&whole.geometry));
    assert!(all_wkt(&batches).contains(&None));
}

#[test]
fn gpkg_column_types_do_not_change_between_batches() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 8, "pts", true);
    // Integers up to the last row, which no longer fits the declared 32 bits, and an untyped
    // column that holds integers first and a real last: the whole read widens both.
    sql(
        &file,
        "ALTER TABLE pts ADD COLUMN loose NUMERIC; ALTER TABLE pts ADD COLUMN narrow MEDIUMINT;
         UPDATE pts SET loose = fid, narrow = fid;
         UPDATE pts SET loose = 1.5, narrow = 5000000000 WHERE fid = 8;",
    );
    let whole = read_gpkg(&file, None).unwrap().vector;
    let (loose, _) = column(&whole.attributes_ipc, "loose");
    let (narrow, _) = column(&whole.attributes_ipc, "narrow");
    assert_eq!(loose, arrow_schema::DataType::Float64);
    assert_eq!(narrow, arrow_schema::DataType::Int64);

    let batches = drain(BatchReader::open_gpkg(&file, None, None, None, 3).unwrap());
    for batch in &batches {
        assert_eq!(column(&batch.attributes_ipc, "loose").0, loose);
        assert_eq!(column(&batch.attributes_ipc, "narrow").0, narrow);
    }
    assert_eq!(batch_types(&batches).len(), 3);
}

#[test]
fn gpkg_layer_selection() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 4, "first", true);
    let (geometry, attributes) = grid(6, Some("EPSG:4326"));
    write_gpkg(&file, &geometry, &attributes, "second", gpkg_options(true)).unwrap();

    assert!(BatchReader::open_gpkg(&file, None, None, None, 2).is_err());
    assert!(BatchReader::open_gpkg(&file, Some("nope"), None, None, 2).is_err());
    let batches = drain(BatchReader::open_gpkg(&file, Some("second"), None, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 2]);
    assert_eq!(batches[0].geometry.crs().as_deref(), Some("EPSG:4326"));
    let batches = drain(BatchReader::open_gpkg(&file, Some("first"), None, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4]);
}

#[test]
fn gpkg_columns_projection() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 5, "pts", true);
    let columns = vec!["name".to_string()];
    let batches = drain(BatchReader::open_gpkg(&file, None, Some(&columns), None, 2).unwrap());
    assert_eq!(sizes(&batches), vec![2, 2, 1]);
    for batch in &batches {
        assert_eq!(crate::test_support::names(&batch.attributes_ipc), ["name"]);
    }
    let missing = vec!["nope".to_string()];
    assert!(BatchReader::open_gpkg(&file, None, Some(&missing), None, 2).is_err());
    let none: Vec<String> = Vec::new();
    let batches = drain(BatchReader::open_gpkg(&file, None, Some(&none), None, 2).unwrap());
    assert!(crate::test_support::names(&batches[0].attributes_ipc).is_empty());
}

#[test]
fn gpkg_bbox_with_and_without_an_rtree() {
    let _guard = serial();
    for indexed in [true, false] {
        let dir = TempDir::new().unwrap();
        let file = gpkg_file(&dir, 30, "pts", indexed);
        let bbox = [3.0, 0.0, 9.0, 100.0];
        let batches = drain(BatchReader::open_gpkg(&file, None, None, Some(bbox), 3).unwrap());
        assert_eq!(sizes(&batches), vec![3, 3, 1], "indexed: {indexed}");
        assert_eq!(all_ids(&batches), (3..10).collect::<Vec<i64>>());
        let none = [500.0, 0.0, 600.0, 1.0];
        assert!(
            drain(BatchReader::open_gpkg(&file, None, None, Some(none), 3).unwrap()).is_empty()
        );
    }
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 5, "pts", true);
    assert!(BatchReader::open_gpkg(&file, None, None, Some([5.0, 0.0, 1.0, 1.0]), 2).is_err());
}

#[test]
fn gpkg_bbox_is_answered_by_the_rtree_when_the_layer_has_one() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 20, "pts", true);
    // Emptying the index (not the table) changes the answer only if the index is consulted.
    sql(&file, "DELETE FROM rtree_pts_geom WHERE id <= 10");
    let bbox = [3.0, 0.0, 15.0, 100.0];
    let batches = drain(BatchReader::open_gpkg(&file, None, None, Some(bbox), 4).unwrap());
    assert_eq!(all_ids(&batches), (10..16).collect::<Vec<i64>>());
    // Without a bbox the table is read whole, index or not.
    let batches = drain(BatchReader::open_gpkg(&file, None, None, None, 4).unwrap());
    assert_eq!(all_ids(&batches), (0..20).collect::<Vec<i64>>());
}

#[test]
fn gpkg_empty_layer_errors_and_counting() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let empty = gpkg_file(&dir, 0, "pts", true);
    let before = open_batch_readers();
    let mut reader = BatchReader::open_gpkg(&empty, None, None, None, 4).unwrap();
    assert_eq!(open_batch_readers(), before + 1);
    assert!(reader.next_batch().unwrap().is_none());
    drop(reader);
    assert_eq!(open_batch_readers(), before);

    assert!(BatchReader::open_gpkg(&path(&dir, "nope.gpkg"), None, None, None, 2).is_err());
    assert!(BatchReader::open_gpkg(&empty, None, None, None, 0).is_err());
    let junk = path(&dir, "junk.gpkg");
    std::fs::write(&junk, "not sqlite at all, not sqlite at all").unwrap();
    assert!(BatchReader::open_gpkg(&junk, None, None, None, 2).is_err());
    assert_eq!(open_batch_readers(), before);
}

// -- skip_rows ----------------------------------------------------------------------------

/// Skipping `k` rows then draining gives the rows of a plain drain from `k` on, in batches of
/// `batch` rows (the last one shorter), for every `k` up to past the end.
fn check_skip(open: impl Fn() -> BatchReader, batch: usize) {
    let whole = all_wkt(&drain(open()));
    let total = whole.len();
    for k in [0, 1, 3, 5, 7, total.saturating_sub(1), total, total + 5] {
        let mut reader = open();
        reader.skip_rows(k).unwrap();
        let batches = drain(reader);
        assert_eq!(
            all_wkt(&batches),
            whole[k.min(total)..].to_vec(),
            "rows after skipping {k}"
        );
        let sizes = sizes(&batches);
        if let Some((last, full)) = sizes.split_last() {
            assert!(full.iter().all(|s| *s == batch), "batch sizes {sizes:?}");
            assert!(*last >= 1 && *last <= batch, "batch sizes {sizes:?}");
        }
    }
}

#[test]
fn parquet_skip_rows_skips_row_groups() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    for covering in [false, true] {
        let file = parquet_file(&dir, 23, 4, covering);
        check_skip(
            || BatchReader::open_geoparquet(&file, None, None, 3).unwrap(),
            3,
        );
        // With a bbox (pushed down with a covering column, else filtered after decoding),
        // the rows are counted after the filter.
        check_skip(
            || BatchReader::open_geoparquet(&file, None, Some([2.0, 0.0, 17.0, 100.0]), 3).unwrap(),
            3,
        );
    }
}

#[test]
fn flatgeobuf_skip_rows() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = fgb_file(&dir, 23, true);
    check_skip(|| BatchReader::open_flatgeobuf(&file, None, 4).unwrap(), 4);
    check_skip(
        || BatchReader::open_flatgeobuf(&file, Some([2.0, 0.0, 17.0, 100.0]), 4).unwrap(),
        4,
    );
}

#[test]
fn feather_skip_rows_across_record_batches() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = feather_file(&dir, 23, Some(5));
    check_skip(
        || BatchReader::open_feather(&file, None, None, 4).unwrap(),
        4,
    );
}

#[test]
fn shapefile_skip_rows() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = shp_file(&dir, "s.shp", 23);
    check_skip(
        || BatchReader::open_shapefile(&file, None, None, 4).unwrap(),
        4,
    );
    check_skip(
        || BatchReader::open_shapefile(&file, None, Some([2.0, 0.0, 17.0, 100.0]), 4).unwrap(),
        4,
    );
}

#[test]
fn gpkg_skip_rows() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let file = gpkg_file(&dir, 23, "pts", true);
    check_skip(
        || BatchReader::open_gpkg(&file, None, None, None, 4).unwrap(),
        4,
    );
    check_skip(
        || BatchReader::open_gpkg(&file, None, None, Some([2.0, 0.0, 17.0, 100.0]), 4).unwrap(),
        4,
    );
}

#[test]
fn geojson_skip_rows() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let features: Vec<String> = (0..23)
        .map(|i| {
            format!(
                r#"{{"type":"Feature","properties":{{"n":{i}}},"geometry":{{"type":"Point","coordinates":[{i},{}]}}}}"#,
                i * 2
            )
        })
        .collect();
    let file = path(&dir, "s.geojson");
    std::fs::write(
        &file,
        format!(
            r#"{{"type":"FeatureCollection","features":[{}]}}"#,
            features.join(",")
        ),
    )
    .unwrap();
    check_skip(|| BatchReader::open_geojson(&file, None, 4).unwrap(), 4);
    check_skip(
        || BatchReader::open_geojson(&file, Some([2.0, 0.0, 17.0, 100.0]), 4).unwrap(),
        4,
    );
}

// -- Record batch size of the Feather / Arrow IPC writers ---------------------------------

use crate::{
    from_arrow_ipc, to_arrow_ipc_multi_batched, write_feather_multi_batched,
    DEFAULT_RECORD_BATCH_SIZE,
};

fn record_batch_sizes(file: &str) -> Vec<usize> {
    use arrow_ipc::reader::FileReader;
    let reader = FileReader::try_new(std::fs::File::open(file).unwrap(), None).unwrap();
    reader.map(|b| b.unwrap().num_rows()).collect()
}

#[test]
fn feather_writer_emits_record_batches_of_the_requested_size() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let (geometry, attributes) = grid(10, Some("EPSG:3857"));
    let file = path(&dir, "b.feather");
    for (size, expected) in [
        (3, vec![3, 3, 3, 1]),
        (10, vec![10]),
        (100, vec![10]),
        (1, vec![1; 10]),
    ] {
        write_feather_multi_batched(
            &file,
            &[("geometry", &geometry)],
            &attributes,
            GeometryEncoding::Wkb,
            FeatherCompression::Lz4,
            size,
        )
        .unwrap();
        assert_eq!(record_batch_sizes(&file), expected, "size {size}");
        // Same rows whatever the batching, and the streaming reader agrees.
        let whole = read_feather(&file, None, None).unwrap();
        assert_eq!(wkt(&whole.geometry), wkt(&geometry), "size {size}");
        let batches = drain(BatchReader::open_feather(&file, None, None, 4).unwrap());
        assert_eq!(all_ids(&batches), (0..10).collect::<Vec<i64>>());
    }
}

#[test]
fn feather_writer_rejects_a_zero_record_batch_size_and_leaves_no_file() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let (geometry, attributes) = grid(4, None);
    let file = path(&dir, "z.feather");
    let err = write_feather_multi_batched(
        &file,
        &[("geometry", &geometry)],
        &attributes,
        GeometryEncoding::Wkb,
        FeatherCompression::Uncompressed,
        0,
    )
    .unwrap_err();
    assert!(err.contains("record_batch_size"), "{err}");
    assert!(!std::path::Path::new(&file).exists());
}

#[test]
fn an_empty_table_is_one_empty_record_batch_or_none_but_reads_back() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let (geometry, attributes) = grid(0, Some("EPSG:3857"));
    let file = path(&dir, "e.feather");
    write_feather_multi_batched(
        &file,
        &[("geometry", &geometry)],
        &attributes,
        GeometryEncoding::Wkb,
        FeatherCompression::Uncompressed,
        5,
    )
    .unwrap();
    let read = read_feather(&file, None, None).unwrap();
    assert_eq!(read.geometry.len(), 0);
}

#[test]
fn arrow_stream_writer_emits_record_batches_of_the_requested_size() {
    let _guard = serial();
    let (geometry, attributes) = grid(10, Some("EPSG:4326"));
    let bytes = to_arrow_ipc_multi_batched(
        &[("geometry", &geometry)],
        &attributes,
        GeometryEncoding::Wkb,
        4,
    )
    .unwrap();
    let reader = arrow_ipc::reader::StreamReader::try_new(bytes.as_slice(), None).unwrap();
    let sizes: Vec<usize> = reader.map(|b| b.unwrap().num_rows()).collect();
    assert_eq!(sizes, vec![4, 4, 2]);
    let read = from_arrow_ipc(&bytes, None, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&geometry));
}

#[test]
fn the_default_record_batch_size_matches_pyarrow() {
    assert_eq!(DEFAULT_RECORD_BATCH_SIZE, 65_536);
}
