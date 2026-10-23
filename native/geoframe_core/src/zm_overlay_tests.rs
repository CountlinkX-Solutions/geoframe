//! Z/M through the interpolating and overlay operations (Shapely 2.2 / GEOS 3.14, OverlayNG):
//! vertices keep their ordinates, new nodes interpolate along both segments and average, the
//! rest is filled from the GEOS elevation model. Expected values come from Shapely.

use crate::{GeoColumn, SetOp, UnaryOp};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

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

fn assert_text(got: &str, want: &str) {
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

/// Compares in GEOS canonical form so that ring starts and winding do not matter.
fn check(actual: Result<GeoColumn, String>, expected: &str) {
    let column = actual.expect("operation succeeds").normalize().unwrap();
    let got = column.to_wkt().unwrap();
    assert_eq!(got.len(), 1);
    assert_text(got[0].as_deref().expect("a geometry"), expected);
}

fn check_raw(actual: Result<GeoColumn, String>, expected: &str) {
    let got = actual.expect("operation succeeds").to_wkt().unwrap();
    assert_text(got[0].as_deref().expect("a geometry"), expected);
}

const A: &str = "POLYGON Z ((0 0 1, 10 0 2, 10 10 3, 0 10 4, 0 0 1))";
const B: &str = "POLYGON Z ((5 5 100, 15 5 200, 15 15 300, 5 15 400, 5 5 100))";
const B_2D: &str = "POLYGON ((5 5, 15 5, 15 15, 5 15, 5 5))";

fn op(a: &str, b: &str, op: SetOp) -> Result<GeoColumn, String> {
    one(a).set_op(&one(b), op)
}

// -- interpolation --------------------------------------------------------------------------

#[test]
fn segmentize_interpolates_z_and_drops_m() {
    let expected = "LINESTRING Z (0 0 0, 3.333333333333334 0 3.333333333333334, 6.666666666666668 0 6.666666666666668, 10 0 10, 10 3.333333333333334 13.333333333333334, 10 6.666666666666668 16.666666666666668, 10 10 20)";
    check_raw(
        one("LINESTRING Z (0 0 0, 10 0 10, 10 10 20)").segmentize(4.0),
        expected,
    );
    check_raw(
        one("LINESTRING ZM (0 0 0 5, 10 0 10 6, 10 10 20 7)").segmentize(4.0),
        expected,
    );
    check_raw(
        one("LINESTRING M (0 0 0, 10 0 10)").segmentize(5.0),
        "LINESTRING (0 0, 5 0, 10 0)",
    );
    check_raw(
        one("POLYGON Z ((0 0 1, 10 0 2, 10 10 3, 0 10 4, 0 0 1))").segmentize(6.0),
        "POLYGON Z ((0 0 1, 5 0 1.5, 10 0 2, 10 5 2.5, 10 10 3, 5 10 3.5, 0 10 4, 0 5 2.5, 0 0 1))",
    );
}

#[test]
fn interpolate_gives_z_and_drops_m() {
    let line = one("LINESTRING Z (0 0 0, 10 0 10, 10 10 20)");
    check_raw(line.interpolate(&[Some(15.0)], false), "POINT Z (10 5 15)");
    check_raw(line.interpolate(&[Some(0.5)], true), "POINT Z (10 0 10)");
    check_raw(line.interpolate(&[Some(-3.0)], false), "POINT Z (10 7 17)");
    check_raw(
        one("LINESTRING ZM (0 0 0 5, 10 0 10 9)").interpolate(&[Some(2.0)], false),
        "POINT Z (2 0 2)",
    );
    check_raw(
        one("LINESTRING M (0 0 0, 10 0 10)").interpolate(&[Some(2.0)], false),
        "POINT (2 0)",
    );
    check_raw(
        one("MULTILINESTRING Z ((0 0 0, 10 0 10), (20 0 100, 30 0 200))")
            .interpolate(&[Some(15.0)], false),
        "POINT Z (25 0 150)",
    );
}

#[test]
fn clip_by_rect_interpolates_new_vertices() {
    check(
        one(A).clip_by_rect(2.0, -5.0, 15.0, 5.0),
        "POLYGON Z ((2 0 1.2, 2 5 2.5, 10 5 2.5, 10 0 2, 2 0 1.2))",
    );
    check(
        one(A).clip_by_rect(5.0, 5.0, 15.0, 15.0),
        "POLYGON Z ((5 5 2.5, 5 10 3.5, 10 10 3, 10 5 2.5, 5 5 2.5))",
    );
    check_raw(
        one("MULTILINESTRING Z ((-5 5 0, 15 5 20), (1 1 1, 2 2 2))")
            .clip_by_rect(0.0, 0.0, 10.0, 10.0),
        "MULTILINESTRING Z ((0 5 5, 10 5 15), (1 1 1, 2 2 2))",
    );
    check(
        one("POLYGON M ((-5 -5 1, 5 -5 2, 5 5 3, -5 5 4, -5 -5 1))")
            .clip_by_rect(0.0, 0.0, 10.0, 10.0),
        "POLYGON ((0 0, 0 5, 5 5, 5 0, 0 0))",
    );
}

// -- vertex-copy constructors ---------------------------------------------------------------

#[test]
fn hulls_and_unique_points_keep_z_and_drop_m() {
    let points = "MULTIPOINT ZM ((0 0 1 5), (10 0 2 6), (10 10 3 7), (3 4 5 8))";
    check(
        one(points).unary(UnaryOp::ConvexHull),
        "POLYGON Z ((0 0 1, 3 4 5, 10 10 3, 10 0 2, 0 0 1))",
    );
    check(
        one(points).concave_hull(1.0, false),
        "POLYGON Z ((0 0 1, 3 4 5, 10 10 3, 10 0 2, 0 0 1))",
    );
    check_raw(
        one("LINESTRING ZM (0 0 0 5, 10 0 10 6, 10 10 20 7)").extract_unique_points(),
        "MULTIPOINT Z ((0 0 0), (10 0 10), (10 10 20))",
    );
}

#[test]
fn line_merge_keeps_z_and_drops_m() {
    check_raw(
        one("MULTILINESTRING ZM ((0 0 1 5, 5 5 2 6), (5 5 3 7, 9 9 4 8))").line_merge(false),
        "LINESTRING Z (0 0 1, 5 5 2, 9 9 4)",
    );
}

#[test]
fn make_valid_keeps_m_only_when_nothing_changes() {
    let valid = "POLYGON ZM ((0 0 1 5, 10 0 2 6, 10 10 3 7, 0 10 4 8, 0 0 1 5))";
    check_raw(one(valid).unary(UnaryOp::MakeValid), valid);
    check(
        one("POLYGON ZM ((0 0 0 1, 10 10 10 2, 10 0 20 3, 0 10 30 4, 0 0 0 1))")
            .unary(UnaryOp::MakeValid),
        "MULTIPOLYGON Z (((5 5 15, 10 10 10, 10 0 20, 5 5 15)), ((0 0 0, 0 10 30, 5 5 15, 0 0 0)))",
    );
}

#[test]
fn representative_point_keeps_z_for_lines_only() {
    check_raw(
        one("MULTILINESTRING Z ((0 0 1, 10 0 2), (0 5 3, 10 5 4))")
            .unary(UnaryOp::RepresentativePoint),
        "POINT Z (0 0 1)",
    );
    check_raw(
        one("POLYGON ZM ((0 0 1 5, 10 0 2 6, 10 10 3 7, 0 10 4 8, 0 0 1 5))")
            .unary(UnaryOp::RepresentativePoint),
        "POINT (5 5)",
    );
}

#[test]
fn remove_repeated_points_and_orient_keep_z_and_m() {
    check_raw(
        one("LINESTRING ZM (0 0 0 1, 0 0 1 2, 5 5 2 3)").remove_repeated_points(0.0),
        "LINESTRING ZM (0 0 0 1, 5 5 2 3)",
    );
    check_raw(
        one("POLYGON ZM ((0 0 1 5, 0 10 4 8, 10 10 3 7, 10 0 2 6, 0 0 1 5))")
            .orient_polygons(false),
        "POLYGON ZM ((0 0 1 5, 10 0 2 6, 10 10 3 7, 0 10 4 8, 0 0 1 5))",
    );
}

// -- set operations -------------------------------------------------------------------------

#[test]
fn polygon_overlay_interpolates_nodes_and_averages() {
    check(
        op(A, B, SetOp::Intersection),
        "POLYGON Z ((5 5 100, 5 10 126.75, 10 10 3, 10 5 76.25, 5 5 100))",
    );
    check(
        op(A, B, SetOp::Union),
        "POLYGON Z ((0 0 1, 0 10 4, 5 10 126.75, 5 15 400, 15 15 300, 15 5 200, 10 5 76.25, 10 0 2, 0 0 1))",
    );
    check(
        op(A, B, SetOp::Difference),
        "POLYGON Z ((0 0 1, 0 10 4, 5 10 126.75, 5 5 100, 10 5 76.25, 10 0 2, 0 0 1))",
    );
}

#[test]
fn overlay_with_a_two_dimensional_input_interpolates_and_fills_from_the_elevation_model() {
    check(
        op(A, B_2D, SetOp::Intersection),
        "POLYGON Z ((5 5 2.5, 5 10 3.5, 10 10 3, 10 5 2.5, 5 5 2.5))",
    );
    check(
        op(A, B_2D, SetOp::Union),
        "POLYGON Z ((0 0 1, 0 10 4, 5 10 3.5, 5 15 2.5, 15 15 3, 15 5 2.5, 10 5 2.5, 10 0 2, 0 0 1))",
    );
    check(
        op(B_2D, A, SetOp::Intersection),
        "POLYGON Z ((5 5 2.5, 5 10 3.5, 10 10 3, 10 5 2.5, 5 5 2.5))",
    );
}

#[test]
fn m_is_interpolated_and_missing_m_stays_nan() {
    let zm = "POLYGON ZM ((0 0 1 11, 10 0 2 12, 10 10 3 13, 0 10 4 14, 0 0 1 11))";
    check(
        op(zm, B, SetOp::Intersection),
        "POLYGON ZM ((5 5 100 NaN, 5 10 126.75 13.5, 10 10 3 13, 10 5 76.25 12.5, 5 5 100 NaN))",
    );
}

#[test]
fn line_overlay_averages_the_z_of_a_crossing() {
    let (l1, l2) = (
        "LINESTRING Z (0 0 0, 10 10 10)",
        "LINESTRING Z (0 10 100, 10 0 200)",
    );
    check(op(l1, l2, SetOp::Intersection), "POINT Z (5 5 77.5)");
    check(
        op(l1, l2, SetOp::Union),
        "MULTILINESTRING Z ((5 5 77.5, 10 10 10), (5 5 77.5, 10 0 200), (0 10 100, 5 5 77.5), (0 0 0, 5 5 77.5))",
    );
    check(
        op(l1, "LINESTRING (0 10, 10 0)", SetOp::Union),
        "MULTILINESTRING Z ((5 5 5, 10 10 10), (5 5 5, 10 0 5), (0 10 5, 5 5 5), (0 0 0, 5 5 5))",
    );
}

#[test]
fn overlay_without_z_stays_two_dimensional_and_empties_keep_the_dimension() {
    check(
        op(
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))",
            B_2D,
            SetOp::Intersection,
        ),
        "POLYGON ((5 5, 5 10, 10 10, 10 5, 5 5))",
    );
    check_raw(
        op(
            A,
            "POLYGON Z ((50 50 1, 60 50 1, 60 60 1, 50 50 1))",
            SetOp::Intersection,
        ),
        "POLYGON Z EMPTY",
    );
}

