use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, Int16Type, Int32Type, Int64Type, Int8Type, UInt64Type};
use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Float32Array, Float64Array,
    Int16Array, Int32Array, Int64Array, Int8Array, StringArray, UInt64Array,
};
use arrow_schema::DataType;
use tempfile::TempDir;

use crate::test_support::{column, ipc, names, path, wkt};
use crate::{read_flatgeobuf, write_flatgeobuf, GeoColumn};

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
            Arc::new(Float64Array::from(vec![Some(1.5), Some(2.5), None])),
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

fn points(coords: &[(f64, f64)], crs: Option<&str>) -> GeoColumn {
    let rows: Vec<Option<String>> = coords
        .iter()
        .map(|(x, y)| Some(format!("POINT ({x} {y})")))
        .collect();
    GeoColumn::from_wkt(&rows, crs).unwrap()
}

fn id_ipc(n: i64) -> Vec<u8> {
    ipc(vec![(
        "id",
        Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())) as ArrayRef,
    )])
}

#[test]
fn round_trip_without_index_preserves_order_attributes_geometry_and_crs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.fgb");
    let geometry = sample_geometry(Some("EPSG:4326"));
    write_flatgeobuf(&file, &geometry, &sample_ipc(), "layer", false).unwrap();
    let read = read_flatgeobuf(&file, None).unwrap();

    assert_eq!(wkt(&read.geometry), wkt(&geometry));
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(
        names(&read.attributes_ipc),
        ["id", "name", "score", "active"]
    );

    let (t, a) = column(&read.attributes_ipc, "id");
    assert_eq!(t, DataType::Int64);
    let id: Vec<Option<i64>> = a.as_primitive::<Int64Type>().iter().collect();
    assert_eq!(id, [Some(1), None, Some(3)]);
    let (_, a) = column(&read.attributes_ipc, "name");
    let name: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(name, [Some("a"), Some("b"), None]);
    let (_, a) = column(&read.attributes_ipc, "score");
    let score: Vec<Option<f64>> = a
        .as_primitive::<arrow_array::types::Float64Type>()
        .iter()
        .collect();
    assert_eq!(score, [Some(1.5), Some(2.5), None]);
    let (_, a) = column(&read.attributes_ipc, "active");
    let active: Vec<Option<bool>> = a.as_boolean().iter().collect();
    assert_eq!(active, [Some(true), None, Some(false)]);
}

#[test]
fn indexed_write_keeps_rows_together_but_may_reorder_them() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "indexed.fgb");
    let coords: Vec<(f64, f64)> = (0..40)
        .map(|i| (f64::from(i * 7 % 40), f64::from(i)))
        .collect();
    let geometry = points(&coords, Some("EPSG:4326"));
    write_flatgeobuf(&file, &geometry, &id_ipc(40), "layer", true).unwrap();
    let read = read_flatgeobuf(&file, None).unwrap();

    let (_, a) = column(&read.attributes_ipc, "id");
    let ids: Vec<i64> = a.as_primitive::<Int64Type>().iter().flatten().collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..40).collect::<Vec<_>>());
    let rows = wkt(&read.geometry);
    for (row, id) in ids.iter().enumerate() {
        let (x, y) = coords[*id as usize];
        assert_eq!(
            rows[row].as_deref(),
            Some(format!("POINT({x} {y})").as_str())
        );
    }
}

#[test]
fn mixed_geometry_types_keep_their_exact_type() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "mixed.fgb");
    let geometry = GeoColumn::from_wkt(
        &[
            Some("POINT (1 2)"),
            Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
            Some("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))"),
            Some("LINESTRING (0 0, 1 1)"),
        ],
        Some("EPSG:4326"),
    )
    .unwrap();
    write_flatgeobuf(&file, &geometry, &id_ipc(4), "layer", false).unwrap();
    let read = read_flatgeobuf(&file, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&geometry));
}

#[test]
fn uniform_polygons_are_not_promoted_to_multipolygons() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "poly.fgb");
    let geometry = GeoColumn::from_wkt(
        &[
            Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
            Some("POLYGON ((5 5, 6 5, 6 6, 5 5))"),
        ],
        None,
    )
    .unwrap();
    write_flatgeobuf(&file, &geometry, &id_ipc(2), "layer", true).unwrap();
    let read = read_flatgeobuf(&file, None).unwrap();
    let rows: Vec<String> = wkt(&read.geometry).into_iter().flatten().collect();
    assert!(rows.iter().all(|r| r.starts_with("POLYGON(")), "{rows:?}");
}

