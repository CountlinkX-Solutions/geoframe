//! Reference cases for the R-tree pair query.

use crate::{GeoColumn, Predicate, QueryPredicate};

fn col(rows: &[&str]) -> GeoColumn {
    let rows: Vec<Option<&str>> = rows.iter().map(|r| Some(*r)).collect();
    GeoColumn::from_wkt(&rows, None).expect("valid WKT")
}

fn square(x: i32, y: i32) -> String {
    format!(
        "POLYGON (({x} {y}, {} {y}, {} {}, {x} {}, {x} {y}))",
        x + 2,
        x + 2,
        y + 2,
        y + 2
    )
}

fn pairs(l: &GeoColumn, r: &GeoColumn, p: QueryPredicate) -> Vec<(u32, u32)> {
    let (li, ri) = l.query_pairs(r, p).expect("query");
    li.into_iter().zip(ri).collect()
}

fn rel(p: Predicate) -> QueryPredicate {
    QueryPredicate::Relation(p)
}

#[test]
fn points_in_a_grid_of_squares() {
    // 2x2 grid of 2x2 squares: ids 0..4, sharing edges.
    let squares: Vec<String> = [(0, 0), (2, 0), (0, 2), (2, 2)]
        .iter()
        .map(|&(x, y)| square(x, y))
        .collect();
    let right = col(&squares.iter().map(String::as_str).collect::<Vec<_>>());
    let points = col(&["POINT (1 1)", "POINT (3 3)", "POINT (9 9)", "POINT (2 2)"]);
    // within: boundary points are not within any square; (2 2) is on all four boundaries.
    assert_eq!(
        pairs(&points, &right, rel(Predicate::Within)),
        vec![(0, 0), (1, 3)]
    );
    // intersects: (2 2) touches all four squares, in right order.
    assert_eq!(
        pairs(&points, &right, rel(Predicate::Intersects)),
        vec![(0, 0), (1, 3), (3, 0), (3, 1), (3, 2), (3, 3)]
    );
    assert_eq!(
        pairs(&points, &right, rel(Predicate::CoveredBy)),
        vec![(0, 0), (1, 3), (3, 0), (3, 1), (3, 2), (3, 3)]
    );
    assert_eq!(
        pairs(&points, &right, rel(Predicate::Touches)),
        vec![(3, 0), (3, 1), (3, 2), (3, 3)]
    );
}

#[test]
fn predicate_direction_is_left_predicate_right() {
    let polys = col(&["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"]);
    let points = col(&["POINT (2 2)", "POINT (0 2)"]);
    // polygon contains point: only the interior point.
    assert_eq!(
        pairs(&polys, &points, rel(Predicate::Contains)),
        vec![(0, 0)]
    );
    assert_eq!(
        pairs(&polys, &points, rel(Predicate::Covers)),
        vec![(0, 0), (0, 1)]
    );
    // point within polygon is the mirror, with the sides swapped.
    assert_eq!(pairs(&points, &polys, rel(Predicate::Within)), vec![(0, 0)]);
    // point contains polygon: never.
    assert!(pairs(&points, &polys, rel(Predicate::Contains)).is_empty());
}

#[test]
fn dwithin_expands_the_probe() {
    let right = col(&["POINT (10 0)", "POINT (0 5)", "POINT (100 100)"]);
    let left = col(&["POINT (0 0)"]);
    assert_eq!(
        pairs(&left, &right, QueryPredicate::DWithin(5.0)),
        vec![(0, 1)]
    );
    assert_eq!(
        pairs(&left, &right, QueryPredicate::DWithin(10.0)),
        vec![(0, 0), (0, 1)]
    );
    // Diagonal: envelope overlaps but exact distance is 7.07 > 6.
    let diag = col(&["POINT (5 5)"]);
    assert!(pairs(&left, &diag, QueryPredicate::DWithin(6.0)).is_empty());
    assert_eq!(
        pairs(&left, &diag, QueryPredicate::DWithin(7.1)),
        vec![(0, 0)]
    );
}

#[test]
fn nulls_and_empties_never_match() {
    let left =
        GeoColumn::from_wkt(&[Some("POINT (1 1)"), None, Some("POINT EMPTY")], None).unwrap();
    let right = GeoColumn::from_wkt(
        &[
            None,
            Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"),
            Some("POINT EMPTY"),
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        pairs(&left, &right, rel(Predicate::Intersects)),
        vec![(0, 1)]
    );
}

#[test]
fn empty_columns_and_ordering() {
    let none = GeoColumn::from_wkt::<&str>(&[], None).unwrap();
    let some = col(&["POINT (0 0)"]);
    assert!(pairs(&none, &some, rel(Predicate::Intersects)).is_empty());
    assert!(pairs(&some, &none, rel(Predicate::Intersects)).is_empty());
    // Output is ordered by (left, right) regardless of R-tree traversal order.
    let big = col(&["POINT (0 0)", "POINT (0 0)", "POINT (0 0)", "POINT (1 1)"]);
    let l = col(&["POINT (0 0)", "POINT (1 1)", "POINT (0 0)"]);
    assert_eq!(
        pairs(&l, &big, rel(Predicate::Intersects)),
        vec![(0, 0), (0, 1), (0, 2), (1, 3), (2, 0), (2, 1), (2, 2)]
    );
}

#[test]
fn crs_mismatch_and_bad_predicates_are_errors() {
    let a = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:4326")).unwrap();
    let b = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:3857")).unwrap();
    assert!(a
        .query_pairs(&b, rel(Predicate::Intersects))
        .unwrap_err()
        .contains("CRS mismatch"));
    assert!(QueryPredicate::from_name("disjoint", None).is_err());
    assert!(QueryPredicate::from_name("nope", None).is_err());
    assert!(QueryPredicate::from_name("dwithin", None).is_err());
    assert!(QueryPredicate::from_name("dwithin", Some(-1.0)).is_err());
    assert!(QueryPredicate::from_name("dwithin", Some(f64::NAN)).is_err());
    assert_eq!(
        QueryPredicate::from_name("within", None),
        Ok(rel(Predicate::Within))
    );
}
