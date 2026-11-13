//! Batch I1: `is_simple`, `self_intersects`, `normalize`, `set_precision`, vertex
//! constructors and `geoparquet_metadata`. Expected answers come from Shapely 2.2 / GEOS 3.14.

use tempfile::TempDir;

use crate::{
    geoparquet_metadata, write_geoparquet, GeoColumn, PrecisionMode, Predicate, VertexKind,
    VertexRing,
};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

/// WKT of `expected`, as this crate prints it, for exact (vertex order) comparisons.
fn canon(wkt: &str) -> String {
    one(wkt).to_wkt().unwrap().remove(0).unwrap()
}

fn text(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}

// -- is_simple ----------------------------------------------------------------------------

#[test]
fn is_simple_matches_geos() {
    let cases: &[(&str, bool)] = &[
        ("POINT (1 1)", true),
        ("MULTIPOINT ((0 0), (1 1))", true),
        ("MULTIPOINT ((0 0), (0 0))", false),
        ("MULTIPOINT EMPTY", true),
        ("LINESTRING (0 0, 4 4)", true),
        ("LINESTRING (0 0, 4 4, 4 0, 0 4)", false),
        ("LINESTRING (0 0, 4 0, 4 4, 0 0)", true),
        ("LINESTRING (0 0, 0 0, 2 0)", true),
        ("LINESTRING (1 1, 1 1)", true),
        ("LINESTRING (0 0, 2 0, 1 1, 1 0)", false),
        ("LINESTRING (0 0, 2 0, 2 2, 0 0, 0 -2)", false),
        ("LINESTRING (0 0, 4 0, 2 0)", false),
        ("LINESTRING (0 0, 1 1, 0 0)", false),
        ("LINESTRING (0 0, 4 0, 4 4, 2 0)", false),
        ("LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0, 2 -1)", false),
        ("LINESTRING (0 0, 4 0, 4 4, 2 2, 0 4, 0 0)", true),
        ("LINESTRING (0 0, 2 2, 4 0, 2 2)", false),
        ("LINESTRING EMPTY", true),
        ("MULTILINESTRING ((0 0, 2 0), (2 0, 4 0))", true),
        ("MULTILINESTRING ((0 0, 2 0), (1 -1, 1 1))", false),
        ("MULTILINESTRING ((0 0, 2 0), (1 0, 3 0))", false),
        ("MULTILINESTRING ((0 0, 2 0), (2 0, 4 0), (2 0, 2 2))", true),
        (
            "MULTILINESTRING ((0 0, 2 0), (2 0, 4 0), (2 0, 2 2), (2 0, 2 -2))",
            true,
        ),
        ("MULTILINESTRING ((0 0, 4 0, 4 4, 0 0), (0 0, -2 -2))", false),
        ("MULTILINESTRING ((0 0, 4 0, 4 4, 0 0), (4 0, 6 0))", false),
        ("MULTILINESTRING ((0 0, 2 0), (2 0, 2 5), (0 0, 2 0))", false),
        ("MULTILINESTRING ((0 0, 4 0), (2 0, 2 3))", false),
        (
            "MULTILINESTRING ((0 0, 4 0, 4 4, 0 0), (10 10, 12 12, 12 10, 10 10))",
            true,
        ),
        (
            "MULTILINESTRING ((0 0, 4 0, 4 4, 0 0), (0 0, 4 0, 4 4, 0 0))",
            false,
        ),
        ("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))", false),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))", true),
        ("POLYGON ((0 0, 1 1, 2 2, 0 0))", false),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2), (3 3, 5 3, 5 5, 3 5, 3 3))",
            true,
        ),
        (
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((1 1, 3 1, 3 3, 1 3, 1 1)))",
            true,
        ),
        ("POLYGON EMPTY", true),
        // Shapely answers false for every collection, empty or not.
        (
            "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 2 2))",
            false,
        ),
        ("GEOMETRYCOLLECTION EMPTY", false),
    ];
    for (wkt, expected) in cases {
        assert_eq!(one(wkt).is_simple(), vec![Some(*expected)], "{wkt}");
    }
}

