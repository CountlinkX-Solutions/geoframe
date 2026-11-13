//! A zero-area (collinear) polygon ring is invalid, and `make_valid` collapses it to its
//! linework, as GEOS does. Expected values come from Shapely 2.2.0 / GEOS 3.14.1:
//!
//! ```python
//! from shapely import wkt
//! from shapely.validation import explain_validity, make_valid
//! g = wkt.loads("POLYGON ((0 0, 1 1, 2 2, 0 0))")
//! g.is_valid                  # False
//! explain_validity(g)         # 'Self-intersection[1 1]'
//! make_valid(g).wkt           # 'MULTILINESTRING ((0 0, 1 1), (1 1, 2 2))'
//! ```

use crate::{GeoColumn, UnaryOp};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn valid(wkt: &str) -> Option<bool> {
    col(&[Some(wkt)]).is_valid()[0]
}

fn norm(wkt: &str) -> String {
    col(&[Some(wkt)]).to_wkt().unwrap().remove(0).unwrap()
}

fn repaired(wkt: &str) -> String {
    col(&[Some(wkt)])
        .unary(UnaryOp::MakeValid)
        .unwrap()
        .to_wkt()
        .unwrap()
        .remove(0)
        .unwrap()
}

#[test]
fn a_collinear_ring_is_invalid() {
    for wkt in [
        "POLYGON ((0 0, 1 1, 2 2, 0 0))",
        "POLYGON ((0 0, 1 1, 1 1, 2 2, 0 0))",
        "POLYGON ((0 0, 2 0, 1 0, 0 0))",
        "POLYGON ((0 0, 1 0, 0 0, 0 0))",
        "POLYGON ((0 0, 0 0, 0 0, 0 0))",
        "POLYGON Z ((0 0 1, 1 1 2, 2 2 3, 0 0 1))",
        "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 2, 3 3, 1 1))",
        "MULTIPOLYGON (((0 0, 1 1, 2 2, 0 0)), ((5 5, 6 5, 6 6, 5 5)))",
    ] {
        assert_eq!(valid(wkt), Some(false), "{wkt}");
    }
}

#[test]
fn other_validity_is_unchanged() {
    assert_eq!(valid("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"), Some(true));
    assert_eq!(valid("POLYGON ((0 0, 1 0, 1 1, 0 0))"), Some(true));
    assert_eq!(valid("POLYGON EMPTY"), Some(true));
    assert_eq!(valid("POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))"), Some(false));
}

#[test]
fn the_reason_of_a_collinear_ring_is_not_valid_geometry() {
    let reasons = col(&[Some("POLYGON ((0 0, 1 1, 2 2, 0 0))")]).is_valid_reason();
    assert_ne!(reasons[0].as_deref(), Some("Valid Geometry"));
    assert!(reasons[0].as_ref().is_some_and(|r| !r.is_empty()));
}

#[test]
fn null_out_invalid_drops_a_collinear_ring() {
    let out = col(&[Some("POLYGON ((0 0, 1 1, 2 2, 0 0))")])
        .null_out_invalid()
        .unwrap();
    assert_eq!(out.to_wkt().unwrap(), vec![None]);
}

#[test]
fn make_valid_collapses_a_collinear_polygon_to_its_linework() {
    // Shapely: noded at every vertex, in the direction of the ring's first segment.
    for (input, want) in [
        (
            "POLYGON ((0 0, 1 1, 2 2, 0 0))",
            "MULTILINESTRING ((0 0, 1 1), (1 1, 2 2))",
        ),
        (
            "POLYGON ((2 2, 1 1, 0 0, 2 2))",
            "MULTILINESTRING ((2 2, 1 1), (1 1, 0 0))",
        ),
        (
            "POLYGON ((0 0, 3 3, 1 1, 2 2, 0 0))",
            "MULTILINESTRING ((0 0, 1 1), (1 1, 2 2), (2 2, 3 3))",
        ),
        (
            "POLYGON ((0 5, 0 3, 0 1, 0 5))",
            "MULTILINESTRING ((0 5, 0 3), (0 3, 0 1))",
        ),
        (
            "POLYGON ((0 0, 1 0, 2 0, 1 0, 0 0))",
            "MULTILINESTRING ((0 0, 1 0), (1 0, 2 0))",
        ),
        ("POLYGON ((0 0, 1 0, 0 0, 0 0))", "LINESTRING (0 0, 1 0)"),
        ("POLYGON ((0 0, 1 1, 0 0))", "LINESTRING (0 0, 1 1)"),
        ("POLYGON ((0 0, 0 0, 0 0, 0 0))", "POINT (0 0)"),
    ] {
        assert_eq!(repaired(input), norm(want), "{input}");
    }
}

