//! Batch I2 (part 1): `snap`, `shared_paths`, `minimum_clearance`, `minimum_clearance_line`.
//! Expected answers come from Shapely 2.2 / GEOS 3.14.

use crate::parity_j::NO_CLEARANCE;
use crate::GeoColumn;

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

/// `wkt` as this crate prints it, so expected values compare textually.
fn canon(wkt: &str) -> String {
    one(wkt).to_wkt().unwrap().remove(0).unwrap()
}

fn text(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}

// -- snap ---------------------------------------------------------------------------------

#[test]
fn snap_matches_geos() {
    let cases: &[(&str, &str, f64, &str)] = &[
        ("POINT (0.5 2.5)", "POINT (0 2)", 1.0, "POINT (0 2)"),
        (
            "LINESTRING (0.1 0.1, 0.49 0.51, 1.01 0.89)",
            "LINESTRING (0 0, 0.5 0.5, 1 1)",
            1.0,
            "LINESTRING (0 0, 0.5 0.5, 1 1)",
        ),
        (
            "POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0))",
            "POINT (0 2)",
            1.0,
            "POLYGON ((0 0, 0 2, 0 10, 10 10, 10 0, 0 0))",
        ),
        (
            "POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0))",
            "POINT (8 10)",
            1.0,
            "POLYGON ((0 0, 0 10, 8 10, 10 10, 10 0, 0 0))",
        ),
        (
            "LINESTRING (0 0, 10 0)",
            "POINT (5 0.5)",
            1.0,
            "LINESTRING (0 0, 5 0.5, 10 0)",
        ),
        (
            "LINESTRING (0 0, 10 0)",
            "MULTIPOINT ((3 0.2),(6 -0.3))",
            1.0,
            "LINESTRING (0 0, 3 0.2, 6 -0.3, 10 0)",
        ),
        (
            "LINESTRING (0 0, 10 0)",
            "MULTIPOINT ((0.3 0),(3 0.2))",
            1.0,
            "LINESTRING (0.3 0, 3 0.2, 10 0)",
        ),
        (
            "LINESTRING (0 0, 10 0, 10 10)",
            "LINESTRING (10 0.4, 10 5)",
            1.0,
            "LINESTRING (0 0, 10 0.4, 10 5, 10 10)",
        ),
        (
            "MULTILINESTRING ((0 0, 10 0),(0 5, 10 5))",
            "POINT (5 0.1)",
            0.5,
            "MULTILINESTRING ((0 0, 5 0.1, 10 0), (0 5, 10 5))",
        ),
        (
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
            "POLYGON ((0.2 0.1, 4.1 -0.1, 4 4.2, 0 3.9, 0.2 0.1))",
            0.5,
            "POLYGON ((0.2 0.1, 4.1 -0.1, 4 4.2, 0 3.9, 0.2 0.1))",
        ),
        (
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
            "POLYGON ((0.2 0.1, 4.1 -0.1, 4 4.2, 0 3.9, 0.2 0.1))",
            0.05,
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 3.9, 0 0))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 2 2))",
            "POINT (1.1 1.1)",
            0.5,
            "GEOMETRYCOLLECTION (POINT (1.1 1.1), LINESTRING (0 0, 1.1 1.1, 2 2))",
        ),
        (
            "LINESTRING (0 0, 10 0)",
            "POINT (5 5)",
            1.0,
            "LINESTRING (0 0, 10 0)",
        ),
        (
            "LINESTRING (0 0, 10 0)",
            "POINT (0 0)",
            1.0,
            "LINESTRING (0 0, 10 0)",
        ),
        (
            "LINESTRING (0 0, 0.5 0, 10 0)",
            "POINT (0.3 0.2)",
            1.0,
            "LINESTRING (0 0, 0.3 0.2, 10 0)",
        ),
        (
            "LINESTRING (0 0, 10 0)",
            "LINESTRING EMPTY",
            1.0,
            "LINESTRING (0 0, 10 0)",
        ),
        ("POINT (0.5 2.5)", "POINT (0 2)", 0.0, "POINT (0.5 2.5)"),
        (
            "LINESTRING (0 0, 4 0, 4 4, 0 0)",
            "POINT (4.2 2)",
            0.5,
            "LINESTRING (0 0, 4 0, 4.2 2, 4 4, 0 0)",
        ),
        (
            "LINESTRING (0 0, 4 0, 4 4, 0 0)",
            "POINT (0.1 0.1)",
            0.5,
            "LINESTRING (0.1 0.1, 4 0, 4 4, 0.1 0.1)",
        ),
        (
            "LINESTRING (0 0, 2 0, 4 0)",
            "MULTIPOINT ((1 0.1),(1.5 -0.1),(3 0.2))",
            0.5,
            "LINESTRING (0 0, 1 0.1, 1.5 -0.1, 2 0, 3 0.2, 4 0)",
        ),
        // One snap point takes only the nearest vertex of a line.
        (
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
            "POINT (2 2)",
            5.0,
            "POLYGON ((2 2, 4 0, 4 4, 0 4, 2 2))",
        ),
        (
            "LINESTRING (0 0, 1 0, 2 0, 3 0)",
            "MULTIPOINT ((1.4 0.2),(1.6 0.3))",
            0.9,
            "LINESTRING (0 0, 1.4 0.2, 1.6 0.3, 2 0, 3 0)",
        ),
        // Snap points beyond the end of a line extend it, in order.
        (
            "LINESTRING (0 0, 5 0)",
            "MULTIPOINT ((5.2 0.1),(5.6 0.1),(6 0.1))",
            2.0,
            "LINESTRING (0 0, 5.2 0.1, 5.6 0.1, 6 0.1)",
        ),
        (
            "LINESTRING (0 0, 5 0)",
            "MULTIPOINT ((-0.2 0.1),(-0.6 0.1),(-1 0.1))",
            2.0,
            "LINESTRING (-1 0.1, -0.6 0.1, -0.2 0.1, 5 0)",
        ),
    ];
    for (a, b, tol, expected) in cases {
        let got = text(&one(a).snap(&one(b), *tol).unwrap());
        assert_eq!(
            got,
            vec![Some(canon(expected))],
            "{a} snapped to {b} ({tol})"
        );
    }
}