#[test]
fn is_simple_keeps_null_rows_null() {
    let c = col(&[Some("POINT (1 1)"), None]);
    assert_eq!(c.is_simple(), vec![Some(true), None]);
}

#[test]
fn self_intersects_is_the_inverse_for_lines_only() {
    let c = col(&[
        Some("LINESTRING (0 0, 4 4, 4 0, 0 4)"),
        Some("LINESTRING (0 0, 4 4)"),
        Some("MULTILINESTRING ((0 0, 4 0), (2 0, 2 3))"),
        Some("POINT (1 1)"),
        Some("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))"),
        None,
    ]);
    assert_eq!(
        c.self_intersects(),
        vec![
            Some(true),
            Some(false),
            Some(true),
            Some(false),
            Some(false),
            None
        ]
    );
}

// -- normalize ----------------------------------------------------------------------------

#[test]
fn normalize_matches_geos() {
    let cases: &[(&str, &str)] = &[
        ("POINT (1 2)", "POINT (1 2)"),
        ("LINESTRING (3 3, 2 2, 1 1)", "LINESTRING (1 1, 2 2, 3 3)"),
        ("LINESTRING (1 1, 2 2, 3 3)", "LINESTRING (1 1, 2 2, 3 3)"),
        ("LINESTRING (2 2, 0 0, 2 2)", "LINESTRING (0 0, 2 2, 0 0)"),
        ("LINESTRING (1 0, 0 5, 1 0)", "LINESTRING (0 5, 1 0, 0 5)"),
        ("LINESTRING (3 3, 3 3, 1 1)", "LINESTRING (1 1, 3 3, 3 3)"),
        (
            "LINESTRING (4 4, 0 4, 0 0, 4 0, 4 4)",
            "LINESTRING (0 0, 0 4, 4 4, 4 0, 0 0)",
        ),
        (
            "LINESTRING (4 4, 4 0, 0 0, 0 4, 4 4)",
            "LINESTRING (0 0, 0 4, 4 4, 4 0, 0 0)",
        ),
        (
            "LINESTRING (0 0, 2 0, 2 2, 0 0, 2 2, 0 0)",
            "LINESTRING (0 0, 2 0, 2 2, 0 0, 2 2, 0 0)",
        ),
        (
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
            "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))",
        ),
        (
            "POLYGON ((2 0, 4 4, 0 4, 0 0, 2 0))",
            "POLYGON ((0 0, 0 4, 4 4, 2 0, 0 0))",
        ),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (6 6, 6 8, 8 8, 8 6, 6 6), (2 2, 2 4, 4 4, 4 2, 2 2), (5 1, 5 2, 6 2, 6 1, 5 1))",
            "POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0), (6 6, 8 6, 8 8, 6 8, 6 6), (5 1, 6 1, 6 2, 5 2, 5 1), (2 2, 4 2, 4 4, 2 4, 2 2))",
        ),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 3 2, 3 3, 2 2), (2 2, 5 2, 5 5, 2 5, 2 2))",
            "POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0), (2 2, 5 2, 5 5, 2 5, 2 2), (2 2, 3 2, 3 3, 2 2))",
        ),
        (
            "POLYGON ((0 0, 0 0, 4 0, 4 4, 0 0))",
            "POLYGON ((0 0, 4 4, 4 0, 0 0, 0 0))",
        ),
        (
            "MULTIPOINT ((0 0), (5 5), (1 3), (1 1))",
            "MULTIPOINT ((5 5), (1 3), (1 1), (0 0))",
        ),
        (
            "MULTILINESTRING ((0 0, 1 1), (5 5, 6 6), (2 2, 3 3, 4 4), (9 9, 8 8))",
            "MULTILINESTRING ((2 2, 3 3, 4 4), (8 8, 9 9), (5 5, 6 6), (0 0, 1 1))",
        ),
        (
            "MULTIPOLYGON (((0 0, 20 0, 20 20, 0 20, 0 0)), ((5 5, 8 5, 8 8, 5 8, 5 5)), ((0 0, 1 0, 1 1, 0 0)))",
            "MULTIPOLYGON (((5 5, 5 8, 8 8, 8 5, 5 5)), ((0 0, 0 20, 20 20, 20 0, 0 0)), ((0 0, 1 1, 1 0, 0 0)))",
        ),
        (
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((0 0, 2 0, 2 2, 0 2, 0 0), (0.5 0.5, 1 0.5, 1 1, 0.5 0.5)))",
            "MULTIPOLYGON (((0 0, 0 2, 2 2, 2 0, 0 0), (0.5 0.5, 1 0.5, 1 1, 0.5 0.5)), ((0 0, 0 2, 2 2, 2 0, 0 0)))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 2 2), POLYGON ((0 0, 1 0, 1 1, 0 0)), MULTIPOINT ((1 1), (0 0)))",
            "GEOMETRYCOLLECTION (POLYGON ((0 0, 1 1, 1 0, 0 0)), LINESTRING (0 0, 2 2), MULTIPOINT ((1 1), (0 0)), POINT (1 1))",
        ),
        (
            "GEOMETRYCOLLECTION (LINESTRING (5 5, 6 6), LINESTRING (0 0, 7 7), POINT (3 3), POINT (2 9), POINT (3 1))",
            "GEOMETRYCOLLECTION (LINESTRING (5 5, 6 6), LINESTRING (0 0, 7 7), POINT (3 3), POINT (3 1), POINT (2 9))",
        ),
        // Collections order their members by type rank, then content, with empties first
        // within a type (nested collections cannot be stored in GeoArrow).
        (
            "GEOMETRYCOLLECTION (POINT (0 0), LINESTRING EMPTY, POINT (2 2), LINESTRING (1 1, 2 2))",
            "GEOMETRYCOLLECTION (LINESTRING (1 1, 2 2), LINESTRING EMPTY, POINT (2 2), POINT (0 0))",
        ),
        ("POLYGON EMPTY", "POLYGON EMPTY"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            text(&one(input).normalize().unwrap()),
            vec![Some(canon(expected))],
            "{input}"
        );
    }
}