#[test]
fn null_geometries_survive_with_and_without_an_index() {
    let dir = TempDir::new().unwrap();
    for index in [false, true] {
        let file = path(&dir, &format!("null{index}.fgb"));
        let geometry = sample_geometry(Some("EPSG:4326"));
        write_flatgeobuf(&file, &geometry, &sample_ipc(), "layer", index).unwrap();
        let read = read_flatgeobuf(&file, None).unwrap();
        let mut rows = wkt(&read.geometry);
        rows.sort();
        let mut expected = wkt(&geometry);
        expected.sort();
        assert_eq!(rows, expected, "index = {index}");
    }
}

#[test]
fn a_leading_null_geometry_does_not_break_the_index() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "lead.fgb");
    let geometry = GeoColumn::from_wkt(
        &[None, Some("POINT (1 1)"), Some("POINT (50 50)")],
        Some("EPSG:4326"),
    )
    .unwrap();
    write_flatgeobuf(&file, &geometry, &id_ipc(3), "l", true).unwrap();
    assert_eq!(read_flatgeobuf(&file, None).unwrap().geometry.len(), 3);
    // a bbox query never returns the geometry-less row
    let hit = read_flatgeobuf(&file, Some([0.0, 0.0, 2.0, 2.0])).unwrap();
    assert_eq!(wkt(&hit.geometry), vec![Some("POINT(1 1)".to_string())]);
}

#[test]
fn column_types_map_both_ways() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "types.fgb");
    let geometry = points(&[(0.0, 0.0), (1.0, 1.0)], Some("EPSG:4326"));
    let attrs = ipc(vec![
        (
            "i8",
            Arc::new(Int8Array::from(vec![Some(-8), None])) as ArrayRef,
        ),
        ("i16", Arc::new(Int16Array::from(vec![Some(-16), None]))),
        ("i32", Arc::new(Int32Array::from(vec![Some(-32), None]))),
        (
            "u64",
            Arc::new(UInt64Array::from(vec![Some(u64::MAX), None])),
        ),
        ("f32", Arc::new(Float32Array::from(vec![Some(0.5), None]))),
        (
            "blob",
            Arc::new(BinaryArray::from_opt_vec(vec![
                Some(b"\x00\x01".as_slice()),
                None,
            ])),
        ),
        ("day", Arc::new(Date32Array::from(vec![Some(19_723), None]))),
    ]);
    write_flatgeobuf(&file, &geometry, &attrs, "layer", false).unwrap();
    let read = read_flatgeobuf(&file, None).unwrap();
    let b = &read.attributes_ipc;

    assert_eq!(column(b, "i8").0, DataType::Int8);
    assert_eq!(column(b, "i8").1.as_primitive::<Int8Type>().value(0), -8);
    assert_eq!(column(b, "i16").0, DataType::Int16);
    assert_eq!(column(b, "i16").1.as_primitive::<Int16Type>().value(0), -16);
    assert_eq!(column(b, "i32").0, DataType::Int32);
    assert_eq!(column(b, "i32").1.as_primitive::<Int32Type>().value(0), -32);
    assert_eq!(column(b, "u64").0, DataType::UInt64);
    assert_eq!(
        column(b, "u64").1.as_primitive::<UInt64Type>().value(0),
        u64::MAX
    );
    assert_eq!(column(b, "f32").0, DataType::Float32);
    assert_eq!(
        column(b, "f32").1.as_primitive::<Float32Type>().value(0),
        0.5
    );
    assert_eq!(column(b, "blob").0, DataType::Binary);
    assert_eq!(column(b, "blob").1.as_binary::<i32>().value(0), b"\x00\x01");
    // dates have no FlatGeobuf-native type here: they travel as ISO text
    assert_eq!(column(b, "day").0, DataType::Utf8);
    assert_eq!(column(b, "day").1.as_string::<i32>().value(0), "2024-01-01");
    assert_eq!(column(b, "i8").1.null_count(), 1);
}