#[test]
fn nan_z_everywhere_stays_nan_and_never_panics() {
    check(
        one("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))")
            .force_3d(&[Some(f64::NAN)])
            .unwrap()
            .set_op(&one(B_2D), SetOp::Intersection),
        "POLYGON Z ((5 5 NaN, 5 10 NaN, 10 10 NaN, 10 5 NaN, 5 5 NaN))",
    );
}

#[test]
fn union_all_and_intersection_all_follow_the_pairwise_rules() {
    let rows = col(&[Some(A), Some(B)]);
    check(
        rows.union_all(),
        "POLYGON Z ((0 0 1, 0 10 4, 5 10 126.75, 5 15 400, 15 15 300, 15 5 200, 10 5 76.25, 10 0 2, 0 0 1))",
    );
    check(
        rows.intersection_all(),
        "POLYGON Z ((5 5 100, 5 10 126.75, 10 10 3, 10 5 76.25, 5 5 100))",
    );
}

#[test]
fn broadcast_and_mixed_columns_work_row_by_row() {
    let left = col(&[
        Some(A),
        Some("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))"),
        None,
    ]);
    let result = left.set_op(&one(B), SetOp::Intersection).unwrap();
    let wkt = result.normalize().unwrap().to_wkt().unwrap();
    assert_text(
        wkt[0].as_deref().unwrap(),
        "POLYGON Z ((5 5 100, 5 10 126.75, 10 10 3, 10 5 76.25, 5 5 100))",
    );
    assert!(wkt[2].is_none());
}

