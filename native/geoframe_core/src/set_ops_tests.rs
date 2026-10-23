//! Reference cases for binary set operations. Expected answers are the Shapely/GeoPandas ones.

use crate::{GeoColumn, SetOp};

fn one(wkt: &str) -> GeoColumn {
    GeoColumn::from_wkt(&[Some(wkt)], None).expect("valid WKT")
}

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

const A: &str = "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))";
const B: &str = "POLYGON ((0.5 0, 1.5 0, 1.5 1, 0.5 1, 0.5 0))";
const FAR: &str = "POLYGON ((5 5, 6 5, 6 6, 5 6, 5 5))";

fn area(c: &GeoColumn) -> f64 {
    c.area()[0].expect("area")
}

fn run(a: &str, b: &str, op: SetOp) -> GeoColumn {
    one(a).set_op(&one(b), op).expect("pairing")
}

#[test]
fn overlapping_squares_match_shapely_areas() {
    assert!((area(&run(A, B, SetOp::Intersection)) - 0.5).abs() < 1e-12);
    assert!((area(&run(A, B, SetOp::Union)) - 1.5).abs() < 1e-12);
    assert!((area(&run(A, B, SetOp::Difference)) - 0.5).abs() < 1e-12);
    assert!((area(&run(A, B, SetOp::SymmetricDifference)) - 1.0).abs() < 1e-12);
}

#[test]
fn disjoint_intersection_is_an_empty_polygon_not_null() {
    let out = run(A, FAR, SetOp::Intersection);
    assert_eq!(
        out.to_wkt().unwrap(),
        vec![Some("POLYGON EMPTY".to_string())]
    );
}

#[test]
fn disjoint_union_is_a_multipolygon_and_single_part_is_a_polygon() {
    let union = run(A, FAR, SetOp::Union);
    assert_eq!(union.geom_type(), vec![Some("MultiPolygon".to_string())]);
    let inter = run(A, B, SetOp::Intersection);
    assert_eq!(inter.geom_type(), vec![Some("Polygon".to_string())]);
}

#[test]
fn difference_with_a_hole_keeps_the_hole_area() {
    let holed = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))";
    let cover = "POLYGON ((0 0, 5 0, 5 5, 0 5, 0 0))";
    // 100 - 4 (hole) - (25 - 4) = 75
    assert!((area(&run(holed, cover, SetOp::Difference)) - 75.0).abs() < 1e-9);
}

#[test]
fn line_against_polygon_uses_clip() {
    let line = "LINESTRING (-1 0.5, 2 0.5)";
    let inter = run(line, A, SetOp::Intersection);
    assert_eq!(inter.length()[0], Some(1.0));
    let diff = run(line, A, SetOp::Difference);
    assert_eq!(diff.length()[0], Some(2.0));
    // Polygon on the left intersects symmetrically; a polygon minus a line is unchanged.
    assert_eq!(run(A, line, SetOp::Intersection).length()[0], Some(1.0));
    assert!((area(&run(A, line, SetOp::Difference)) - 1.0).abs() < 1e-12);
}

#[test]
fn line_fully_outside_gives_empty_linestring() {
    let out = run("LINESTRING (7 7, 8 8)", A, SetOp::Intersection);
    assert_eq!(
        out.to_wkt().unwrap(),
        vec![Some("LINESTRING EMPTY".to_string())]
    );
}

#[test]
fn points_keep_or_drop_by_predicate() {
    let pts = "MULTIPOINT ((0.5 0.5), (9 9))";
    let inter = run(pts, A, SetOp::Intersection);
    assert_eq!(
        inter.to_wkt().unwrap(),
        vec![Some("POINT(0.5 0.5)".to_string())]
    );
    let diff = run(pts, A, SetOp::Difference);
    assert_eq!(diff.to_wkt().unwrap(), vec![Some("POINT(9 9)".to_string())]);
    let gone = run("POINT (0.5 0.5)", A, SetOp::Difference);
    assert_eq!(
        gone.to_wkt().unwrap(),
        vec![Some("MULTIPOINT EMPTY".to_string())]
    );
    // polygon on the left, points on the right
    let back = run(A, pts, SetOp::Intersection);
    assert_eq!(
        back.to_wkt().unwrap(),
        vec![Some("POINT(0.5 0.5)".to_string())]
    );
}

#[test]
fn point_unions_deduplicate() {
    let out = run("POINT (1 1)", "MULTIPOINT ((1 1), (2 2))", SetOp::Union);
    assert_eq!(out.count_coordinates(), vec![Some(2)]);
    let sym = run(
        "POINT (1 1)",
        "MULTIPOINT ((1 1), (2 2))",
        SetOp::SymmetricDifference,
    );
    assert_eq!(sym.to_wkt().unwrap(), vec![Some("POINT(2 2)".to_string())]);
}

#[test]
fn unsupported_combinations_are_null() {
    for op in [SetOp::Union, SetOp::SymmetricDifference] {
        assert_eq!(
            run("LINESTRING (0 0, 1 1)", A, op).to_wkt().unwrap(),
            vec![None]
        );
    }
    let lines = run(
        "LINESTRING (0 0, 2 2)",
        "LINESTRING (0 2, 2 0)",
        SetOp::Intersection,
    );
    assert_eq!(lines.to_wkt().unwrap(), vec![None]);
    let gc = run(
        "GEOMETRYCOLLECTION (POINT (0 0), POINT (1 1))",
        A,
        SetOp::Union,
    );
    assert_eq!(gc.to_wkt().unwrap(), vec![None]);
}