#[test]
fn snap_broadcasts_and_keeps_nulls_and_crs() {
    let a = GeoColumn::from_wkt(
        &[Some("POINT (0.5 2.5)"), None, Some("POINT (9 9)")],
        Some("EPSG:4326"),
    )
    .unwrap();
    let b = GeoColumn::from_wkt(&[Some("POINT (0 2)")], Some("EPSG:4326")).unwrap();
    let out = a.snap(&b, 1.0).unwrap();
    assert_eq!(
        text(&out),
        vec![Some(canon("POINT (0 2)")), None, Some(canon("POINT (9 9)"))]
    );
    assert_eq!(out.crs().as_deref(), Some("EPSG:4326"));
}

#[test]
fn snap_rejects_mismatched_lengths_and_crs() {
    let a = col(&[Some("POINT (0 0)"), Some("POINT (1 1)")]);
    let b = col(&[
        Some("POINT (0 0)"),
        Some("POINT (1 1)"),
        Some("POINT (2 2)"),
    ]);
    assert!(a.snap(&b, 1.0).is_err());
    let c = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:4326")).unwrap();
    let d = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:3857")).unwrap();
    assert!(c.snap(&d, 1.0).is_err());
}

#[test]
fn snap_work_is_capped() {
    // 8,000 x 8,000 vertex and snap-point pairs exceed the cap: the row is null.
    let coords: Vec<String> = (0..8000).map(|i| format!("{} {}", i, i % 7)).collect();
    let line = format!("LINESTRING ({})", coords.join(", "));
    let pts: Vec<String> = (0..8000).map(|i| format!("({} {})", i, i % 5)).collect();
    let multi = format!("MULTIPOINT ({})", pts.join(", "));
    let out = one(&line).snap(&one(&multi), 1.0).unwrap();
    assert_eq!(text(&out), vec![None]);
}

