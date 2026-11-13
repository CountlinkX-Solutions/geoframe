//! Reference cases for parity batch A (constructors and extraction). Expected answers are the
//! Shapely/GeoPandas ones unless a test says otherwise.

use crate::{read_geojson_str, GeoColumn, Predicate};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn wkts(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}

fn assert_geom_eq(actual: &GeoColumn, expected: &[Option<&str>]) {
    assert_eq!(actual.len(), expected.len());
    let want = col(expected);
    let eq = actual.predicate(&want, Predicate::Equals).unwrap();
    for (i, row) in expected.iter().enumerate() {
        match row {
            None => assert!(wkts(actual)[i].is_none(), "row {i} should be null"),
            Some(_) => assert_eq!(eq[i], Some(true), "row {i}: {:?}", wkts(actual)[i]),
        }
    }
}

const DONUT: &str =
    "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2), (6 6, 8 6, 8 8, 6 8, 6 6))";

// -- from_xy ------------------------------------------------------------------------------

#[test]
fn from_xy_builds_points_and_nulls_incomplete_rows() {
    let c = GeoColumn::from_xy(
        &[Some(1.0), None, Some(3.0), Some(4.0)],
        &[Some(2.0), Some(5.0), None, Some(6.0)],
        Some("EPSG:4326"),
    )
    .unwrap();
    assert_eq!(c.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(c.x(), vec![Some(1.0), None, None, Some(4.0)]);
    assert_eq!(c.y(), vec![Some(2.0), None, None, Some(6.0)]);
}

#[test]
fn from_xy_rejects_length_mismatch() {
    let err = GeoColumn::from_xy(&[Some(1.0)], &[], None).unwrap_err();
    assert!(err.contains("same length"), "{err}");
}

// -- get_coordinates ----------------------------------------------------------------------

#[test]
fn get_coordinates_lists_vertices_with_row_index_and_skips_nulls() {
    let c = col(&[
        Some("POINT (1 2)"),
        None,
        Some("LINESTRING (0 0, 1 1)"),
        Some("POINT EMPTY"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
    ]);
    let (rows, x, y) = c.get_coordinates();
    assert_eq!(rows, vec![0, 2, 2, 4, 4, 4, 4]);
    assert_eq!(x, vec![1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]);
    assert_eq!(y, vec![2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
    let total: usize = c.count_coordinates().into_iter().flatten().sum();
    assert_eq!(rows.len(), total);
}

// -- counts and parts ---------------------------------------------------------------------

#[test]
fn count_geometries_matches_shapely() {
    let c = col(&[
        Some("POINT (0 0)"),
        Some("MULTIPOINT (0 0, 1 1, 2 2)"),
        Some("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))"),
        Some("GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (0 0, 1 1))"),
        Some("GEOMETRYCOLLECTION EMPTY"),
        Some("POINT EMPTY"),
        None,
    ]);
    assert_eq!(
        c.count_geometries(),
        vec![Some(1), Some(3), Some(2), Some(2), Some(0), Some(0), None]
    );
}

#[test]
fn count_interior_rings_counts_holes_of_polygons_only() {
    let c = col(&[
        Some(DONUT),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("POINT (0 0)"),
        Some("MULTIPOLYGON (((0 0, 9 0, 9 9, 0 0), (1 1, 2 1, 2 2, 1 1)))"),
        None,
    ]);
    assert_eq!(
        c.count_interior_rings(),
        vec![Some(2), Some(0), Some(0), Some(0), None]
    );
}

#[test]
fn get_geometry_picks_parts_with_negative_indices() {
    let c = col(&[
        Some("MULTIPOINT (0 0, 1 1, 2 2)"),
        Some("POINT (7 7)"),
        Some("GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (0 0, 1 1))"),
        None,
    ]);
    assert_geom_eq(
        &c.get_geometry(0).unwrap(),
        &[
            Some("POINT (0 0)"),
            Some("POINT (7 7)"),
            Some("POINT (0 0)"),
            None,
        ],
    );
    assert_geom_eq(
        &c.get_geometry(-1).unwrap(),
        &[
            Some("POINT (2 2)"),
            Some("POINT (7 7)"),
            Some("LINESTRING (0 0, 1 1)"),
            None,
        ],
    );
    let out = c.get_geometry(5).unwrap();
    assert!(wkts(&out).iter().all(Option::is_none));
    let out = c.get_geometry(-4).unwrap();
    assert_eq!(wkts(&out)[0], None);
}

// Generator (Shapely 2.2): shapely.get_geometry(arr, [0, 1, 2, -1, 0, 0, -1, 0]) over
// [POINT (1 1), LINESTRING (0 0, 1 1), MULTIPOINT (0 0, 1 1, 2 2),
//  GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1)), POLYGON EMPTY,
//  GEOMETRYCOLLECTION EMPTY, POLYGON ((0 0, 1 0, 1 1, 0 0)), None].
#[test]
fn get_geometry_rows_takes_one_index_per_row() {
    let c = col(&[
        Some("POINT (1 1)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("MULTIPOINT (0 0, 1 1, 2 2)"),
        Some("GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1))"),
        Some("POLYGON EMPTY"),
        Some("GEOMETRYCOLLECTION EMPTY"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        None,
    ]);
    let idx = [
        Some(0),
        Some(1),
        Some(2),
        Some(-1),
        Some(0),
        Some(0),
        Some(-1),
        Some(0),
    ];
    assert_eq!(
        wkts(&c.get_geometry_rows(&idx).unwrap()),
        vec![
            Some("POINT(1 1)".to_string()),
            None,
            Some("POINT(2 2)".to_string()),
            Some("LINESTRING(0 0,1 1)".to_string()),
            Some("POLYGON EMPTY".to_string()),
            None,
            Some("POLYGON((0 0,1 0,1 1,0 0))".to_string()),
            None,
        ]
    );
    // A null index gives a null row; a wrong length is an error.
    let out = c.get_geometry_rows(&[None; 8]).unwrap();
    assert!(wkts(&out).iter().all(Option::is_none));
    assert!(c.get_geometry_rows(&[Some(0)]).is_err());
}

#[test]
fn interiors_gives_a_multilinestring_of_holes() {
    let c = col(&[
        Some(DONUT),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("POINT (0 0)"),
        Some("MULTIPOLYGON (((0 0, 9 0, 9 9, 0 0)))"),
        None,
    ]);
    let out = c.interiors().unwrap();
    assert_geom_eq(
        &out,
        &[
            Some("MULTILINESTRING ((2 2, 4 2, 4 4, 2 4, 2 2), (6 6, 8 6, 8 8, 6 8, 6 6))"),
            Some("MULTILINESTRING EMPTY"),
            None,
            None,
            None,
        ],
    );
    assert_eq!(wkts(&out)[1].as_deref(), Some("MULTILINESTRING EMPTY"));
}

#[test]
fn extract_unique_points_dedupes_in_first_seen_order() {
    let c = col(&[
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("LINESTRING (0 0, 1 1, 0 0, 2 2)"),
        Some("MULTIPOINT (3 3, 3 3)"),
        None,
    ]);
    let out = c.extract_unique_points().unwrap();
    assert_eq!(
        wkts(&out),
        vec![
            Some("MULTIPOINT((0 0),(1 0),(1 1))".to_string()),
            Some("MULTIPOINT((0 0),(1 1),(2 2))".to_string()),
            Some("MULTIPOINT((3 3))".to_string()),
            None
        ]
    );
}

#[test]
fn mean_coordinate_averages_vertices_without_the_ring_closing_point() {
    let c = col(&[
        Some("POINT (1 2)"),
        Some("LINESTRING (0 0, 2 0, 4 6)"),
        Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"),
        Some("POINT EMPTY"),
        None,
    ]);
    let out = c.mean_coordinate().unwrap();
    assert_geom_eq(
        &out,
        &[
            Some("POINT (1 2)"),
            Some("POINT (2 2)"),
            Some("POINT (2 2)"),
            None,
            None,
        ],
    );
}

// -- collect ------------------------------------------------------------------------------

#[test]
fn collect_combines_by_family() {
    let points = col(&[Some("POINT (0 0)"), None, Some("POINT (1 1)")]);
    assert_geom_eq(
        &points.collect(false).unwrap(),
        &[Some("MULTIPOINT (0 0, 1 1)")],
    );

    let polys = col(&[
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("MULTIPOLYGON (((5 5, 6 5, 6 6, 5 5)))"),
    ]);
    let out = polys.collect(false).unwrap();
    assert_eq!(out.geom_type(), vec![Some("MultiPolygon".to_string())]);

    let lines = col(&[Some("LINESTRING (0 0, 1 1)"), Some("LINESTRING (2 2, 3 3)")]);
    assert_eq!(
        lines.collect(false).unwrap().geom_type(),
        vec![Some("MultiLineString".to_string())]
    );

    let mixed = col(&[Some("POINT (0 0)"), Some("LINESTRING (0 0, 1 1)")]);
    assert_eq!(
        mixed.collect(false).unwrap().geom_type(),
        vec![Some("GeometryCollection".to_string())]
    );
}

#[test]
fn collect_single_geometry_stays_unless_multi_is_forced() {
    let one = col(&[Some("POINT (3 3)"), None]);
    assert_geom_eq(&one.collect(false).unwrap(), &[Some("POINT (3 3)")]);
    assert_geom_eq(&one.collect(true).unwrap(), &[Some("MULTIPOINT (3 3)")]);
}

#[test]
fn collect_of_nothing_is_an_empty_collection_and_keeps_the_crs() {
    let c = GeoColumn::from_wkt(&[None::<&str>], Some("EPSG:3857")).unwrap();
    let out = c.collect(false).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out.crs().as_deref(), Some("EPSG:3857"));
    assert_eq!(out.is_empty_rows(), vec![Some(true)]);
}

// -- nulls --------------------------------------------------------------------------------

#[test]
fn null_rows_are_only_actual_nulls_not_empty_geometries() {
    let c = col(&[Some("POINT (0 0)"), None, Some("POINT EMPTY")]);
    assert_eq!(c.null_rows(), vec![false, true, false]);
}

#[test]
fn fill_null_replaces_only_nulls() {
    let c = col(&[Some("POINT (0 0)"), None, Some("POINT EMPTY")]);
    let fill = col(&[Some("POINT (9 9)")]);
    let out = c.fill_null(&fill).unwrap();
    assert_eq!(out.null_rows(), vec![false, false, false]);
    assert_eq!(out.x(), vec![Some(0.0), Some(9.0), None]);
    assert!(c
        .fill_null(&col(&[Some("POINT (1 1)"), Some("POINT (2 2)")]))
        .is_err());
}

#[test]
fn fill_null_checks_the_crs() {
    let a = GeoColumn::from_wkt(&[None::<&str>], Some("EPSG:4326")).unwrap();
    let b = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:3857")).unwrap();
    assert!(a.fill_null(&b).unwrap_err().contains("CRS mismatch"));
}

// -- line predicates and validity ---------------------------------------------------------

#[test]
fn is_closed_follows_shapely() {
    let c = col(&[
        Some("LINESTRING (0 0, 1 0, 1 1, 0 0)"),
        Some("LINESTRING (0 0, 1 0, 1 1)"),
        Some("MULTILINESTRING ((0 0, 1 0, 1 1, 0 0), (5 5, 6 5, 6 6, 5 5))"),
        Some("MULTILINESTRING ((0 0, 1 0, 1 1, 0 0), (5 5, 6 5))"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("POINT (0 0)"),
        Some("LINESTRING EMPTY"),
        None,
    ]);
    assert_eq!(
        c.is_closed(),
        vec![
            Some(true),
            Some(false),
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
            None
        ]
    );
}

#[test]
fn is_ring_needs_closed_and_simple() {
    let c = col(&[
        Some("LINESTRING (0 0, 1 0, 1 1, 0 0)"),
        Some("LINESTRING (0 0, 2 2, 2 0, 0 2, 0 0)"),
        Some("LINESTRING (0 0, 1 0, 1 1)"),
        Some("LINESTRING (0 0, 1 1, 0 0)"),
        Some("LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        None,
    ]);
    assert_eq!(
        c.is_ring(),
        vec![
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            Some(true),
            Some(false),
            None
        ]
    );
}

#[test]
fn is_valid_reason_names_a_reason_or_valid_geometry() {
    let c = col(&[
        Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"),
        Some("POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))"),
        Some("POINT (1 1)"),
        None,
    ]);
    let reasons = c.is_valid_reason();
    assert_eq!(reasons[0].as_deref(), Some("Valid Geometry"));
    assert_ne!(reasons[1].as_deref(), Some("Valid Geometry"));
    assert!(reasons[1].as_ref().is_some_and(|r| !r.is_empty()));
    assert_eq!(reasons[2].as_deref(), Some("Valid Geometry"));
    assert_eq!(reasons[3], None);
}

#[test]
fn null_out_invalid_keeps_valid_rows() {
    let c = col(&[
        Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"),
        Some("POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))"),
        None,
    ]);
    let out = c.null_out_invalid().unwrap();
    assert_eq!(out.null_rows(), vec![false, true, true]);
}

// -- GeoJSON text -------------------------------------------------------------------------

#[test]
fn read_geojson_str_infers_types_like_the_file_reader() {
    let text = r#"{"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{"n":1,"s":"a"},"geometry":{"type":"Point","coordinates":[1,2]}},
        {"type":"Feature","properties":{"n":2.5},"geometry":null}]}"#;
    let read = read_geojson_str(text, Some("EPSG:4326")).unwrap();
    assert_eq!(read.geometry.len(), 2);
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(read.geometry.x(), vec![Some(1.0), None]);
    assert_eq!(
        crate::test_support::names(&read.attributes_ipc),
        vec!["n", "s"]
    );
    let (ty, _) = crate::test_support::column(&read.attributes_ipc, "n");
    assert_eq!(ty, arrow_schema::DataType::Float64);
    assert!(read_geojson_str("{nope", None).is_err());
}
