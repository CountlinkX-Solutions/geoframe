//! Batch B: mixed-dimension and line overlays, `explode`, `clip_by_rect`, `intersection_all`.
//! Expected answers are the Shapely 2.2 / GEOS 3.14 ones.

use crate::{GeoColumn, SetOp};

fn one(wkt: &str) -> GeoColumn {
    GeoColumn::from_wkt(&[Some(wkt)], None).expect("valid WKT")
}

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn run(a: &str, b: &str, op: SetOp) -> GeoColumn {
    one(a).set_op(&one(b), op).expect("pairing")
}

fn kind(c: &GeoColumn) -> String {
    c.geom_type()[0].clone().expect("a geometry")
}

fn wkt(c: &GeoColumn) -> String {
    c.to_wkt().unwrap()[0].clone().expect("a geometry")
}

fn parts(c: &GeoColumn) -> usize {
    c.count_geometries()[0].expect("count")
}

const SQ: &str = "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))";
const SQ_TOUCH: &str = "POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))";
const SQ_CORNER: &str = "POLYGON ((4 4, 8 4, 8 8, 4 8, 4 4))";
const SQ_OVERLAP: &str = "POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))";
const MPOLY: &str = "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((5 5, 8 5, 8 8, 5 8, 5 5)))";
const LINE_H: &str = "LINESTRING (0 0, 10 0)";
const LINE_CROSS: &str = "LINESTRING (2 -2, 2 6)";

// -- line x line -----------------------------------------------------------------------

#[test]
fn crossing_lines_intersect_in_a_point() {
    let out = run(LINE_H, LINE_CROSS, SetOp::Intersection);
    assert_eq!(wkt(&out), "POINT(2 0)");
}

#[test]
fn collinear_lines_intersect_in_a_segment() {
    let out = run(
        "LINESTRING (0 0, 10 0)",
        "LINESTRING (5 0, 15 0)",
        SetOp::Intersection,
    );
    assert_eq!(kind(&out), "LineString");
    assert_eq!(out.length()[0], Some(5.0));
}

#[test]
fn identical_lines_intersect_in_the_line_and_have_an_empty_difference() {
    let inter = run(LINE_H, LINE_H, SetOp::Intersection);
    assert_eq!(inter.length()[0], Some(10.0));
    let diff = run(LINE_H, LINE_H, SetOp::Difference);
    assert_eq!(wkt(&diff), "LINESTRING EMPTY");
    let sym = run(LINE_H, LINE_H, SetOp::SymmetricDifference);
    assert_eq!(wkt(&sym), "LINESTRING EMPTY");
}

#[test]
fn disjoint_lines_intersect_in_an_empty_line() {
    let out = run(
        "LINESTRING (0 0, 1 0)",
        "LINESTRING (0 5, 1 5)",
        SetOp::Intersection,
    );
    assert_eq!(wkt(&out), "LINESTRING EMPTY");
}

#[test]
fn crossing_lines_union_is_noded() {
    let out = run(LINE_H, LINE_CROSS, SetOp::Union);
    assert_eq!(kind(&out), "MultiLineString");
    assert_eq!(parts(&out), 4);
    assert!((out.length()[0].unwrap() - 18.0).abs() < 1e-12);
}

#[test]
fn partial_overlap_difference_and_symmetric_difference() {
    let a = "LINESTRING (0 0, 10 0)";
    let b = "LINESTRING (5 0, 15 0)";
    let diff = run(a, b, SetOp::Difference);
    assert_eq!(diff.length()[0], Some(5.0));
    let sym = run(a, b, SetOp::SymmetricDifference);
    assert_eq!(sym.length()[0], Some(10.0));
    let union = run(a, b, SetOp::Union);
    assert_eq!(union.length()[0], Some(15.0));
}

#[test]
fn plain_vertices_do_not_cut_a_line_but_nodes_do() {
    let out = run(
        "LINESTRING (0 0, 5 0, 10 0)",
        "LINESTRING (2 -2, 2 2)",
        SetOp::Union,
    );
    assert_eq!(parts(&out), 4);
    // (0 0, 2 0) + (2 0, 5 0, 10 0) + the two halves of the crossing line
    assert_eq!(out.count_coordinates(), vec![Some(9)]);
}