// -- constructors over a whole column -------------------------------------------------------

#[test]
fn delaunay_polygonize_and_build_area_take_z_from_the_column() {
    let points = one("MULTIPOINT ZM ((0 0 1 5), (10 0 2 6), (10 10 3 7), (3 4 5 8))");
    let triangles = points.delaunay_triangles(0.0, false).unwrap();
    let wkt: Vec<String> = triangles
        .normalize()
        .unwrap()
        .to_wkt()
        .unwrap()
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(wkt.len(), 2);
    for (got, want) in wkt.iter().zip([
        "POLYGON Z ((0 0 1, 3 4 5, 10 0 2, 0 0 1))",
        "POLYGON Z ((3 4 5, 10 10 3, 10 0 2, 3 4 5))",
    ]) {
        assert_text(got, want);
    }

    let lines = one("MULTILINESTRING Z ((0 0 1, 10 0 2), (10 0 2, 10 10 3), (10 10 3, 0 0 1))");
    check(
        Ok(lines.polygonize(false).unwrap().polygons),
        "POLYGON Z ((0 0 1, 10 10 3, 10 0 2, 0 0 1))",
    );
    check(
        lines.build_area(false),
        "POLYGON Z ((0 0 1, 10 10 3, 10 0 2, 0 0 1))",
    );
}