#[test]
fn normalize_keeps_nulls_and_crs() {
    let c = GeoColumn::from_wkt(&[Some("LINESTRING (3 3, 1 1)"), None], Some("EPSG:4326")).unwrap();
    let n = c.normalize().unwrap();
    assert_eq!(n.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(text(&n), vec![Some(canon("LINESTRING (1 1, 3 3)")), None]);
}

#[test]
fn normalize_is_idempotent() {
    let c = col(&[
        Some("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (6 6, 6 8, 8 8, 8 6, 6 6), (2 2, 2 4, 4 4, 4 2, 2 2))"),
        Some("MULTIPOINT ((0 0), (5 5), (1 3))"),
    ]);
    let once = c.normalize().unwrap();
    assert_eq!(text(&once.normalize().unwrap()), text(&once));
}

// -- set_precision ------------------------------------------------------------------------

fn precise(wkt: &str, grid: f64, mode: PrecisionMode) -> Option<String> {
    text(&one(wkt).set_precision(grid, mode).unwrap()).remove(0)
}

#[test]
fn pointwise_rounds_every_coordinate_half_up() {
    let p = PrecisionMode::Pointwise;
    assert_eq!(
        precise("POINT (1.4 2.6)", 1.0, p),
        Some(canon("POINT (1 3)"))
    );
    // GEOS rounds with floor(x + 0.5): halves go up, also for negative numbers.
    assert_eq!(
        precise("POINT (0.5 1.5)", 1.0, p),
        Some(canon("POINT (1 2)"))
    );
    assert_eq!(
        precise("POINT (-0.5 -1.5)", 1.0, p),
        Some(canon("POINT (0 -1)"))
    );
    assert_eq!(
        precise("POINT (1.23456 2.34567)", 0.01, p),
        Some(canon("POINT (1.23 2.35)"))
    );
    assert_eq!(
        precise("POINT (1.23456 2.34567)", 0.25, p),
        Some(canon("POINT (1.25 2.25)"))
    );
    assert_eq!(
        precise("POINT (1.23456 2.34567)", 3.0, p),
        Some(canon("POINT (0 3)"))
    );
    assert_eq!(
        precise("POINT (0.3 2.7)", 0.1, p),
        Some(canon("POINT (0.3 2.7)"))
    );
    // The sign of the grid size is ignored.
    assert_eq!(
        precise("POINT (1.5 2.5)", -1.0, p),
        Some(canon("POINT (2 3)"))
    );
}

#[test]
fn pointwise_keeps_repeated_vertices_and_collapsed_shapes() {
    let p = PrecisionMode::Pointwise;
    assert_eq!(
        precise("LINESTRING (0.1 0.1, 0.2 0.2, 5.4 5.4)", 1.0, p),
        Some(canon("LINESTRING (0 0, 0 0, 5 5)"))
    );
    assert_eq!(
        precise("POLYGON ((0 0, 0.2 0, 0.2 0.2, 0 0.2, 0 0))", 1.0, p),
        Some(canon("POLYGON ((0 0, 0 0, 0 0, 0 0, 0 0))"))
    );
    assert_eq!(
        precise("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))", 1.0, p),
        Some(canon("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))"))
    );
}

#[test]
fn zero_grid_leaves_coordinates_alone() {
    for mode in [
        PrecisionMode::ValidOutput,
        PrecisionMode::Pointwise,
        PrecisionMode::KeepCollapsed,
    ] {
        assert_eq!(
            precise("POINT (1.23456 2.34567)", 0.0, mode),
            Some(canon("POINT (1.23456 2.34567)"))
        );
    }
}

fn assert_same_shape(actual: &GeoColumn, expected: &str) {
    let want = one(expected);
    let both_empty = text(actual) == text(&want);
    let equal = actual.predicate(&want, Predicate::Equals).unwrap() == vec![Some(true)];
    assert!(both_empty || equal, "{:?} is not {expected}", text(actual));
}

#[test]
fn valid_output_removes_repeated_vertices_and_collapsed_parts() {
    let v = PrecisionMode::ValidOutput;
    let cases: &[(&str, &str)] = &[
        ("POINT (1.4 2.6)", "POINT (1 3)"),
        (
            "MULTIPOINT ((0.1 0.1), (0.2 0.2), (3.3 3.3))",
            "MULTIPOINT ((0 0), (0 0), (3 3))",
        ),
        (
            "LINESTRING (0.1 0.1, 0.2 0.2, 5.4 5.4)",
            "LINESTRING (0 0, 5 5)",
        ),
        ("LINESTRING (0.1 0.1, 0.2 0.2)", "LINESTRING EMPTY"),
        ("LINESTRING (0 0, 0 0.4, 0 0)", "LINESTRING EMPTY"),
        (
            "LINESTRING (0.1 0.1, 5.1 0.2, 5.2 5.1, 0.3 0.4)",
            "LINESTRING (0 0, 5 0, 5 5, 0 0)",
        ),
        (
            "POLYGON ((0.1 0.1, 4.2 0.1, 4.2 4.2, 0.1 4.2, 0.1 0.1))",
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
        ),
        (
            "POLYGON ((0 0, 0.2 0, 0.2 0.2, 0 0.2, 0 0))",
            "POLYGON EMPTY",
        ),
        ("POLYGON ((0 0, 5 0, 5.1 0.2, 0 0))", "POLYGON EMPTY"),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2.2 2.2, 2.4 2.2, 2.4 2.4, 2.2 2.2))",
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))",
        ),
        (
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((0.1 0.1, 0.3 0.1, 0.3 0.3, 0.1 0.1)))",
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT (1.2 1.2), LINESTRING (0 0, 0.1 0.1))",
            "GEOMETRYCOLLECTION (POINT (1 1))",
        ),
        (
            "LINESTRING (0 0, 10 10, 10 0, 0 10)",
            "LINESTRING (0 0, 10 10, 10 0, 0 10)",
        ),
    ];
    for (input, expected) in cases {
        let got = one(input).set_precision(1.0, v).unwrap();
        assert_same_shape(&got, expected);
    }
}

