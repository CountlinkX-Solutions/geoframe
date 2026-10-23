use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, ListArray, StringArray,
};
use arrow_schema::DataType;
use tempfile::TempDir;

use crate::test_support::{column, columns, ipc, names, path, wkt};
use crate::{read_geojson, write_geojson, GeoColumn};

fn write_text(dir: &TempDir, name: &str, text: &str) -> String {
    let file = path(dir, name);
    std::fs::write(&file, text).unwrap();
    file
}

const MIXED: &str = r#"{
  "type": "FeatureCollection",
  "features": [
    {"type": "Feature", "geometry": {"type": "Point", "coordinates": [1, 2]},
     "properties": {"n": 1, "f": 1, "m": "x", "b": true, "nest": {"k": 1}, "gone": null}},
    {"type": "Feature", "geometry": null,
     "properties": {"n": null, "f": 2.5, "m": 7, "b": false, "nest": [1, 2], "gone": null}},
    {"type": "Feature", "geometry": {"type": "Polygon", "coordinates": [[[0,0],[4,0],[4,3],[0,3],[0,0]]]},
     "properties": {"n": 3, "extra": "late"}}
  ]
}"#;

#[test]
fn reads_geometry_and_crs() {
    let dir = TempDir::new().unwrap();
    let read = read_geojson(&write_text(&dir, "a.geojson", MIXED)).unwrap();
    assert_eq!(
        wkt(&read.geometry),
        vec![
            Some("POINT(1 2)".to_string()),
            None,
            Some("POLYGON((0 0,4 0,4 3,0 3,0 0))".to_string())
        ]
    );
    assert_eq!(read.geometry.crs().as_deref(), Some("OGC:CRS84"));
}

#[test]
fn infers_column_types_across_features() {
    let dir = TempDir::new().unwrap();
    let read = read_geojson(&write_text(&dir, "a.geojson", MIXED)).unwrap();
    let bytes = &read.attributes_ipc;
    // first-seen order, properties missing from a feature are null
    assert_eq!(names(bytes), ["n", "f", "m", "b", "nest", "gone", "extra"]);

    let (t, a) = column(bytes, "n");
    assert_eq!(t, DataType::Int64);
    let n: Vec<Option<i64>> = a.as_primitive::<Int64Type>().iter().collect();
    assert_eq!(n, [Some(1), None, Some(3)]);

    // int + float widens to float
    let (t, a) = column(bytes, "f");
    assert_eq!(t, DataType::Float64);
    let f: Vec<Option<f64>> = a.as_primitive::<Float64Type>().iter().collect();
    assert_eq!(f, [Some(1.0), Some(2.5), None]);

    // string + int falls back to string
    let (t, a) = column(bytes, "m");
    assert_eq!(t, DataType::Utf8);
    let m: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(m, [Some("x"), Some("7"), None]);

    let (t, a) = column(bytes, "b");
    assert_eq!(t, DataType::Boolean);
    let b: Vec<Option<bool>> = a.as_boolean().iter().collect();
    assert_eq!(b, [Some(true), Some(false), None]);

    // nested values become JSON text
    let (t, a) = column(bytes, "nest");
    assert_eq!(t, DataType::Utf8);
    let nest: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(nest, [Some(r#"{"k":1}"#), Some("[1,2]"), None]);

    // an all-null column is kept as an all-null string column
    let (t, a) = column(bytes, "gone");
    assert_eq!(t, DataType::Utf8);
    assert_eq!(a.null_count(), 3);
}

#[test]
fn feature_id_becomes_id_column_only_when_every_feature_has_one() {
    let dir = TempDir::new().unwrap();
    let all = r#"{"type":"FeatureCollection","features":[
      {"type":"Feature","id":10,"geometry":null,"properties":{"a":1}},
      {"type":"Feature","id":"x","geometry":null,"properties":{"a":2}}]}"#;
    let read = read_geojson(&write_text(&dir, "all.geojson", all)).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["id", "a"]);
    let (t, a) = column(&read.attributes_ipc, "id");
    assert_eq!(t, DataType::Utf8);
    let ids: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(ids, [Some("10"), Some("x")]);

    let some = r#"{"type":"FeatureCollection","features":[
      {"type":"Feature","id":1,"geometry":null,"properties":{"a":1}},
      {"type":"Feature","geometry":null,"properties":{"a":2}}]}"#;
    let read = read_geojson(&write_text(&dir, "some.geojson", some)).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["a"]);

    let clash = r#"{"type":"FeatureCollection","features":[
      {"type":"Feature","id":1,"geometry":null,"properties":{"id":"mine"}}]}"#;
    let read = read_geojson(&write_text(&dir, "clash.geojson", clash)).unwrap();
    let (_, a) = column(&read.attributes_ipc, "id");
    assert_eq!(a.as_string::<i32>().value(0), "mine");
}