#[test]
fn nulls_propagate_and_crs_is_kept() {
    let left = GeoColumn::from_wkt(&[Some(A), None, Some(A)], Some("EPSG:3857")).unwrap();
    let right = col(&[Some(B), Some(B), None]);
    let out = left.set_op(&right, SetOp::Intersection).unwrap();
    assert_eq!(out.crs().as_deref(), Some("EPSG:3857"));
    let wkt = out.to_wkt().unwrap();
    assert!(wkt[0].is_some() && wkt[1].is_none() && wkt[2].is_none());
}

#[test]
fn broadcasting_pairing_and_crs_rules() {
    let left = col(&[Some(A), Some(FAR)]);
    let out = left.set_op(&one(B), SetOp::Intersection).unwrap();
    assert!((out.area()[0].unwrap() - 0.5).abs() < 1e-12);
    assert_eq!(out.area()[1], Some(0.0));
    let three = col(&[Some(A), Some(A), Some(A)]);
    assert!(left
        .set_op(&three, SetOp::Union)
        .unwrap_err()
        .contains("length mismatch"));
    let a = GeoColumn::from_wkt(&[Some(A)], Some("EPSG:4326")).unwrap();
    let b = GeoColumn::from_wkt(&[Some(A)], Some("EPSG:3857")).unwrap();
    assert!(a
        .set_op(&b, SetOp::Union)
        .unwrap_err()
        .contains("CRS mismatch"));
}

#[test]
fn set_op_names_parse() {
    assert_eq!(
        SetOp::from_name("symmetric_difference"),
        Some(SetOp::SymmetricDifference)
    );
    assert_eq!(SetOp::from_name("nope"), None);
}

#[test]
fn union_all_merges_overlapping_and_touching_polygons() {
    let c = col(&[Some(A), Some(B), None, Some(FAR)]);
    let out = c.union_all().unwrap();
    assert_eq!(out.len(), 1);
    assert!((area(&out) - 2.5).abs() < 1e-12);
}

#[test]
fn union_all_handles_mixed_winding() {
    let cw = "POLYGON ((0 0, 0 1, 1 1, 1 0, 0 0))";
    let out = col(&[Some(cw), Some(B)]).union_all().unwrap();
    assert!((area(&out) - 1.5).abs() < 1e-12);
}

#[test]
fn union_all_points_lines_and_edge_cases() {
    let pts = col(&[Some("POINT (1 1)"), Some("MULTIPOINT ((1 1), (2 2))")]);
    assert_eq!(pts.union_all().unwrap().count_coordinates(), vec![Some(2)]);
    let lines = col(&[
        Some("LINESTRING (0 0, 1 1)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("LINESTRING (2 2, 3 3)"),
    ]);
    assert_eq!(
        lines.union_all().unwrap().geom_type(),
        vec![Some("MultiLineString".to_string())]
    );
    assert_eq!(
        lines.union_all().unwrap().count_coordinates(),
        vec![Some(4)]
    );
    let mixed = col(&[Some("POINT (1 1)"), Some(A)]);
    assert_eq!(mixed.union_all().unwrap().to_wkt().unwrap(), vec![None]);
    let empty = col(&[None, None]);
    assert_eq!(
        empty.union_all().unwrap().to_wkt().unwrap(),
        vec![Some("GEOMETRYCOLLECTION EMPTY".to_string())]
    );
}

const SQ0: &str = "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))";
const SQ1: &str = "POLYGON ((1 0, 2 0, 2 1, 1 1, 1 0))";
const SQ2: &str = "POLYGON ((5 5, 6 5, 6 6, 5 6, 5 5))";

#[test]
fn union_groups_unions_adjacent_squares_per_group() {
    let c = col(&[Some(SQ0), Some(SQ2), Some(SQ1)]);
    let out = c.union_groups(&[0, 1, 0], 2).unwrap();
    assert_eq!(out.len(), 2);
    let areas = out.area();
    assert!((areas[0].unwrap() - 2.0).abs() < 1e-12);
    assert!((areas[1].unwrap() - 1.0).abs() < 1e-12);
    assert_eq!(out.geom_type()[0], Some("Polygon".to_string()));
}

#[test]
fn union_groups_empty_group_and_null_rows() {
    let c = col(&[Some(SQ0), None, None]);
    let out = c.union_groups(&[0, 1, 1], 3).unwrap();
    let wkt = out.to_wkt().unwrap();
    assert!(wkt[0].as_deref().unwrap().starts_with("POLYGON"));
    assert_eq!(wkt[1].as_deref(), Some("GEOMETRYCOLLECTION EMPTY"));
    assert_eq!(wkt[2].as_deref(), Some("GEOMETRYCOLLECTION EMPTY"));
}

#[test]
fn union_groups_matches_union_all_and_keeps_crs() {
    let c = GeoColumn::from_wkt(&[Some(SQ0), Some(SQ1)], Some("EPSG:3857")).unwrap();
    let out = c.union_groups(&[0, 0], 1).unwrap();
    assert_eq!(out.crs(), Some("EPSG:3857".to_string()));
    assert_eq!(
        out.to_wkt().unwrap(),
        c.union_all().unwrap().to_wkt().unwrap()
    );
}

#[test]
fn union_groups_validates_inputs() {
    let c = col(&[Some(SQ0), Some(SQ1)]);
    assert!(c.union_groups(&[0], 1).is_err());
    assert!(c.union_groups(&[0, 2], 2).is_err());
    assert!(c.union_groups(&[], 0).is_err());
    assert_eq!(col(&[]).union_groups(&[], 0).unwrap().len(), 0);
}