#[test]
fn valid_output_returns_valid_polygons() {
    // The two thin spikes of this polygon collapse onto each other at grid size 2.
    let thin = one("POLYGON ((0 0, 10 0, 10 0.4, 5 0.9, 0 0.4, 0 0))");
    let got = thin.set_precision(2.0, PrecisionMode::ValidOutput).unwrap();
    assert_eq!(got.is_valid(), vec![Some(true)]);
    // A hole that lands outside the snapped shell must not leave an invalid polygon.
    let holed =
        one("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (0.2 0.2, 0.2 4, 4 4, 4 0.2, 0.2 0.2))");
    let got = holed
        .set_precision(5.0, PrecisionMode::ValidOutput)
        .unwrap();
    assert_eq!(got.is_valid(), vec![Some(true)]);
}

#[test]
fn valid_output_merges_holes_and_parts_that_snap_onto_each_other() {
    let v = PrecisionMode::ValidOutput;
    // Two holes that land on the same square at grid size 2.5 become one hole.
    let holes = one("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2), (3 3, 5 3, 5 5, 3 5, 3 3))");
    assert_same_shape(
        &holes.set_precision(2.5, v).unwrap(),
        "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2.5 2.5, 5 2.5, 5 5, 2.5 5, 2.5 2.5))",
    );
    // Two squares that snap to the same square are one square, not their symmetric difference.
    let parts = one("MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((0.1 0.1, 2.1 0.1, 2.1 2.1, 0.1 2.1, 0.1 0.1)))");
    assert_same_shape(
        &parts.set_precision(2.5, v).unwrap(),
        "MULTIPOLYGON (((0 0, 2.5 0, 2.5 2.5, 0 2.5, 0 0)))",
    );
}

