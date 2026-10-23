//! Z/M through the geo bridge: which operations carry the extra ordinates and which return 2D
//! (Shapely 2.2 / GEOS 3.14 behaviour, see `docs/parity.md`).

use crate::{GeoColumn, Origin, PrecisionMode, UnaryOp};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

/// The WKT text with every number replaced by `#`, and the numbers (NaN included).
fn split(text: &str) -> (String, Vec<f64>) {
    let mut shape = String::new();
    let mut numbers = Vec::new();
    let mut token = String::new();
    let flush = |token: &mut String, shape: &mut String, numbers: &mut Vec<f64>| {
        if token.is_empty() {
            return;
        }
        if let Ok(value) = token.parse::<f64>() {
            numbers.push(value);
            shape.push('#');
        } else {
            shape.push_str(token);
        }
        token.clear();
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+') {
            token.push(ch);
        } else {
            flush(&mut token, &mut shape, &mut numbers);
            if !ch.is_whitespace() {
                shape.push(ch);
            }
        }
    }
    flush(&mut token, &mut shape, &mut numbers);
    (shape, numbers)
}

/// Same geometry type, dimension and structure, and numbers within 1e-9 (NaN equals NaN).
fn assert_wkt(actual: &GeoColumn, expected: &[Option<&str>]) {
    let got = actual.to_wkt().unwrap();
    assert_eq!(got.len(), expected.len());
    for (got, want) in got.iter().zip(expected) {
        match (got, want) {
            (None, None) => {}
            (Some(got), Some(want)) => {
                let ((gs, gn), (ws, wn)) = (split(got), split(want));
                assert_eq!(gs, ws, "shape of {got} vs {want}");
                assert_eq!(gn.len(), wn.len(), "{got} vs {want}");
                for (g, w) in gn.iter().zip(&wn) {
                    assert!(
                        (g.is_nan() && w.is_nan()) || (g - w).abs() <= 1e-9,
                        "{got} vs {want}"
                    );
                }
            }
            other => panic!("null mismatch: {other:?}"),
        }
    }
}

fn check(actual: Result<GeoColumn, String>, expected: &str) {
    assert_wkt(&actual.expect("operation succeeds"), &[Some(expected)]);
}

const Z_LINE: &str = "LINESTRING Z (0 0 1, 1 1 2, 2 0 3, 3 3 4)";
const M_LINE: &str = "LINESTRING M (0 0 1, 1 1 2, 2 0 3, 3 3 4)";
const ZM_LINE: &str = "LINESTRING ZM (0 0 1 10, 1 1 2 20, 2 0 3 30, 3 3 4 40)";
const Z_POLY: &str =
    "POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 4 4, 0 0 1), (1 1 5, 2 1 6, 2 2 7, 1 1 5))";
const M_POLY: &str = "POLYGON M ((0 0 1, 4 0 2, 4 4 3, 0 4 4, 0 0 1))";

// -- affine family --------------------------------------------------------------------------

#[test]
fn translate_moves_z_by_zoff_and_drops_m() {
    check(
        one(Z_LINE).translate3(1.0, 2.0, 3.0),
        "LINESTRING Z (1 2 4, 2 3 5, 3 2 6, 4 5 7)",
    );
    check(
        one(Z_LINE).translate(1.0, 2.0),
        "LINESTRING Z (1 2 1, 2 3 2, 3 2 3, 4 5 4)",
    );
    check(
        one(M_LINE).translate3(1.0, 2.0, 3.0),
        "LINESTRING (1 2, 2 3, 3 2, 4 5)",
    );
    check(
        one(ZM_LINE).translate3(1.0, 2.0, 3.0),
        "LINESTRING Z (1 2 4, 2 3 5, 3 2 6, 4 5 7)",
    );
    check(one("POINT (1 2)").translate3(1.0, 2.0, 3.0), "POINT (2 4)");
}