// -- shared_paths -------------------------------------------------------------------------

#[test]
fn shared_paths_matches_geos() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "LINESTRING (0 0, 2 0, 4 0)",
            "LINESTRING (0 0, 4 0)",
            "GEOMETRYCOLLECTION (MULTILINESTRING ((0 0, 2 0), (2 0, 4 0)), MULTILINESTRING EMPTY)",
        ),
        (
            "LINESTRING (0 0, 2 0, 2 2)",
            "LINESTRING (0 0, 2 0, 2 2)",
            "GEOMETRYCOLLECTION (MULTILINESTRING ((0 0, 2 0), (2 0, 2 2)), MULTILINESTRING EMPTY)",
        ),
        (
            "LINESTRING (0 0, 4 0)",
            "LINESTRING (4 0, 0 0)",
            "GEOMETRYCOLLECTION (MULTILINESTRING EMPTY, MULTILINESTRING ((0 0, 4 0)))",
        ),
        (
            "LINESTRING (0 0, 4 0, 4 4)",
            "LINESTRING (1 0, 4 0, 4 3, 5 3)",
            "GEOMETRYCOLLECTION (MULTILINESTRING ((1 0, 4 0), (4 0, 4 3)), MULTILINESTRING EMPTY)",
        ),
        (
            "LINESTRING (0 0, 4 0)",
            "LINESTRING (2 0, 2 4)",
            "GEOMETRYCOLLECTION (MULTILINESTRING EMPTY, MULTILINESTRING EMPTY)",
        ),
        (
            "MULTILINESTRING ((0 0, 4 0),(0 1, 4 1))",
            "MULTILINESTRING ((1 0, 3 0),(3 1, 1 1))",
            "GEOMETRYCOLLECTION (MULTILINESTRING ((1 0, 3 0)), MULTILINESTRING ((1 1, 3 1)))",
        ),
        (
            "LINESTRING (0 0, 4 0)",
            "LINESTRING (1 0, 2 0, 3 0)",
            "GEOMETRYCOLLECTION (MULTILINESTRING ((1 0, 2 0), (2 0, 3 0)), MULTILINESTRING EMPTY)",
        ),
        (
            "LINESTRING (0 0, 4 0, 0 0)",
            "LINESTRING (1 0, 3 0)",
            "GEOMETRYCOLLECTION (MULTILINESTRING ((1 0, 3 0)), MULTILINESTRING EMPTY)",
        ),
        (
            "LINESTRING EMPTY",
            "LINESTRING (0 0, 1 1)",
            "GEOMETRYCOLLECTION (MULTILINESTRING EMPTY, MULTILINESTRING EMPTY)",
        ),
        (
            "LINESTRING (0 0, 1 0, 1 1, 0 1, 0 0)",
            "LINESTRING (1 1, 0 1)",
            "GEOMETRYCOLLECTION (MULTILINESTRING EMPTY, MULTILINESTRING ((0 1, 1 1)))",
        ),
    ];
    for (a, b, expected) in cases {
        let got = one(a).shared_paths(&one(b)).unwrap();
        let want = one(expected);
        assert_eq!(
            normalised(&got),
            normalised(&want),
            "shared_paths({a}, {b})"
        );
    }
}

/// A shared-paths result with every line's direction and the line order made canonical.
fn normalised(column: &GeoColumn) -> Vec<Option<String>> {
    text(&column.normalize().unwrap())
}