#[test]
fn collections_unwrap_single_member_multi_geometries() {
    // GEOS rebuilds a collection through `buildGeometry`, which turns a one-member multi into
    // its member; a multi outside a collection keeps its type.
    let input = "GEOMETRYCOLLECTION (MULTIPOINT ((5 5)), MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0))), MULTILINESTRING ((0 0, 1 1)))";
    for mode in [PrecisionMode::Pointwise, PrecisionMode::ValidOutput] {
        let got = one(input).set_precision(1.0, mode).unwrap();
        let wkt = text(&got).remove(0).unwrap();
        assert!(
            wkt.contains("POINT(5 5)") && !wkt.contains("MULTI"),
            "{mode:?}: {wkt}"
        );
    }
    assert_eq!(
        precise("MULTIPOINT ((5 5))", 1.0, PrecisionMode::Pointwise),
        Some(canon("MULTIPOINT ((5 5))"))
    );
}

#[test]
fn keep_collapsed_keeps_collapsed_lines() {
    let k = PrecisionMode::KeepCollapsed;
    assert_eq!(
        precise("LINESTRING (0.1 0.1, 0.2 0.2)", 1.0, k),
        Some(canon("LINESTRING (0 0, 0 0)"))
    );
    assert_eq!(
        precise("LINESTRING (0 0, 0 0.4, 0 0)", 1.0, k),
        Some(canon("LINESTRING (0 0, 0 0)"))
    );
    assert_eq!(
        precise("POLYGON ((0 0, 0.2 0, 0.2 0.2, 0 0.2, 0 0))", 1.0, k),
        Some(canon("POLYGON EMPTY"))
    );
}

