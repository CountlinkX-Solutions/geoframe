use crate::GeoColumn;

fn wkt(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

#[test]
fn wkt_round_trip_keeps_nulls_and_order() {
    let col = wkt(&[Some("POINT (1 2)"), None, Some("LINESTRING (0 0, 3 4)")]);
    assert_eq!(col.len(), 3);
    let out = col.to_wkt().unwrap();
    assert_eq!(out[0].as_deref(), Some("POINT(1 2)"));
    assert_eq!(out[1], None);
    assert_eq!(out[2].as_deref(), Some("LINESTRING(0 0,3 4)"));
}

#[test]
fn invalid_wkt_names_the_failing_row() {
    let err = GeoColumn::from_wkt(&[Some("POINT (1 2)"), Some("NOT WKT")], None).unwrap_err();
    assert!(err.contains("row 1"), "unexpected message: {err}");
}

#[test]
fn wkb_round_trip() {
    let col = wkt(&[Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"), None]);
    let wkb = col.to_wkb().unwrap();
    assert!(wkb[0].is_some() && wkb[1].is_none());
    let rows: Vec<Option<&[u8]>> = wkb.iter().map(|r| r.as_deref()).collect();
    let back = GeoColumn::from_wkb(&rows, None).unwrap();
    assert_eq!(back.to_wkt().unwrap(), col.to_wkt().unwrap());
}

#[test]
fn invalid_wkb_names_the_failing_row() {
    let bad: Vec<Option<&[u8]>> = vec![None, Some(&[1, 2, 3])];
    let err = GeoColumn::from_wkb(&bad, None).unwrap_err();
    assert!(err.contains("row 1"), "unexpected message: {err}");
}

#[test]
fn crs_is_stored_and_replaced_without_touching_geometry() {
    let col = GeoColumn::from_wkt(&[Some("POINT (1 2)")], Some("EPSG:4326")).unwrap();
    assert_eq!(col.crs().as_deref(), Some("EPSG:4326"));
    let moved = col.set_crs(Some("EPSG:3857"));
    assert_eq!(moved.crs().as_deref(), Some("EPSG:3857"));
    assert_eq!(moved.to_wkt().unwrap(), col.to_wkt().unwrap());
    assert_eq!(moved.set_crs(None).crs(), None);
}

#[test]
fn area_is_unsigned_planar_with_nulls() {
    let col = wkt(&[
        Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"),
        Some("POLYGON ((0 0, 0 3, 4 3, 4 0, 0 0))"),
        Some("POINT (1 1)"),
        None,
    ]);
    assert_eq!(col.area(), vec![Some(12.0), Some(12.0), Some(0.0), None]);
}

#[test]
fn length_is_perimeter_for_polygons_and_zero_for_points() {
    let col = wkt(&[
        Some("LINESTRING (0 0, 3 4)"),
        Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"),
        Some("POINT (1 1)"),
        None,
    ]);
    assert_eq!(col.length(), vec![Some(5.0), Some(14.0), Some(0.0), None]);
}

#[test]
fn is_valid_flags_self_intersections() {
    let col = wkt(&[
        Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"),
        Some("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))"),
        None,
    ]);
    assert_eq!(col.is_valid(), vec![Some(true), Some(false), None]);
}

#[test]
fn bounds_are_null_for_null_and_empty_rows() {
    let col = wkt(&[Some("LINESTRING (1 5, 3 -2)"), None, Some("POLYGON EMPTY")]);
    assert_eq!(col.bounds(), vec![Some([1.0, -2.0, 3.0, 5.0]), None, None]);
    assert_eq!(col.total_bounds(), Some([1.0, -2.0, 3.0, 5.0]));
    assert_eq!(wkt(&[None]).total_bounds(), None);
}

#[test]
fn centroid_returns_points_and_keeps_crs() {
    let col = GeoColumn::from_wkt(
        &[Some("POLYGON ((0 0, 4 0, 4 2, 0 2, 0 0))"), None],
        Some("EPSG:4326"),
    )
    .unwrap();
    let c = col.centroid().unwrap();
    assert_eq!(c.crs().as_deref(), Some("EPSG:4326"));
    let out = c.to_wkt().unwrap();
    assert_eq!(out[0].as_deref(), Some("POINT(2 1)"));
    assert_eq!(out[1], None);
}

#[test]
fn take_reorders_repeats_and_validates_indices() {
    let col = GeoColumn::from_wkt(
        &[Some("POINT (0 0)"), None, Some("POINT (2 2)")],
        Some("EPSG:4326"),
    )
    .unwrap();
    let t = col.take(&[2, 1, 0, 2]).unwrap();
    assert_eq!(t.crs().as_deref(), Some("EPSG:4326"));
    let out: Vec<_> = t.to_wkt().unwrap().into_iter().collect();
    assert_eq!(
        out,
        vec![
            Some("POINT(2 2)".to_string()),
            None,
            Some("POINT(0 0)".to_string()),
            Some("POINT(2 2)".to_string())
        ]
    );
    assert!(col.take(&[3]).is_err());
}

#[test]
fn empty_point_is_kept_but_measures_are_null() {
    let col = wkt(&[Some("POINT EMPTY"), Some("POINT (1 1)")]);
    assert_eq!(col.len(), 2);
    assert_eq!(col.area(), vec![None, Some(0.0)]);
    assert_eq!(col.centroid().unwrap().to_wkt().unwrap()[0], None);
}

fn geojson(rows: &[Option<&str>]) -> Vec<Option<serde_json::Value>> {
    wkt(rows)
        .to_geojson()
        .into_iter()
        .map(|row| row.map(|text| serde_json::from_str(&text).expect("valid JSON")))
        .collect()
}

#[test]
fn geojson_point() {
    let out = geojson(&[Some("POINT (1.5 2)")]);
    assert_eq!(
        out[0],
        Some(serde_json::json!({"type": "Point", "coordinates": [1.5, 2.0]}))
    );
}

#[test]
fn geojson_polygon_with_hole() {
    let out = geojson(&[Some(
        "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))",
    )]);
    let value = out[0].as_ref().unwrap();
    assert_eq!(value["type"], "Polygon");
    let rings = value["coordinates"].as_array().unwrap();
    assert_eq!(rings.len(), 2);
    assert_eq!(rings[0].as_array().unwrap().len(), 5);
}

#[test]
fn geojson_multipolygon() {
    let out = geojson(&[Some(
        "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))",
    )]);
    let value = out[0].as_ref().unwrap();
    assert_eq!(value["type"], "MultiPolygon");
    assert_eq!(value["coordinates"].as_array().unwrap().len(), 2);
}

#[test]
fn geojson_keeps_nulls_and_order() {
    let out = geojson(&[Some("POINT (1 2)"), None, Some("LINESTRING (0 0, 3 4)")]);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].as_ref().unwrap()["type"], "Point");
    assert!(out[1].is_none());
    assert_eq!(out[2].as_ref().unwrap()["type"], "LineString");
}

#[test]
fn geojson_empty_geometries() {
    let out = geojson(&[Some("POLYGON EMPTY"), Some("POINT EMPTY")]);
    assert_eq!(
        out[0],
        Some(serde_json::json!({"type": "Polygon", "coordinates": []}))
    );
    // Not representable as a `geo` geometry, so null (same policy as the measures).
    assert!(out[1].is_none());
}

#[test]
fn geojson_of_empty_column() {
    assert!(wkt(&[]).to_geojson().is_empty());
}
