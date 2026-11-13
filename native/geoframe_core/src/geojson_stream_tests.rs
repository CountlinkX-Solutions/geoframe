use std::sync::Mutex;

use arrow_schema::DataType;
use tempfile::TempDir;

use crate::test_support::{columns, names, path, wkt};
use crate::{open_batch_readers, read_geojson, BatchReader, StreamBatch};

/// The open-reader counter is process wide: tests that read it run one at a time.
static COUNTER: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    COUNTER.lock().unwrap_or_else(|e| e.into_inner())
}

fn file(dir: &TempDir, text: &str) -> String {
    let file = path(dir, "s.geojson");
    std::fs::write(&file, text).unwrap();
    file
}

fn point(i: usize, properties: &str) -> String {
    format!(
        r#"{{"type":"Feature","properties":{properties},"geometry":{{"type":"Point","coordinates":[{i},{}]}}}}"#,
        i * 2
    )
}

fn collection(features: &[String]) -> String {
    format!(
        r#"{{"type":"FeatureCollection","features":[{}]}}"#,
        features.join(",")
    )
}

fn drain(mut reader: BatchReader) -> Vec<StreamBatch> {
    let mut batches = Vec::new();
    while let Some(batch) = reader.next_batch().unwrap() {
        batches.push(batch);
    }
    batches
}

fn types(ipc: &[u8]) -> Vec<(String, DataType)> {
    columns(ipc).into_iter().map(|(n, t, _)| (n, t)).collect()
}

fn cells(ipc: &[u8], name: &str) -> Vec<String> {
    let (_, array) = crate::test_support::column(ipc, name);
    (0..array.len())
        .map(|i| {
            if array.is_null(i) {
                "null".to_string()
            } else {
                arrow_cast::display::array_value_to_string(&array, i).unwrap()
            }
        })
        .collect()
}

fn sizes(batches: &[StreamBatch]) -> Vec<usize> {
    batches.iter().map(|b| b.geometry.len()).collect()
}