#[test]
fn set_precision_rejects_bad_grid_and_keeps_crs() {
    let c = GeoColumn::from_wkt(&[Some("POINT (1.4 2.6)"), None], Some("EPSG:3857")).unwrap();
    assert!(c.set_precision(f64::NAN, PrecisionMode::Pointwise).is_err());
    assert!(c
        .set_precision(f64::INFINITY, PrecisionMode::Pointwise)
        .is_err());
    let r = c.set_precision(1.0, PrecisionMode::ValidOutput).unwrap();
    assert_eq!(r.crs().as_deref(), Some("EPSG:3857"));
    assert_eq!(text(&r), vec![Some(canon("POINT (1 3)")), None]);
}

#[test]
fn precision_mode_parses_names() {
    assert_eq!(
        PrecisionMode::parse("valid_output").unwrap(),
        PrecisionMode::ValidOutput
    );
    assert_eq!(
        PrecisionMode::parse("pointwise").unwrap(),
        PrecisionMode::Pointwise
    );
    assert_eq!(
        PrecisionMode::parse("keep_collapsed").unwrap(),
        PrecisionMode::KeepCollapsed
    );
    assert!(PrecisionMode::parse("nope").is_err());
}

// -- constructors from vertices -----------------------------------------------------------

type Vertices = Vec<Vec<Vec<(f64, f64)>>>;

fn line(v: &[(f64, f64)]) -> Vertices {
    vec![vec![v.to_vec()]]
}

#[test]
fn line_strings_from_vertices() {
    let rows = vec![
        Some(line(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)])),
        None,
        Some(line(&[])),
    ];
    let c = GeoColumn::from_vertices(VertexKind::LineString, &rows, Some("EPSG:4326")).unwrap();
    assert_eq!(c.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(
        text(&c),
        vec![
            Some(canon("LINESTRING (0 0, 1 1, 2 0)")),
            None,
            Some(canon("LINESTRING EMPTY"))
        ]
    );
}

#[test]
fn a_line_needs_two_vertices() {
    let rows = vec![Some(line(&[(0.0, 0.0)]))];
    let err = GeoColumn::from_vertices(VertexKind::LineString, &rows, None).unwrap_err();
    assert!(err.contains("row 0") && err.contains("two"), "{err}");
}

#[test]
fn multi_points_from_vertices() {
    let rows = vec![Some(line(&[(0.0, 0.0), (1.0, 2.0)])), Some(line(&[]))];
    let c = GeoColumn::from_vertices(VertexKind::MultiPoint, &rows, None).unwrap();
    assert_eq!(
        text(&c),
        vec![
            Some(canon("MULTIPOINT ((0 0), (1 2))")),
            Some(canon("MULTIPOINT EMPTY"))
        ]
    );
}

#[test]
fn polygons_from_rings_close_open_rings() {
    let shell = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
    let closed_hole = vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 1.0)];
    let rows = vec![
        Some(vec![vec![shell.clone(), closed_hole]]),
        Some(vec![vec![shell]]),
    ];
    let c = GeoColumn::from_vertices(VertexKind::Polygon, &rows, None).unwrap();
    assert_eq!(
        text(&c),
        vec![
            Some(canon(
                "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))"
            )),
            Some(canon("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"))
        ]
    );
}

#[test]
fn a_ring_needs_three_distinct_vertices() {
    let rows = vec![Some(vec![vec![vec![(0.0, 0.0), (1.0, 1.0)]]])];
    let err = GeoColumn::from_vertices(VertexKind::Polygon, &rows, None).unwrap_err();
    assert!(err.contains("row 0") && err.contains("ring"), "{err}");
}