#[test]
fn accepts_a_single_feature_and_a_bare_geometry() {
    let dir = TempDir::new().unwrap();
    let feature = r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[5,6]},"properties":{"a":1}}"#;
    let read = read_geojson(&write_text(&dir, "f.geojson", feature)).unwrap();
    assert_eq!(wkt(&read.geometry), vec![Some("POINT(5 6)".to_string())]);
    assert_eq!(names(&read.attributes_ipc), ["a"]);

    let geometry = r#"{"type":"Point","coordinates":[5,6]}"#;
    let read = read_geojson(&write_text(&dir, "g.geojson", geometry)).unwrap();
    assert_eq!(read.geometry.len(), 1);
    assert!(columns(&read.attributes_ipc).is_empty());
}

#[test]
fn empty_collection_reads_as_zero_rows() {
    let dir = TempDir::new().unwrap();
    let read = read_geojson(&write_text(
        &dir,
        "e.geojson",
        r#"{"type":"FeatureCollection","features":[]}"#,
    ))
    .unwrap();
    assert_eq!(read.geometry.len(), 0);
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    let dir = TempDir::new().unwrap();
    for (name, text) in [
        ("junk.geojson", "not json at all"),
        (
            "truncated.geojson",
            r#"{"type":"FeatureCollection","features":[{"#,
        ),
        ("other.geojson", r#"{"type":"Topology"}"#),
        ("empty.geojson", ""),
    ] {
        let err = read_geojson(&write_text(&dir, name, text)).err();
        assert!(err.is_some(), "{name} should fail");
    }
    let err = read_geojson(&path(&dir, "missing.geojson")).err().unwrap();
    assert!(err.contains("missing.geojson"), "{err}");
}

fn sample_ipc() -> Vec<u8> {
    ipc(vec![
        (
            "id",
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
        ),
        (
            "name",
            Arc::new(StringArray::from(vec![Some("a"), Some("b"), None])),
        ),
        (
            "score",
            Arc::new(Float64Array::from(vec![Some(1.5), Some(2.0), None])),
        ),
        (
            "active",
            Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
        ),
    ])
}

fn sample_geometry(crs: Option<&str>) -> GeoColumn {
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

#[test]
fn round_trip_preserves_attributes_geometry_and_column_order() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "rt.geojson");
    let geometry = sample_geometry(Some("OGC:CRS84"));
    write_geojson(&file, &geometry, &sample_ipc(), false).unwrap();
    let read = read_geojson(&file).unwrap();

    assert_eq!(wkt(&read.geometry), wkt(&geometry));
    assert_eq!(read.geometry.crs().as_deref(), Some("OGC:CRS84"));
    assert_eq!(
        names(&read.attributes_ipc),
        ["id", "name", "score", "active"]
    );

    let (_, a) = column(&read.attributes_ipc, "id");
    let id: Vec<Option<i64>> = a.as_primitive::<Int64Type>().iter().collect();
    assert_eq!(id, [Some(1), None, Some(3)]);
    let (t, a) = column(&read.attributes_ipc, "score");
    assert_eq!(t, DataType::Float64);
    let score: Vec<Option<f64>> = a.as_primitive::<Float64Type>().iter().collect();
    assert_eq!(score, [Some(1.5), Some(2.0), None]);
    let (_, a) = column(&read.attributes_ipc, "name");
    let name: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(name, [Some("a"), Some("b"), None]);
    let (_, a) = column(&read.attributes_ipc, "active");
    let active: Vec<Option<bool>> = a.as_boolean().iter().collect();
    assert_eq!(active, [Some(true), None, Some(false)]);
}

