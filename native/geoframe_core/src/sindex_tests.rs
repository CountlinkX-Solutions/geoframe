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

// -- nearest search ---------------------------------------------------------------------

fn nearest(
    l: &GeoColumn,
    r: &GeoColumn,
    max_distance: Option<f64>,
    exclusive: bool,
) -> Vec<(u32, u32, f64)> {
    let (li, ri, d) = l
        .nearest_pairs(r, max_distance, exclusive)
        .expect("nearest");
    li.into_iter()
        .zip(ri)
        .zip(d)
        .map(|((l, r), d)| (l, r, d))
        .collect()
}

fn close(actual: &[(u32, u32, f64)], expected: &[(u32, u32, f64)]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert_eq!((a.0, a.1), (e.0, e.1), "{actual:?} vs {expected:?}");
        assert!((a.2 - e.2).abs() < 1e-9, "{actual:?} vs {expected:?}");
    }
}

#[test]
fn nearest_points_to_points() {
    let right = col(&["POINT (0 0)", "POINT (10 0)", "POINT (0 10)"]);
    let left = col(&["POINT (1 0)", "POINT (9 1)", "POINT (0 8)"]);
    close(
        &nearest(&left, &right, None, false),
        &[(0, 0, 1.0), (1, 1, 2f64.sqrt()), (2, 2, 2.0)],
    );
}

#[test]
fn nearest_is_geometry_distance_not_centroid() {
    // The long line's centroid is far from the point, but the line passes 1 away from it;
    // the nearby point has a closer centroid-to-point distance only at 4.
    let right = col(&["LINESTRING (-100 1, 100 1)", "POINT (4 0)"]);
    let left = col(&["POINT (0 0)"]);
    close(&nearest(&left, &right, None, false), &[(0, 0, 1.0)]);
}

#[test]
fn nearest_inside_polygon_is_zero_and_polygons_are_supported() {
    let right = col(&[
        "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
        "POLYGON ((10 0, 14 0, 14 4, 10 4, 10 0))",
    ]);
    let left = col(&["POINT (2 2)", "POINT (7 2)", "POINT (12 2)", "POINT (20 2)"]);
    close(
        &nearest(&left, &right, None, false),
        // (7 2) is 3 from the first polygon and 3 from the second: a tie.
        &[
            (0, 0, 0.0),
            (1, 0, 3.0),
            (1, 1, 3.0),
            (2, 1, 0.0),
            (3, 1, 6.0),
        ],
    );
}

#[test]
fn nearest_returns_all_ties_ordered_by_right_row() {
    let right = col(&["POINT (1 0)", "POINT (-1 0)", "POINT (0 1)", "POINT (5 5)"]);
    let left = col(&["POINT (0 0)"]);
    close(
        &nearest(&left, &right, None, false),
        &[(0, 0, 1.0), (0, 1, 1.0), (0, 2, 1.0)],
    );
}

#[test]
fn nearest_max_distance_is_an_inclusive_cutoff() {
    let right = col(&["POINT (3 4)", "POINT (30 40)"]);
    let left = col(&["POINT (0 0)", "POINT (100 100)"]);
    close(&nearest(&left, &right, Some(5.0), false), &[(0, 0, 5.0)]);
    assert!(nearest(&left, &right, Some(4.99), false).is_empty());
    // A zero cutoff keeps only touching geometries.
    let zero = col(&["POINT (3 4)"]);
    close(&nearest(&zero, &right, Some(0.0), false), &[(0, 0, 0.0)]);
}

#[test]
fn nearest_exclusive_skips_equal_geometries() {
    let right = col(&["POINT (0 0)", "POINT (2 0)", "POINT (0 0)"]);
    let left = col(&["POINT (0 0)"]);
    close(
        &nearest(&left, &right, None, false),
        &[(0, 0, 0.0), (0, 2, 0.0)],
    );
    close(&nearest(&left, &right, None, true), &[(0, 1, 2.0)]);
    // Same frame against itself: each row's nearest other geometry.
    let pts = col(&["POINT (0 0)", "POINT (1 0)", "POINT (5 0)"]);
    close(
        &nearest(&pts, &pts, None, true),
        &[(0, 1, 1.0), (1, 0, 1.0), (2, 1, 4.0)],
    );
}

#[test]
fn nearest_skips_nulls_and_empties_and_validates_input() {
    let left =
        GeoColumn::from_wkt(&[Some("POINT (1 1)"), None, Some("POINT EMPTY")], None).unwrap();
    let right =
        GeoColumn::from_wkt(&[None, Some("POINT EMPTY"), Some("POINT (4 5)")], None).unwrap();
    close(&nearest(&left, &right, None, false), &[(0, 2, 5.0)]);
    let none = GeoColumn::from_wkt::<&str>(&[], None).unwrap();
    assert!(nearest(&left, &none, None, false).is_empty());
    assert!(nearest(&none, &right, None, false).is_empty());

    let a = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:4326")).unwrap();
    let b = GeoColumn::from_wkt(&[Some("POINT (0 0)")], Some("EPSG:3857")).unwrap();
    assert!(a
        .nearest_pairs(&b, None, false)
        .unwrap_err()
        .contains("CRS mismatch"));
    assert!(a.nearest_pairs(&a, Some(-1.0), false).is_err());
    assert!(a.nearest_pairs(&a, Some(f64::NAN), false).is_err());
}

#[test]
fn nearest_matches_brute_force_on_a_lattice() {
    let right_wkt: Vec<String> = (0..40)
        .map(|i| {
            let (x, y) = ((i * 7) % 13, (i * 5) % 11);
            if i % 3 == 0 {
                square(x, y)
            } else {
                format!("POINT ({x} {y})")
            }
        })
        .collect();
    let left_wkt: Vec<String> = (0..60)
        .map(|i| format!("POINT ({} {})", (i * 3) % 17 - 2, (i * 4) % 15 - 2))
        .collect();
    let right = col(&right_wkt.iter().map(String::as_str).collect::<Vec<_>>());
    let left = col(&left_wkt.iter().map(String::as_str).collect::<Vec<_>>());
    let mut expected = Vec::new();
    for (i, left_row) in left_wkt.iter().enumerate() {
        let li = col(&[left_row.as_str()]);
        let ds: Vec<f64> = right_wkt
            .iter()
            .map(|r| li.distance(&col(&[r.as_str()])).unwrap()[0].unwrap())
            .collect();
        let min = ds.iter().cloned().fold(f64::INFINITY, f64::min);
        for (j, d) in ds.iter().enumerate() {
            if *d == min {
                expected.push((i as u32, j as u32, min));
            }
        }
    }
    close(&nearest(&left, &right, None, false), &expected);
}