#[test]
fn multi_line_strings_and_multi_polygons() {
    let lines = vec![Some(vec![
        vec![vec![(0.0, 0.0), (1.0, 1.0)]],
        vec![vec![(5.0, 5.0), (6.0, 5.0), (6.0, 6.0)]],
    ])];
    let c = GeoColumn::from_vertices(VertexKind::MultiLineString, &lines, None).unwrap();
    assert_eq!(
        text(&c),
        vec![Some(canon("MULTILINESTRING ((0 0, 1 1), (5 5, 6 5, 6 6))"))]
    );

    let square = |x: f64| vec![(x, 0.0), (x + 1.0, 0.0), (x + 1.0, 1.0), (x, 1.0)];
    let polys = vec![Some(vec![vec![square(0.0)], vec![square(5.0)]])];
    let c = GeoColumn::from_vertices(VertexKind::MultiPolygon, &polys, None).unwrap();
    assert_eq!(
        text(&c),
        vec![Some(canon(
            "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((5 0, 6 0, 6 1, 5 1, 5 0)))"
        ))]
    );
}

// Z vertices follow Shapely 2.2 (`linestrings`, `multipoints`, `polygons`, ...); the expected
// WKT is the Shapely result, see `build_zm_coords` in test/reference/generate.py.
fn z_ring(v: &[(f64, f64, f64)]) -> VertexRing {
    (
        v.iter().map(|&(x, y, _)| (x, y)).collect(),
        Some(v.iter().map(|&(_, _, z)| z).collect()),
    )
}

fn flat_ring(v: &[(f64, f64)]) -> VertexRing {
    (v.to_vec(), None)
}

#[test]
fn z_vertices_build_z_geometries() {
    let rows = vec![
        Some(vec![vec![z_ring(&[(0.0, 0.0, 1.0), (1.0, 1.0, 2.0)])]]),
        Some(vec![vec![flat_ring(&[(0.0, 0.0), (1.0, 1.0)])]]),
        None,
    ];
    let c = GeoColumn::from_vertices_z(VertexKind::LineString, &rows, None).unwrap();
    assert_eq!(c.has_z(), [true, false, false]);
    assert_eq!(
        text(&c),
        vec![
            Some("LINESTRING Z(0 0 1,1 1 2)".to_string()),
            Some(canon("LINESTRING (0 0, 1 1)")),
            None
        ]
    );
}

#[test]
fn z_multipoints_and_multi_lines() {
    let c = GeoColumn::from_vertices_z(
        VertexKind::MultiPoint,
        &[Some(vec![vec![z_ring(&[
            (0.0, 0.0, 1.0),
            (1.0, 2.0, 2.0),
        ])]])],
        None,
    )
    .unwrap();
    assert_eq!(
        text(&c),
        vec![Some("MULTIPOINT Z((0 0 1),(1 2 2))".to_string())]
    );

    // a 2D line next to a Z line is promoted with NaN Z (Shapely multilinestrings)
    let mixed = vec![
        vec![z_ring(&[(0.0, 0.0, 1.0), (1.0, 1.0, 2.0)])],
        vec![flat_ring(&[(5.0, 5.0), (6.0, 6.0)])],
    ];
    let c = GeoColumn::from_vertices_z(VertexKind::MultiLineString, &[Some(mixed)], None).unwrap();
    assert_eq!(
        text(&c),
        vec![Some(
            "MULTILINESTRING Z((0 0 1,1 1 2),(5 5 NaN,6 6 NaN))".to_string()
        )]
    );
}

#[test]
fn z_polygons_close_with_the_first_z_and_promote_rings() {
    let shell = z_ring(&[(0.0, 0.0, 1.0), (4.0, 0.0, 2.0), (4.0, 4.0, 3.0)]);
    let hole = flat_ring(&[(1.0, 1.0), (2.0, 1.0), (2.0, 2.0)]);
    let c = GeoColumn::from_vertices_z(VertexKind::Polygon, &[Some(vec![vec![shell, hole]])], None)
        .unwrap();
    assert_eq!(
        text(&c),
        vec![Some(
            "POLYGON Z((0 0 1,4 0 2,4 4 3,0 0 1),(1 1 NaN,2 1 NaN,2 2 NaN,1 1 NaN))".to_string()
        )]
    );

    // closed in XY but not in Z: Shapely appends the first vertex again
    let open_in_z = z_ring(&[
        (0.0, 0.0, 1.0),
        (4.0, 0.0, 2.0),
        (4.0, 4.0, 3.0),
        (0.0, 0.0, 9.0),
    ]);
    let c = GeoColumn::from_vertices_z(VertexKind::Polygon, &[Some(vec![vec![open_in_z]])], None)
        .unwrap();
    assert_eq!(
        text(&c),
        vec![Some(
            "POLYGON Z((0 0 1,4 0 2,4 4 3,0 0 9,0 0 1))".to_string()
        )]
    );
}