#[test]
fn crs_maps_to_and_from_the_header() {
    let dir = TempDir::new().unwrap();
    let cases = [
        (Some("EPSG:3857"), Some("EPSG:3857")),
        (Some("EPSG:4326"), Some("EPSG:4326")),
        // FlatGeobuf has no CRS84: it is stored as the EPSG code every reader knows
        (Some("OGC:CRS84"), Some("EPSG:4326")),
        (Some("ESRI:102100"), Some("ESRI:102100")),
        (Some("IAU:49900"), Some("IAU:49900")),
        (None, None),
    ];
    for (i, (written, expected)) in cases.into_iter().enumerate() {
        let file = path(&dir, &format!("crs{i}.fgb"));
        write_flatgeobuf(
            &file,
            &points(&[(1.0, 2.0)], written),
            &id_ipc(1),
            "l",
            true,
        )
        .unwrap();
        let read = read_flatgeobuf(&file, None).unwrap();
        assert_eq!(read.geometry.crs().as_deref(), expected, "{written:?}");
    }
}

#[test]
fn unwritable_crs_is_rejected() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "bad.fgb");
    let err = write_flatgeobuf(
        &file,
        &points(&[(1.0, 2.0)], Some("utm")),
        &id_ipc(1),
        "l",
        true,
    )
    .unwrap_err();
    assert!(err.contains("CRS"), "{err}");
    assert!(!std::path::Path::new(&file).exists());
}

#[test]
fn bbox_filter_uses_the_index() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "bbox.fgb");
    let coords: Vec<(f64, f64)> = (0..50).map(|i| (f64::from(i), f64::from(i))).collect();
    write_flatgeobuf(&file, &points(&coords, None), &id_ipc(50), "l", true).unwrap();

    let read = read_flatgeobuf(&file, Some([9.5, 9.5, 12.5, 12.5])).unwrap();
    let (_, a) = column(&read.attributes_ipc, "id");
    let mut ids: Vec<i64> = a.as_primitive::<Int64Type>().iter().flatten().collect();
    ids.sort_unstable();
    assert_eq!(ids, [10, 11, 12]);
    assert_eq!(read.geometry.len(), 3);

    let none = read_flatgeobuf(&file, Some([100.0, 100.0, 101.0, 101.0])).unwrap();
    assert_eq!(none.geometry.len(), 0);
}

#[test]
fn bbox_filter_needs_an_index() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "noindex.fgb");
    write_flatgeobuf(&file, &points(&[(1.0, 2.0)], None), &id_ipc(1), "l", false).unwrap();
    let err = read_flatgeobuf(&file, Some([0.0, 0.0, 5.0, 5.0]))
        .err()
        .unwrap();
    assert!(err.contains("index"), "{err}");
}

#[test]
fn empty_frame_round_trips() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "empty.fgb");
    let geometry = GeoColumn::from_wkt::<&str>(&[], Some("EPSG:4326")).unwrap();
    write_flatgeobuf(&file, &geometry, &id_ipc(0), "l", true).unwrap();
    let read = read_flatgeobuf(&file, None).unwrap();
    assert_eq!(read.geometry.len(), 0);
    assert_eq!(
        read_flatgeobuf(&file, Some([0.0, 0.0, 1.0, 1.0]))
            .unwrap()
            .geometry
            .len(),
        0
    );
}

#[test]
fn malformed_files_are_errors_not_panics() {
    let dir = TempDir::new().unwrap();
    let junk = path(&dir, "junk.fgb");
    std::fs::write(&junk, b"definitely not flatgeobuf").unwrap();
    assert!(read_flatgeobuf(&junk, None).is_err());

    let good = path(&dir, "good.fgb");
    let coords: Vec<(f64, f64)> = (0..20).map(|i| (f64::from(i), 0.0)).collect();
    write_flatgeobuf(&good, &points(&coords, None), &id_ipc(20), "l", true).unwrap();
    let bytes = std::fs::read(&good).unwrap();
    for cut in [10, 60, bytes.len() - 30] {
        let truncated = path(&dir, &format!("cut{cut}.fgb"));
        std::fs::write(&truncated, &bytes[..cut]).unwrap();
        assert!(read_flatgeobuf(&truncated, None).is_err(), "cut at {cut}");
    }
    let err = read_flatgeobuf(&path(&dir, "missing.fgb"), None)
        .err()
        .unwrap();
    assert!(err.contains("missing.fgb"), "{err}");
}

#[test]
fn unsupported_attribute_types_fail_and_leave_no_file() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "list.fgb");
    let list =
        arrow_array::ListArray::from_iter_primitive::<Int64Type, _, _>(vec![Some(vec![Some(1)])]);
    let attrs = ipc(vec![("l", Arc::new(list) as ArrayRef)]);
    let err = write_flatgeobuf(&file, &points(&[(0.0, 0.0)], None), &attrs, "l", true).unwrap_err();
    assert!(err.contains("\"l\""), "{err}");
    assert!(!std::path::Path::new(&file).exists());
}