#[test]
fn intersection_keeps_the_first_inputs_z_at_a_shared_edge() {
    let a = "POLYGON Z ((0 0 1, 10 0 2, 10 10 3, 0 10 4, 0 0 1))";
    let b = "POLYGON Z ((10 0 20, 20 0 21, 20 10 22, 10 10 23, 10 0 20))";
    check(
        op(a, b, SetOp::Intersection),
        "LINESTRING Z (10 0 2, 10 10 3)",
    );
}

#[test]
fn non_finite_coordinates_and_ordinates_never_panic() {
    let nan_xy = col(&[Some(A)])
        .map_vertices(|_, _| Some(Box::new(|x, y, z, _m| Ok((x * f64::NAN, y, z)))))
        .unwrap();
    for other in [one(A), one(B_2D), nan_xy.clone()] {
        for op in [
            SetOp::Intersection,
            SetOp::Union,
            SetOp::Difference,
            SetOp::SymmetricDifference,
        ] {
            let _ = nan_xy.set_op(&other, op);
            let _ = other.set_op(&nan_xy, op);
        }
    }
    let _ = nan_xy.union_all();
    let _ = nan_xy.segmentize(2.0);
    let _ = nan_xy.clip_by_rect(0.0, 0.0, 5.0, 5.0);
    let inf_z = one(A).force_3d(&[Some(f64::INFINITY)]).unwrap();
    let _ = inf_z.set_op(&one(B), SetOp::Union);
    let _ = one(B).set_op(&inf_z, SetOp::Intersection);
}