#[test]
fn a_vertex_where_both_lines_have_a_vertex_is_a_node() {
    // GEOS keeps (0 0, 3 4) and (3 4, 6 4) apart here as well.
    let out = run(
        "LINESTRING (0 0, 3 4, 6 4)",
        "LINESTRING (0 0, 3 4, 6 4)",
        SetOp::Intersection,
    );
    assert_eq!(parts(&out), 2);
    assert!((out.length()[0].unwrap() - 8.0).abs() < 1e-12);
}

// -- polygon x polygon lower-dimensional parts -------------------------------------------

#[test]
fn touching_squares_intersect_in_their_shared_edge() {
    let out = run(SQ, SQ_TOUCH, SetOp::Intersection);
    assert_eq!(kind(&out), "LineString");
    assert_eq!(out.length()[0], Some(4.0));
}

#[test]
fn corner_touching_squares_intersect_in_a_point() {
    let out = run(SQ, SQ_CORNER, SetOp::Intersection);
    assert_eq!(wkt(&out), "POINT(4 4)");
}

#[test]
fn overlap_plus_isolated_touch_point_is_a_collection() {
    let out = run(MPOLY, SQ_OVERLAP, SetOp::Intersection);
    assert_eq!(kind(&out), "GeometryCollection");
    assert_eq!(parts(&out), 2);
    assert!((out.area()[0].unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn shared_edges_inside_the_overlap_are_not_reported_twice() {
    // The overlap polygon's boundary already contains the shared parts.
    let out = run(
        SQ,
        "POLYGON ((0 0, 4 0, 4 2, 0 2, 0 0))",
        SetOp::Intersection,
    );
    assert_eq!(kind(&out), "Polygon");
}

// -- line x polygon ----------------------------------------------------------------------

#[test]
fn line_touching_a_polygon_at_a_point_intersects_in_that_point() {
    let out = run("LINESTRING (4 4, 6 6)", SQ, SetOp::Intersection);
    assert_eq!(wkt(&out), "POINT(4 4)");
    let swapped = run(SQ, "LINESTRING (4 4, 6 6)", SetOp::Intersection);
    assert_eq!(wkt(&swapped), "POINT(4 4)");
}

#[test]
fn polygon_union_line_keeps_the_outside_part() {
    let out = run(SQ, LINE_H, SetOp::Union);
    assert_eq!(kind(&out), "GeometryCollection");
    assert_eq!(parts(&out), 2);
    assert!((out.area()[0].unwrap() - 16.0).abs() < 1e-12);
    assert!((out.length()[0].unwrap() - (16.0 + 6.0)).abs() < 1e-12);
}

#[test]
fn polygon_union_line_inside_is_the_polygon() {
    let out = run(SQ, "LINESTRING (1 1, 3 3)", SetOp::Union);
    assert_eq!(kind(&out), "Polygon");
}

#[test]
fn line_union_polygon_and_symmetric_difference_agree_with_union() {
    for op in [SetOp::Union, SetOp::SymmetricDifference] {
        let out = run(LINE_CROSS, SQ, op);
        assert_eq!(kind(&out), "GeometryCollection");
        assert_eq!(parts(&out), 3);
    }
}

// -- points x anything -------------------------------------------------------------------

#[test]
fn point_union_polygon() {
    assert_eq!(kind(&run("POINT (1 1)", SQ, SetOp::Union)), "Polygon");
    let far = run("POINT (9 9)", SQ, SetOp::Union);
    assert_eq!(kind(&far), "GeometryCollection");
    assert_eq!(parts(&far), 2);
    let sym = run(SQ, "POINT (9 9)", SetOp::SymmetricDifference);
    assert_eq!(kind(&sym), "GeometryCollection");
}

#[test]
fn point_union_line() {
    assert_eq!(
        kind(&run("POINT (2 0)", LINE_H, SetOp::Union)),
        "LineString"
    );
    let off = run(LINE_H, "MULTIPOINT ((2 0), (5 5))", SetOp::Union);
    assert_eq!(kind(&off), "GeometryCollection");
    assert_eq!(parts(&off), 2);
}

#[test]
fn only_geometry_collections_remain_unsupported() {
    let gc = run(
        "GEOMETRYCOLLECTION (POINT (0 0), POINT (1 1))",
        SQ,
        SetOp::Union,
    );
    assert_eq!(gc.to_wkt().unwrap(), vec![None]);
}

// -- explode -----------------------------------------------------------------------------

#[test]
fn explode_splits_multi_geometries_and_collections_one_level() {
    let c = col(&[
        Some("POINT (1 1)"),
        Some("MULTIPOINT ((1 1), (2 2))"),
        Some("GEOMETRYCOLLECTION (POINT (1 1), GEOMETRYCOLLECTION (POINT (3 3)), MULTIPOINT ((5 5), (6 6)))"),
        Some(MPOLY),
    ]);
    let (parts, source) = c.explode().unwrap();
    assert_eq!(source, vec![0, 1, 1, 2, 2, 2, 3, 3]);
    assert_eq!(parts.len(), 8);
    let kinds: Vec<_> = parts.geom_type().into_iter().flatten().collect();
    assert_eq!(kinds[5], "MultiPoint");
    // Storage flattens nested collections, so a collection's members are always atoms.
    assert_eq!(kinds[4], "Point");
}

#[test]
fn explode_drops_nulls_and_empty_collections_but_keeps_empty_atomic_geometries() {
    let c = col(&[
        None,
        Some("POLYGON EMPTY"),
        Some("MULTIPOINT EMPTY"),
        Some("GEOMETRYCOLLECTION EMPTY"),
        Some("POINT (1 1)"),
    ]);
    let (parts, source) = c.explode().unwrap();
    assert_eq!(source, vec![1, 4]);
    assert_eq!(parts.len(), 2);
}

#[test]
fn explode_keeps_the_crs() {
    let c = GeoColumn::from_wkt(&[Some(MPOLY)], Some("EPSG:3857")).unwrap();
    let (parts, _) = c.explode().unwrap();
    assert_eq!(parts.crs().as_deref(), Some("EPSG:3857"));
}

// -- clip_by_rect ------------------------------------------------------------------------

fn clip(wkt_text: &str) -> GeoColumn {
    one(wkt_text).clip_by_rect(0.0, 0.0, 10.0, 10.0).unwrap()
}

#[test]
fn clip_by_rect_clips_polygons_lines_and_points() {
    let poly = clip("POLYGON ((-5 -5, 5 -5, 5 5, -5 5, -5 -5))");
    assert!((poly.area()[0].unwrap() - 25.0).abs() < 1e-9);
    let line = clip("LINESTRING (-5 5, 5 5)");
    assert_eq!(wkt(&line), "LINESTRING(0 5,5 5)");
    assert_eq!(wkt(&clip("POINT (1 1)")), "POINT(1 1)");
}

#[test]
fn clip_by_rect_drops_boundary_only_contact() {
    for geometry in [
        "POINT (10 5)",
        "POINT (11 1)",
        "LINESTRING (0 0, 0 5)",
        "LINESTRING (-5 -5, 0 0)",
        "POLYGON ((10 0, 15 0, 15 5, 10 5, 10 0))",
    ] {
        assert_eq!(
            wkt(&clip(geometry)),
            "GEOMETRYCOLLECTION EMPTY",
            "{geometry}"
        );
    }
}

#[test]
fn clip_by_rect_keeps_holes_and_multi_parts() {
    let holed = clip("POLYGON ((-5 -5, 15 -5, 15 15, -5 15, -5 -5), (2 2, 4 2, 4 4, 2 4, 2 2))");
    assert!((holed.area()[0].unwrap() - 96.0).abs() < 1e-9);
    assert_eq!(wkt(&clip("MULTIPOINT (1 1, 20 20)")), "POINT(1 1)");
}

// -- intersection_all --------------------------------------------------------------------

#[test]
fn intersection_all_folds_every_non_null_row() {
    let c = col(&[Some(SQ), None, Some(SQ_OVERLAP)]);
    let out = c.intersection_all().unwrap();
    assert_eq!(out.len(), 1);
    assert!((out.area()[0].unwrap() - 4.0).abs() < 1e-9);
}

#[test]
fn intersection_all_of_touching_polygons_is_their_edge() {
    let c = col(&[Some(SQ), Some(SQ_TOUCH)]);
    assert_eq!(wkt(&c.intersection_all().unwrap()), "LINESTRING(4 0,4 4)");
}

#[test]
fn intersection_all_of_nothing_is_an_empty_collection() {
    for rows in [vec![], vec![None, None]] {
        let out = col(&rows).intersection_all().unwrap();
        assert_eq!(wkt(&out), "GEOMETRYCOLLECTION EMPTY");
    }
}