#[test]
fn write_requires_a_lon_lat_crs_unless_forced() {
    let dir = TempDir::new().unwrap();
    for crs in [Some("OGC:CRS84"), Some("EPSG:4326")] {
        let file = path(&dir, "ok.geojson");
        write_geojson(&file, &sample_geometry(crs), &sample_ipc(), false).unwrap();
    }
    for (i, crs) in [Some("EPSG:3857"), None].into_iter().enumerate() {
        let file = path(&dir, &format!("bad{i}.geojson"));
        let err = write_geojson(&file, &sample_geometry(crs), &sample_ipc(), false).unwrap_err();
        assert!(err.contains("force"), "{err}");
        assert!(!std::path::Path::new(&file).exists());
        write_geojson(&file, &sample_geometry(crs), &sample_ipc(), true).unwrap();
        // forced output is read back as CRS84, as RFC 7946 defines
        assert_eq!(
            read_geojson(&file).unwrap().geometry.crs().as_deref(),
            Some("OGC:CRS84")
        );
    }
}

#[test]
fn non_finite_floats_are_written_as_null() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "nan.geojson");
    let geometry = GeoColumn::from_wkt(&[Some("POINT (0 0)"), Some("POINT (1 1)")], None).unwrap();
    let attrs = ipc(vec![(
        "v",
        Arc::new(Float64Array::from(vec![f64::NAN, f64::INFINITY])) as ArrayRef,
    )]);
    write_geojson(&file, &geometry, &attrs, true).unwrap();
    let read = read_geojson(&file).unwrap();
    assert_eq!(column(&read.attributes_ipc, "v").1.null_count(), 2);
}

#[test]
fn unsupported_column_types_fail_and_leave_no_file() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "list.geojson");
    let list = ListArray::from_iter_primitive::<Int64Type, _, _>(vec![Some(vec![Some(1)])]);
    let attrs = ipc(vec![("l", Arc::new(list) as ArrayRef)]);
    let geometry = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:4326")).unwrap();
    let err = write_geojson(&file, &geometry, &attrs, false).unwrap_err();
    assert!(err.contains("\"l\""), "{err}");
    assert!(!std::path::Path::new(&file).exists());
}

#[test]
fn attribute_and_geometry_row_counts_must_match() {
    let dir = TempDir::new().unwrap();
    let err = write_geojson(
        &path(&dir, "x.geojson"),
        &sample_geometry(Some("EPSG:4326")),
        &ipc(vec![("a", Arc::new(Int64Array::from(vec![1])) as ArrayRef)]),
        false,
    )
    .unwrap_err();
    assert!(err.contains("rows"), "{err}");
}

#[test]
fn coordinates_and_floats_round_trip_exactly() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "exact.geojson");
    let geometry =
        GeoColumn::from_wkt(&[Some("POINT (180 18.260135650634766)")], Some("EPSG:4326")).unwrap();
    let attrs = ipc(vec![(
        "v",
        Arc::new(Float64Array::from(vec![0.1 + 0.2])) as ArrayRef,
    )]);
    write_geojson(&file, &geometry, &attrs, false).unwrap();
    let read = read_geojson(&file).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&geometry));
    let (_, a) = column(&read.attributes_ipc, "v");
    assert_eq!(a.as_primitive::<Float64Type>().value(0), 0.1 + 0.2);
}