#[test]
fn batches_concatenate_to_the_whole_read_with_its_types() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    // `n` is an integer until the last feature, which makes it a float; `s` is text; `late`
    // only appears in the last feature; `flag` is a boolean with a null.
    let mut features: Vec<String> = (0..9)
        .map(|i| point(i, &format!(r#"{{"n":{i},"s":"a{i}","flag":true}}"#)))
        .collect();
    features.push(point(
        9,
        r#"{"n":2.5,"s":"z","late":"x","flag":null,"nested":{"a":[1,2]}}"#,
    ));
    let file = file(&dir, &collection(&features));
    let whole = read_geojson(&file).unwrap();
    let whole_types = types(&whole.attributes_ipc);
    assert!(whole_types.contains(&("n".to_string(), DataType::Float64)));

    let batches = drain(BatchReader::open_geojson(&file, None, 4).unwrap());
    assert_eq!(sizes(&batches), vec![4, 4, 2]);
    for batch in &batches {
        assert_eq!(types(&batch.attributes_ipc), whole_types);
        assert_eq!(batch.geometry.crs().as_deref(), Some("OGC:CRS84"));
        assert_eq!(batch.geometry_column, None);
    }
    let all_wkt: Vec<_> = batches.iter().flat_map(|b| wkt(&b.geometry)).collect();
    assert_eq!(all_wkt, wkt(&whole.geometry));
    for name in names(&whole.attributes_ipc) {
        let joined: Vec<String> = batches
            .iter()
            .flat_map(|b| cells(&b.attributes_ipc, &name))
            .collect();
        assert_eq!(joined, cells(&whole.attributes_ipc, &name), "{name}");
    }
}

#[test]
fn feature_ids_follow_the_whole_read_rule() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let with_id = |i: usize, id: &str| {
        format!(r#"{{"type":"Feature","id":{id},"properties":{{"v":{i}}},"geometry":null}}"#)
    };
    // Every feature has an id: a leading `id` column, a float because of the last one.
    let all = collection(&[with_id(0, "1"), with_id(1, "2"), with_id(2, "3.5")]);
    let f = file(&dir, &all);
    let whole = read_geojson(&f).unwrap();
    let batches = drain(BatchReader::open_geojson(&f, None, 2).unwrap());
    assert_eq!(
        types(&batches[0].attributes_ipc),
        types(&whole.attributes_ipc)
    );
    assert_eq!(names(&batches[0].attributes_ipc), ["id", "v"]);

    // One feature without an id: no `id` column at all.
    let some = collection(&[with_id(0, "1"), point(1, r#"{"v":1}"#)]);
    let f = file(&dir, &some);
    let whole = read_geojson(&f).unwrap();
    let batches = drain(BatchReader::open_geojson(&f, None, 1).unwrap());
    assert_eq!(names(&whole.attributes_ipc), ["v"]);
    assert_eq!(names(&batches[0].attributes_ipc), ["v"]);
}

#[test]
fn foreign_members_key_order_and_strings_with_braces_are_handled() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let f = file(
        &dir,
        &format!(
            r#"{{ "name": "a ] \" [ {{", "features": [ {} , {} ],
                 "crs": {{"type":"name","properties":{{"name":"x"}}}}, "type": "FeatureCollection" }}"#,
            point(0, r#"{"t":"} ] \" {"}"#),
            point(1, r#"{"t":"b"}"#),
        ),
    );
    let whole = read_geojson(&f).unwrap();
    let batches = drain(BatchReader::open_geojson(&f, None, 1).unwrap());
    assert_eq!(sizes(&batches), vec![1, 1]);
    let all: Vec<String> = batches
        .iter()
        .flat_map(|b| cells(&b.attributes_ipc, "t"))
        .collect();
    assert_eq!(all, cells(&whole.attributes_ipc, "t"));
}

#[test]
fn a_lone_feature_and_a_bare_geometry_are_one_row_files() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    for text in [
        point(3, r#"{"k":1}"#),
        r#"{"type":"Point","coordinates":[1,2]}"#.to_string(),
    ] {
        let f = file(&dir, &text);
        let whole = read_geojson(&f).unwrap();
        let batches = drain(BatchReader::open_geojson(&f, None, 5).unwrap());
        assert_eq!(sizes(&batches), vec![1]);
        assert_eq!(wkt(&batches[0].geometry), wkt(&whole.geometry));
        assert_eq!(
            types(&batches[0].attributes_ipc),
            types(&whole.attributes_ipc)
        );
    }
}

#[test]
fn bbox_filters_per_feature_and_batches_are_refilled() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let mut features: Vec<String> = (0..10)
        .map(|i| point(i, &format!(r#"{{"n":{i}}}"#)))
        .collect();
    features.push(r#"{"type":"Feature","properties":{"n":99.5},"geometry":null}"#.to_string());
    let f = file(&dir, &collection(&features));
    let whole = read_geojson(&f).unwrap();
    // Points (i, 2i): 2..=6 are inside x in [2, 6].
    let batches = drain(BatchReader::open_geojson(&f, Some([2.0, 0.0, 6.0, 100.0]), 2).unwrap());
    assert_eq!(sizes(&batches), vec![2, 2, 1]);
    // The types still come from the whole file (the late float), not from the kept rows.
    for batch in &batches {
        assert_eq!(types(&batch.attributes_ipc), types(&whole.attributes_ipc));
    }
    let n: Vec<String> = batches
        .iter()
        .flat_map(|b| cells(&b.attributes_ipc, "n"))
        .collect();
    assert_eq!(n, ["2.0", "3.0", "4.0", "5.0", "6.0"]);
}

#[test]
fn empty_collection_yields_no_batches_and_errors_are_reported() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let f = file(&dir, &collection(&[]));
    assert!(drain(BatchReader::open_geojson(&f, None, 3).unwrap()).is_empty());

    assert!(BatchReader::open_geojson(&path(&dir, "missing.geojson"), None, 3).is_err());
    assert!(BatchReader::open_geojson(&f, None, 0).is_err());
    let bad = file(
        &dir,
        r#"{"type":"FeatureCollection","features":[{"type":"Feature""#,
    );
    assert!(BatchReader::open_geojson(&bad, None, 3).is_err());
    let bad = file(&dir, "not json");
    assert!(BatchReader::open_geojson(&bad, None, 3).is_err());
}

#[test]
fn a_reader_is_counted_until_dropped() {
    let _guard = serial();
    let dir = TempDir::new().unwrap();
    let f = file(&dir, &collection(&[point(0, "{}"), point(1, "{}")]));
    let before = open_batch_readers();
    let mut reader = BatchReader::open_geojson(&f, None, 1).unwrap();
    assert_eq!(open_batch_readers(), before + 1);
    assert!(reader.next_batch().unwrap().is_some());
    drop(reader);
    assert_eq!(open_batch_readers(), before);
}