#[test]
fn make_valid_keeps_the_valid_parts_beside_the_collapsed_ones() {
    // Shapely: a collapsed part becomes lines, the valid polygon stays, polygons first (GEOS
    // lists the polygon's ring clockwise, here it keeps the input order: vertex detail).
    assert_eq!(
        repaired("MULTIPOLYGON (((0 0, 1 1, 2 2, 0 0)), ((5 5, 6 5, 6 6, 5 5)))"),
        norm("GEOMETRYCOLLECTION (POLYGON ((5 5, 6 5, 6 6, 5 5)), MULTILINESTRING ((0 0, 1 1), (1 1, 2 2)))")
    );
    // A collapsed hole becomes a line beside the polygon.
    assert_eq!(
        repaired("POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0), (1 1, 2 1, 1 1, 1 1))"),
        norm("GEOMETRYCOLLECTION (POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0)), LINESTRING (1 1, 2 1))")
    );
}

#[test]
fn make_valid_of_a_collinear_polygon_with_z_keeps_z() {
    assert_eq!(
        repaired("POLYGON Z ((0 0 1, 2 2 5, 1 1 3, 0 0 1))"),
        norm("MULTILINESTRING Z ((0 0 1, 1 1 3), (1 1 3, 2 2 5))")
    );
}

// -- is_valid_reason in GEOS' words ---------------------------------------------------------
//
// GEOS' `IsValidOp` reports the first error in its check order as `<text>[<x> <y>]`:
// coordinates, too few points, area intersections (`Self-intersection` for a crossing or an
// overlap, `Ring Self-intersection` for a ring touching itself), hole outside shell, nested
// holes (`Holes are nested`), nested shells, disconnected interior. Expected values:
//
// ```python
// from shapely import wkt, is_valid_reason
// is_valid_reason(wkt.loads("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))"))   # 'Self-intersection[2 2]'
// ```

fn reason(wkt: &str) -> String {
    col(&[Some(wkt)]).is_valid_reason().remove(0).unwrap()
}