#[test]
fn translate_keeps_two_dimensional_rows_and_nulls_in_a_mixed_column() {
    let column = col(&[Some("POINT Z (1 2 3)"), Some("POINT (1 2)"), None]);
    assert_wkt(
        &column.translate(1.0, 1.0).unwrap(),
        &[Some("POINT Z (2 3 3)"), Some("POINT (2 3)"), None],
    );
}

#[test]
fn scale_scales_z_about_zero() {
    check(
        one(Z_LINE).scale3(2.0, 3.0, 4.0, &Origin::Coord(0.0, 0.0)),
        "LINESTRING Z (0 0 4, 2 3 8, 4 0 12, 6 9 16)",
    );
    check(
        one(Z_LINE).scale3(2.0, 3.0, 4.0, &Origin::Center),
        "LINESTRING Z (-1.5 -3 4, 0.5 0 8, 2.5 -3 12, 4.5 6 16)",
    );
    check(
        one(Z_POLY).scale(2.0, 2.0, &Origin::Coord(0.0, 0.0)),
        "POLYGON Z ((0 0 1, 8 0 2, 8 8 3, 0 8 4, 0 0 1), (2 2 5, 4 2 6, 4 4 7, 2 2 5))",
    );
    check(
        one(M_POLY).scale3(2.0, 2.0, 2.0, &Origin::Coord(0.0, 0.0)),
        "POLYGON ((0 0, 8 0, 8 8, 0 8, 0 0))",
    );
}

#[test]
fn rotate_and_skew_leave_z_alone() {
    check(
        one(Z_LINE).rotate(90.0, &Origin::Coord(0.0, 0.0)),
        "LINESTRING Z (0 0 1, -1 1 2, 0 2 3, -3 3 4)",
    );
    check(
        one(ZM_LINE).rotate(90.0, &Origin::Coord(0.0, 0.0)),
        "LINESTRING Z (0 0 1, -1 1 2, 0 2 3, -3 3 4)",
    );
    let skewed = one(Z_LINE)
        .skew_rows(
            &[Some(10.0)],
            &[Some(20.0)],
            &[Some(Origin::Coord(0.0, 0.0))],
            false,
        )
        .unwrap();
    assert_wkt(
        &skewed,
        &[Some(
            "LINESTRING Z (0 0 1, 1.176326980708465 1.3639702342662023 2, 2 0.7279404685324047 3, 3.528980942125395 4.091910702798607 4)",
        )],
    );
}

#[test]
fn affine_transform_six_numbers_leaves_z_and_twelve_transform_it() {
    check(
        one(Z_LINE).affine_transform(&[1.0, 0.0, 0.0, 1.0, 5.0, 5.0]),
        "LINESTRING Z (5 5 1, 6 6 2, 7 5 3, 8 8 4)",
    );
    check(
        one(Z_LINE)
            .affine_transform3(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 5.0, 5.0, 5.0]),
        "LINESTRING Z (5 5 7, 6 6 9, 7 5 11, 8 8 13)",
    );
    // x' = a x + b y + c z + xoff and z' = g x + h y + i z + zoff
    check(
        one("POINT Z (1 2 3)")
            .affine_transform3(&[1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 5.0, 5.0, 5.0]),
        "POINT Z (9 7 11)",
    );
    // a 2D geometry only uses a, b, d, e, xoff, yoff
    check(
        one("LINESTRING (0 0, 1 1)")
            .affine_transform3(&[1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 5.0, 5.0, 5.0]),
        "LINESTRING (5 5, 6 6)",
    );
}

// -- vertex copies ----------------------------------------------------------------------------

#[test]
fn reverse_and_normalize_keep_z_and_m() {
    check(
        one(ZM_LINE).reverse(),
        "LINESTRING ZM (3 3 4 40, 2 0 3 30, 1 1 2 20, 0 0 1 10)",
    );
    check(
        one(Z_POLY).reverse(),
        "POLYGON Z ((0 0 1, 0 4 4, 4 4 3, 4 0 2, 0 0 1), (1 1 5, 2 2 7, 2 1 6, 1 1 5))",
    );
    check(
        one(Z_POLY).normalize(),
        "POLYGON Z ((0 0 1, 0 4 4, 4 4 3, 4 0 2, 0 0 1), (1 1 5, 2 1 6, 2 2 7, 1 1 5))",
    );
    check(
        one(M_POLY).normalize(),
        "POLYGON M ((0 0 1, 0 4 4, 4 4 3, 4 0 2, 0 0 1))",
    );
    check(
        one("MULTIPOINT Z ((5 5 1), (0 0 2))").normalize(),
        "MULTIPOINT Z ((5 5 1), (0 0 2))",
    );
}