#[test]
fn shared_paths_rejects_non_lines_with_null() {
    let a = col(&[
        Some("POINT (0 0)"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        None,
        Some("LINESTRING (0 0, 4 0)"),
    ]);
    let b = one("LINESTRING (0 0, 4 0)");
    let out = text(&a.shared_paths(&b).unwrap());
    assert_eq!(out[0], None);
    assert_eq!(out[1], None);
    assert_eq!(out[2], None);
    assert!(out[3].is_some());
}

// -- minimum_clearance --------------------------------------------------------------------

#[test]
fn minimum_clearance_matches_geos() {
    let cases: &[(&str, f64)] = &[
        ("POINT (1 1)", NO_CLEARANCE),
        ("MULTIPOINT ((0 0),(3 4))", 5.0),
        ("MULTIPOINT ((0 0),(0 0))", NO_CLEARANCE),
        ("LINESTRING (0 0, 10 0)", 10.0),
        ("LINESTRING (0 0, 10 0, 5 0.5)", 0.5),
        ("LINESTRING (0 0, 10 0, 10 1, 5 0.5)", 0.5),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))", 4.0),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
            2.0,
        ),
        ("POLYGON ((0 0, 6 0, 3 5, 0 0))", 5.0),
        ("LINESTRING EMPTY", NO_CLEARANCE),
        (
            "GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (1 1, 5 5))",
            std::f64::consts::SQRT_2,
        ),
        ("LINESTRING (1 1, 1 1)", NO_CLEARANCE),
        ("LINESTRING (0 0, 0 0, 5 5)", 7.0710678118654755),
        ("MULTILINESTRING ((0 0, 5 0),(2 1, 3 1))", 1.0),
        (
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((5 5, 8 5, 8 8, 5 8, 5 5)))",
            2.0,
        ),
        ("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))", 2.8284271247461903),
        ("LINESTRING (0 0, 1 0.001, 2 0)", 1.000000499999875),
    ];
    for (wkt, expected) in cases {
        let got = one(wkt).minimum_clearance()[0].expect("not null");
        assert!(
            (got - expected).abs() <= 1e-12 * expected.abs().max(1.0),
            "{wkt}: {got} != {expected}"
        );
    }
}

#[test]
fn minimum_clearance_keeps_nulls() {
    let c = col(&[Some("LINESTRING (0 0, 3 4)"), None]);
    assert_eq!(c.minimum_clearance(), vec![Some(5.0), None]);
}

#[test]
fn minimum_clearance_line_connects_the_closest_pair() {
    let cases: &[(&str, &str)] = &[
        ("POINT (1 1)", "LINESTRING EMPTY"),
        ("LINESTRING (0 0, 10 0)", "LINESTRING (0 0, 10 0)"),
        ("LINESTRING (0 0, 10 0, 5 0.5)", "LINESTRING (5 0.5, 5 0)"),
        ("POLYGON ((0 0, 6 0, 3 5, 0 0))", "LINESTRING (3 5, 3 0)"),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
            "LINESTRING (2 2, 4 2)",
        ),
        (
            "MULTILINESTRING ((0 0, 5 0),(2 1, 3 1))",
            "LINESTRING (2 1, 3 1)",
        ),
        ("LINESTRING (0 0, 0 0, 5 5)", "LINESTRING (0 0, 5 5)"),
        ("LINESTRING EMPTY", "LINESTRING EMPTY"),
    ];
    for (wkt, expected) in cases {
        let got = one(wkt).minimum_clearance_line().unwrap();
        assert_eq!(
            normalised(&got),
            normalised(&one(expected)),
            "minimum_clearance_line({wkt})"
        );
    }
}

#[test]
fn minimum_clearance_line_nulls_and_crs() {
    let c = GeoColumn::from_wkt(&[Some("LINESTRING (0 0, 3 4)"), None], Some("EPSG:4326")).unwrap();
    let out = c.minimum_clearance_line().unwrap();
    assert_eq!(text(&out), vec![Some(canon("LINESTRING (0 0, 3 4)")), None]);
    assert_eq!(out.crs().as_deref(), Some("EPSG:4326"));
}

#[test]
fn minimum_clearance_scales_to_many_vertices() {
    // 20,000 collinear-ish vertices must not take quadratic time.
    let coords: Vec<String> = (0..20_000)
        .map(|i| format!("{} {}", i, (i % 2) * 3))
        .collect();
    let line = format!("LINESTRING ({})", coords.join(", "));
    let got = one(&line).minimum_clearance()[0].unwrap();
    assert!(got > 0.0 && got < 4.0);
}