/// Cases where GeoFrame reproduces GEOS' text and coordinate.
const GEOS_REASONS: &[(&str, &str)] = &[
    ("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))", "Self-intersection[2 2]"),  // bowtie
    ("LINESTRING (1 1, 1 1)", "Too few points in geometry component[1 1]"),  // line_zero
    ("LINESTRING (3 4, 3 4, 3 4)", "Too few points in geometry component[3 4]"),  // line_one_pt
    ("POLYGON ((0 0, 1 1, 2 2, 0 0))", "Self-intersection[1 1]"),  // coll_ring
    ("POLYGON ((1 1, 1 1, 1 1, 1 1))", "Too few points in geometry component[1 1]"),  // zero_ring
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (20 20, 21 20, 21 21, 20 21, 20 20))", "Hole lies outside shell[20 20]"),  // hole_out
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (5 5, 15 5, 15 6, 5 6, 5 5))", "Self-intersection[10 5]"),  // hole_cross
    ("POLYGON ((0 0, 20 0, 20 20, 0 20, 0 0), (2 2, 18 2, 18 18, 2 18, 2 2), (5 5, 8 5, 8 8, 5 8, 5 5))", "Holes are nested[5 5]"),  // nested_holes
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (0 5, 5 0, 5 1, 1 5, 0 5), (5 0, 10 5, 9 5, 5 1, 5 0))", "Self-intersection[5 1]"),  // disconnected
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0)), ((2 2, 4 2, 4 4, 2 4, 2 2)))", "Nested shells[2 2]"),  // nested_shells
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0)), ((10 0, 20 0, 20 10, 10 10, 10 0)))", "Self-intersection[10 10]"),  // mp_edge
    ("POLYGON ((0 0, 10 0, 10 10, 5 0, 5 5, 0 5, 0 0))", "Ring Self-intersection[5 0]"),  // ring_selfint
    ("POLYGON ((0 0, 10 0, 10 10, 5 5, 5 10, 5 5, 0 10, 0 0))", "Ring Self-intersection[5 5]"),  // inverted
    ("POLYGON ((0 0, 1 1, 0 0))", "Too few points in geometry component[0 0]"),  // few4
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 3 3, 2 2))", "Too few points in geometry component[2 2]"),  // few_hole
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 6 2, 6 6, 2 6, 2 2, 4 4, 2 2))", "Ring Self-intersection[2 2]"),  // hole_selftouch
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (1 1, 5 1, 5 5, 1 5, 1 1), (5 1, 9 1, 9 5, 5 5, 5 1))", "Self-intersection[5 5]"),  // holes_touch
    ("LINESTRING (0 0, 1 1)", "Valid Geometry"),  // line_ok
    ("GEOMETRYCOLLECTION (POINT (1 1), POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0)))", "Self-intersection[2 2]"),  // gc_bad
    ("GEOMETRYCOLLECTION (LINESTRING (1 1, 1 1), POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0)))", "Too few points in geometry component[1 1]"),  // gc_bad2
    ("MULTILINESTRING ((0 0, 1 1), (2 2, 2 2))", "Too few points in geometry component[2 2]"),  // mls_bad
    ("MULTIPOINT ((1 1), (2 2))", "Valid Geometry"),  // mpt
    ("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0), (10 10, 11 10, 11 11, 10 11, 10 10))", "Self-intersection[2 2]"),  // two_selfint
    ("POLYGON ((0 0, 10 0, 10 10, 5 5, 0 10, 0 0))", "Valid Geometry"),  // ring_closed_selftouch
    ("POLYGON ((0 0, 4 0, 4 4, 2 0, 0 4, 0 0))", "Ring Self-intersection[2 0]"),  // shell_selftouch
    ("POLYGON ((0 0, 3 0, 3 3, 1 -1, 0 3, 0 0))", "Self-intersection[1.5 0]"),  // selfint_x
    ("POLYGON EMPTY", "Valid Geometry"),  // epoly
    ("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((5 5, 9 9, 9 5, 5 9, 5 5)))", "Self-intersection[7 7]"),  // mp_member_bad
    ("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0), (20 20, 21 20, 21 21, 20 21, 20 20))", "Self-intersection[2 2]"),  // hole_outside_and_bowtie
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (30 30, 31 30, 31 31, 30 31, 30 30), (20 20, 21 20, 21 21, 20 21, 20 20))", "Hole lies outside shell[30 30]"),  // disjoint_holes_outside_two
];

/// Several errors of one class: GEOS reports the one its monotone-chain traversal meets
/// first, GeoFrame the first in ring and segment order. The text agrees, the coordinate may not.
const GEOS_REASONS_OTHER_COORDINATE: &[(&str, &str)] = &[
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (0 0, 5 0, 5 5, 0 5, 0 0))", "Self-intersection[0 5]"),
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0)), ((0 0, 10 0, 10 10, 0 10, 0 0)))", "Self-intersection[10 10]"),
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0)), ((5 5, 15 5, 15 15, 5 15, 5 5)))", "Self-intersection[5 10]"),
    ("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (1 1, 6 1, 6 6, 1 6, 1 1), (4 4, 9 4, 9 9, 4 9, 4 4))", "Self-intersection[4 6]"),
];

#[test]
fn several_errors_of_one_class_keep_geos_text() {
    for (wkt, expected) in GEOS_REASONS_OTHER_COORDINATE {
        let (text, _) = expected.split_once('[').unwrap();
        assert_eq!(reason(wkt).split_once('[').unwrap().0, text, "{wkt}");
    }
}

#[test]
fn the_reason_is_geos_text_and_coordinate() {
    let bad: Vec<String> = GEOS_REASONS
        .iter()
        .filter(|(wkt, expected)| reason(wkt) != *expected)
        .map(|(wkt, expected)| format!("{wkt}: {} vs {expected}", reason(wkt)))
        .collect();
    assert!(bad.is_empty(), "{bad:#?}");
}