#[test]
fn boundary_exterior_interiors_and_parts_keep_z_and_m() {
    check(
        one(ZM_LINE).unary(UnaryOp::Boundary),
        "MULTIPOINT ZM ((0 0 1 10), (3 3 4 40))",
    );
    check(
        one(Z_POLY).unary(UnaryOp::Boundary),
        "MULTILINESTRING Z ((0 0 1, 4 0 2, 4 4 3, 0 4 4, 0 0 1), (1 1 5, 2 1 6, 2 2 7, 1 1 5))",
    );
    check(
        one(M_POLY).unary(UnaryOp::Boundary),
        "LINESTRING M (0 0 1, 4 0 2, 4 4 3, 0 4 4, 0 0 1)",
    );
    check(
        one(Z_POLY).unary(UnaryOp::Exterior),
        "LINESTRING Z (0 0 1, 4 0 2, 4 4 3, 0 4 4, 0 0 1)",
    );
    check(
        one(Z_POLY).interiors(),
        "MULTILINESTRING Z ((1 1 5, 2 1 6, 2 2 7, 1 1 5))",
    );
    check(
        one("GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))").get_geometry(1),
        "LINESTRING Z (0 0 1, 1 1 2)",
    );
    let (parts, source) =
        one("MULTIPOLYGON Z (((0 0 1, 2 0 1, 2 2 1, 0 0 1)), ((5 5 2, 8 5 2, 8 8 2, 5 5 2)))")
            .explode()
            .unwrap();
    assert_eq!(source, [0, 0]);
    assert_wkt(
        &parts,
        &[
            Some("POLYGON Z ((0 0 1, 2 0 1, 2 2 1, 0 0 1))"),
            Some("POLYGON Z ((5 5 2, 8 5 2, 8 8 2, 5 5 2))"),
        ],
    );
}

#[test]
fn boundary_with_no_points_is_two_dimensional() {
    let closed = one("LINESTRING Z (0 0 1, 1 0 2, 1 1 3, 0 0 1)")
        .unary(UnaryOp::Boundary)
        .unwrap();
    assert_eq!(closed.has_z(), [false]);
    assert_wkt(&closed, &[Some("MULTIPOINT EMPTY")]);
}

#[test]
fn simplify_keeps_the_z_of_surviving_vertices_and_drops_m() {
    let line = "LINESTRING Z (0 0 1, 5 0.01 2, 10 0 3, 10 10 4)";
    check(
        one(line).simplify(1.0, false),
        "LINESTRING Z (0 0 1, 10 0 3, 10 10 4)",
    );
    check(
        one("LINESTRING ZM (0 0 1 7, 5 0.01 2 8, 10 0 3 9, 10 10 4 6)").simplify(1.0, false),
        "LINESTRING Z (0 0 1, 10 0 3, 10 10 4)",
    );
    check(
        one("LINESTRING M (0 0 1, 5 0.01 2, 10 0 3, 10 10 4)").simplify(1.0, false),
        "LINESTRING (0 0, 10 0, 10 10)",
    );
}

