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
