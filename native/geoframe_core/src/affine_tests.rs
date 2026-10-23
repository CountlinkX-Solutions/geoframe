//! Reference cases for batch D: skew, affine_transform, per-row parameters, wrap_longitude
//! and is_geographic. Expected answers are the Shapely ones.

use crate::{crs_is_geographic, GeoColumn, Origin};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn with_crs(rows: &[Option<&str>], crs: &str) -> GeoColumn {
    GeoColumn::from_wkt(rows, Some(crs)).expect("valid WKT")
}

const UNIT: &str = "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))";

fn wkt(c: &GeoColumn) -> Vec<Option<String>> {
    c.to_wkt().unwrap()
}

fn same(a: &GeoColumn, b: &GeoColumn) -> bool {
    a.equals_exact(b, 1e-9)
        .unwrap()
        .into_iter()
        .all(|v| v == Some(true))
}

fn center() -> Vec<Option<Origin>> {
    vec![Some(Origin::Center)]
}

#[test]
fn skew_of_unit_square_matches_shapely() {
    let c = col(&[Some(UNIT)]);
    let out = c
        .skew_rows(&[Some(45.0)], &[Some(0.0)], &center(), false)
        .unwrap();
    let expected = col(&[Some("POLYGON ((-0.5 0, 0.5 0, 1.5 1, 0.5 1, -0.5 0))")]);
    assert!(same(&out, &expected), "{:?}", wkt(&out));
}

#[test]
fn skew_in_radians_equals_skew_in_degrees() {
    let c = col(&[Some(UNIT)]);
    let deg = c
        .skew_rows(&[Some(30.0)], &[Some(10.0)], &center(), false)
        .unwrap();
    let rad = c
        .skew_rows(
            &[Some(30f64.to_radians())],
            &[Some(10f64.to_radians())],
            &center(),
            true,
        )
        .unwrap();
    assert!(same(&deg, &rad));
}

#[test]
fn skew_zero_is_identity_and_origin_coord_is_a_fixed_point() {
    let c = col(&[Some(UNIT)]);
    let id = c
        .skew_rows(&[Some(0.0)], &[Some(0.0)], &center(), false)
        .unwrap();
    assert!(same(&c, &id));
    let out = c
        .skew_rows(
            &[Some(45.0)],
            &[Some(0.0)],
            &[Some(Origin::Coord(0.0, 0.0))],
            false,
        )
        .unwrap();
    let expected = col(&[Some("POLYGON ((0 0, 1 0, 2 1, 1 1, 0 0))")]);
    assert!(same(&out, &expected), "{:?}", wkt(&out));
}

#[test]
fn affine_transform_identity_swap_and_translate() {
    let c = col(&[Some("LINESTRING (1 2, 3 4)"), None]);
    let id = c.affine_transform(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    assert_eq!(wkt(&id), wkt(&c));
    let swap = c.affine_transform(&[0.0, 1.0, 1.0, 0.0, 0.0, 0.0]).unwrap();
    assert!(same(
        &swap.take(&[0]).unwrap(),
        &col(&[Some("LINESTRING (2 1, 4 3)")])
    ));
    assert_eq!(wkt(&swap)[1], None);
    // x' = a x + b y + xoff ; y' = d x + e y + yoff
    let m = c
        .affine_transform(&[2.0, 1.0, 0.0, 3.0, 10.0, 20.0])
        .unwrap();
    assert!(same(
        &m.take(&[0]).unwrap(),
        &col(&[Some("LINESTRING (14 26, 20 32)")])
    ));
}

#[test]
fn per_row_translate_and_nulls() {
    let c = col(&[
        Some("POINT (0 0)"),
        Some("POINT (0 0)"),
        Some("POINT (0 0)"),
        None,
    ]);
    let out = c
        .translate_rows(
            &[Some(1.0), Some(2.0), None, Some(1.0)],
            &[Some(10.0), Some(20.0), Some(30.0), Some(1.0)],
        )
        .unwrap();
    assert_eq!(
        wkt(&out),
        vec![
            Some("POINT(1 10)".to_string()),
            Some("POINT(2 20)".to_string()),
            None,
            None
        ]
    );
}

#[test]
fn per_row_scale_rotate_and_origin() {
    let c = col(&[Some(UNIT), Some(UNIT)]);
    let origins = vec![Some(Origin::Coord(0.0, 0.0)), Some(Origin::Centroid)];
    let scaled = c
        .scale_rows(&[Some(2.0), Some(2.0)], &[Some(2.0), Some(2.0)], &origins)
        .unwrap();
    assert!(same(
        &scaled,
        &col(&[
            Some("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"),
            Some("POLYGON ((-0.5 -0.5, 1.5 -0.5, 1.5 1.5, -0.5 1.5, -0.5 -0.5))")
        ])
    ));
    let rotated = c
        .rotate_rows(
            &[Some(90.0), Some(0.0)],
            &[Some(Origin::Coord(0.0, 0.0)), Some(Origin::Center)],
        )
        .unwrap();
    assert!(same(
        &rotated,
        &col(&[Some("POLYGON ((0 0, 0 1, -1 1, -1 0, 0 0))"), Some(UNIT)])
    ));
}

#[test]
fn per_row_length_mismatch_is_an_error() {
    let c = col(&[Some(UNIT), Some(UNIT)]);
    assert!(c
        .translate_rows(&[Some(1.0)], &[Some(1.0), Some(1.0)])
        .is_err());
}

#[test]
fn wrap_longitude_wraps_into_half_open_range() {
    let c = with_crs(
        &[
            Some("POINT (190 10)"),
            Some("POINT (-190 10)"),
            Some("POINT (180 0)"),
            Some("POINT (-180 0)"),
            Some("POINT (540 0)"),
            Some("POINT (10 5)"),
            None,
        ],
        "EPSG:4326",
    );
    let out = c.wrap_longitude().unwrap();
    assert_eq!(out.crs().as_deref(), Some("EPSG:4326"));
    let xs: Vec<Option<f64>> = out.x();
    assert_eq!(
        xs,
        vec![
            Some(-170.0),
            Some(170.0),
            Some(-180.0),
            Some(-180.0),
            Some(-180.0),
            Some(10.0),
            None
        ]
    );
    assert_eq!(out.wrap_longitude().unwrap().x(), xs);
}

#[test]
fn wrap_longitude_accepts_naive_and_rejects_projected() {
    let naive = col(&[Some("POINT (200 0)")]);
    assert_eq!(naive.wrap_longitude().unwrap().x(), vec![Some(-160.0)]);
    let projected = with_crs(&[Some("POINT (200 0)")], "EPSG:3857");
    assert!(projected
        .wrap_longitude()
        .unwrap_err()
        .contains("geographic"));
}

#[test]
fn is_geographic_by_crs() {
    assert_eq!(crs_is_geographic("EPSG:4326"), Some(true));
    assert_eq!(crs_is_geographic("OGC:CRS84"), Some(true));
    assert_eq!(crs_is_geographic("EPSG:4269"), Some(true));
    assert_eq!(crs_is_geographic("EPSG:3857"), Some(false));
    assert_eq!(crs_is_geographic("EPSG:32633"), Some(false));
    assert_eq!(crs_is_geographic("nonsense"), None);
    assert_eq!(
        with_crs(&[Some("POINT (0 0)")], "EPSG:4326").is_geographic(),
        Some(true)
    );
    assert_eq!(col(&[Some("POINT (0 0)")]).is_geographic(), None);
}