#[test]
fn set_precision_keeps_z_and_m_except_pointwise_which_drops_m() {
    let line = "LINESTRING Z (0.12 0.34 1, 1.26 1.01 2, 2.49 0.51 3, 3.3 2.9 4)";
    let expected = "LINESTRING Z (0 0.5 1, 1.5 1 2, 2.5 0.5 3, 3.5 3 4)";
    check(
        one(line).set_precision(0.5, PrecisionMode::ValidOutput),
        expected,
    );
    check(
        one(line).set_precision(0.5, PrecisionMode::Pointwise),
        expected,
    );
    check(
        one("LINESTRING ZM (0 0 1 10, 1 1 2 20)").set_precision(0.5, PrecisionMode::ValidOutput),
        "LINESTRING ZM (0 0 1 10, 1 1 2 20)",
    );
    check(
        one("LINESTRING ZM (0 0 1 10, 1 1 2 20)").set_precision(0.5, PrecisionMode::Pointwise),
        "LINESTRING Z (0 0 1, 1 1 2)",
    );
    check(
        one("LINESTRING M (0 0 1, 1 1 2)").set_precision(0.5, PrecisionMode::ValidOutput),
        "LINESTRING M (0 0 1, 1 1 2)",
    );
}

#[test]
fn snap_takes_z_from_the_reference_and_gives_nan_where_it_has_none() {
    let line = one("LINESTRING Z (0.12 0.34 1, 1 1 2, 2.49 0.51 3)");
    check(
        line.snap(&one("POINT Z (1.1 1 9)"), 0.3),
        "LINESTRING Z (0.12 0.34 1, 1.1 1 9, 2.49 0.51 3)",
    );
    check(
        line.snap(&one("POINT (1.1 1)"), 0.3),
        "LINESTRING Z (0.12 0.34 1, 1.1 1 NaN, 2.49 0.51 3)",
    );
    // a reference vertex on an existing vertex wins as well
    check(
        one("LINESTRING Z (0 0 1, 1 1 2)").snap(&one("POINT Z (1 1 99)"), 0.3),
        "LINESTRING Z (0 0 1, 1 1 99)",
    );
    check(
        one("LINESTRING ZM (0 0 1 5, 1 1 2 6)").snap(&one("POINT Z (1.1 1 9)"), 0.3),
        "LINESTRING Z (0 0 1, 1.1 1 9)",
    );
    check(
        one("LINESTRING (0 0, 1.05 1, 3 3)").snap(&one("POINT Z (1.1 1 9)"), 0.3),
        "LINESTRING (0 0, 1.1 1, 3 3)",
    );
}

// -- dimension-dropping operations stay 2D -------------------------------------------------

#[test]
fn operations_without_a_vertex_correspondence_return_two_dimensional_results() {
    let envelope = one(Z_POLY).unary(UnaryOp::Envelope).unwrap();
    assert_eq!(envelope.has_z(), [false]);
    check(one("POINT Z (1 2 3)").centroid(), "POINT (1 2)");
}

// -- to_crs -----------------------------------------------------------------------------------

#[test]
fn to_crs_passes_z_through_and_drops_m() {
    let z = GeoColumn::from_wkt(&[Some("POINT Z (-80 25 100)")], Some("EPSG:4326")).unwrap();
    let projected = z.to_crs("EPSG:3857").unwrap();
    assert_wkt(
        &projected,
        &[Some("POINT Z (-8905559.263461886 2875744.624352243 100)")],
    );
    let zm = GeoColumn::from_wkt(&[Some("POINT ZM (-80 25 100 5)")], Some("EPSG:4326")).unwrap();
    assert_wkt(
        &zm.to_crs("EPSG:3857").unwrap(),
        &[Some("POINT Z (-8905559.263461886 2875744.624352243 100)")],
    );
    let m = GeoColumn::from_wkt(&[Some("POINT M (-80 25 5)")], Some("EPSG:4326")).unwrap();
    assert_wkt(
        &m.to_crs("EPSG:3857").unwrap(),
        &[Some("POINT (-8905559.263461886 2875744.624352243)")],
    );
}

#[test]
fn to_crs_transforms_ellipsoidal_height_to_geocentric() {
    let z = GeoColumn::from_wkt(&[Some("POINT Z (-80 25 100)")], Some("EPSG:4979")).unwrap();
    let geocentric = z.to_crs("EPSG:4978").unwrap();
    assert_wkt(
        &geocentric,
        &[Some(
            "POINT Z (1004399.2484914872 -5696231.1976074595 2679116.7247839617)",
        )],
    );
}