#[test]
fn z_multi_polygons() {
    let square = |z: f64| vec![z_ring(&[(0.0, 0.0, z), (4.0, 0.0, z), (4.0, 4.0, z)])];
    let flat = vec![flat_ring(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)])];
    let c = GeoColumn::from_vertices_z(
        VertexKind::MultiPolygon,
        &[Some(vec![square(1.0), flat])],
        Some("EPSG:4979"),
    )
    .unwrap();
    assert_eq!(c.crs().as_deref(), Some("EPSG:4979"));
    assert_eq!(
        text(&c),
        vec![Some(
            "MULTIPOLYGON Z(((0 0 1,4 0 1,4 4 1,0 0 1)),((0 0 NaN,4 0 NaN,4 4 NaN,0 0 NaN)))"
                .to_string()
        )]
    );
}

#[test]
fn z_validation_still_names_the_row() {
    let err = GeoColumn::from_vertices_z(
        VertexKind::LineString,
        &[Some(vec![vec![z_ring(&[(0.0, 0.0, 1.0)])]])],
        None,
    )
    .unwrap_err();
    assert!(err.contains("row 0") && err.contains("two"), "{err}");

    // a Z list shorter than its vertices is a bug on the caller's side, not a panic
    let ragged = ((vec![(0.0, 0.0), (1.0, 1.0)]), Some(vec![1.0]));
    let err = GeoColumn::from_vertices_z(VertexKind::LineString, &[Some(vec![vec![ragged]])], None)
        .unwrap_err();
    assert!(err.contains("row 0") && err.contains("Z"), "{err}");
}

#[test]
fn non_finite_vertices_are_rejected() {
    let rows = vec![Some(line(&[(0.0, 0.0), (f64::NAN, 1.0)]))];
    assert!(GeoColumn::from_vertices(VertexKind::LineString, &rows, None).is_err());
}

#[test]
fn vertex_kind_parses_names() {
    assert_eq!(VertexKind::parse("polygon").unwrap(), VertexKind::Polygon);
    assert!(VertexKind::parse("circle").is_err());
}

// -- geoparquet_metadata ------------------------------------------------------------------

#[test]
fn geoparquet_metadata_returns_the_geo_json() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.parquet").to_str().unwrap().to_string();
    let geometry = GeoColumn::from_wkt(&[Some("POINT (1 2)"), None], Some("EPSG:4326")).unwrap();
    write_geoparquet(&path, &geometry, "geom", &empty_attributes(2)).unwrap();

    let json = geoparquet_metadata(&path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["primary_column"], "geom");
    assert_eq!(doc["columns"]["geom"]["encoding"], "WKB");
    assert!(doc["version"].is_string());
}

#[test]
fn geoparquet_metadata_reports_plain_parquet_and_missing_files() {
    let dir = TempDir::new().unwrap();
    let missing = dir
        .path()
        .join("missing.parquet")
        .to_str()
        .unwrap()
        .to_string();
    assert!(geoparquet_metadata(&missing).is_err());
}

fn empty_attributes(rows: usize) -> Vec<u8> {
    use arrow_array::{ArrayRef, Int64Array};
    let values: Vec<Option<i64>> = (0..rows as i64).map(Some).collect();
    let array: ArrayRef = std::sync::Arc::new(Int64Array::from(values));
    crate::test_support::ipc(vec![("id", array)])
}
