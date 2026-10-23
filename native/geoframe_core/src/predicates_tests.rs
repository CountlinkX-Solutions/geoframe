//! Reference cases for binary predicates. Expected answers are the Shapely/GeoPandas ones.

use crate::{GeoColumn, Predicate};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

const SQUARE: &str = "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))";

fn pred(a: &GeoColumn, b: &GeoColumn, p: Predicate) -> Vec<Option<bool>> {
    a.predicate(b, p).expect("same length")
}

#[test]
fn point_inside_boundary_outside_a_polygon() {
    let points = col(&[
        Some("POINT (2 2)"),
        Some("POINT (0 2)"),
        Some("POINT (9 9)"),
    ]);
    let sq = one(SQUARE);
    // contains: boundary points are NOT contained; covers: they are.
    let sq3 = sq.take(&[0, 0, 0]).unwrap();
    assert_eq!(
        pred(&sq3, &points, Predicate::Contains),
        vec![Some(true), Some(false), Some(false)]
    );
    assert_eq!(
        pred(&sq3, &points, Predicate::Covers),
        vec![Some(true), Some(true), Some(false)]
    );
    assert_eq!(
        pred(&points, &sq3, Predicate::Within),
        vec![Some(true), Some(false), Some(false)]
    );
    assert_eq!(
        pred(&points, &sq3, Predicate::CoveredBy),
        vec![Some(true), Some(true), Some(false)]
    );
    assert_eq!(
        pred(&points, &sq3, Predicate::Touches),
        vec![Some(false), Some(true), Some(false)]
    );
    assert_eq!(
        pred(&points, &sq3, Predicate::Intersects),
        vec![Some(true), Some(true), Some(false)]
    );
    assert_eq!(
        pred(&points, &sq3, Predicate::Disjoint),
        vec![Some(false), Some(false), Some(true)]
    );
}

#[test]
fn touching_crossing_overlapping_equal() {
    let a = col(&[
        Some(SQUARE),                  // touching squares
        Some("LINESTRING (0 0, 4 4)"), // crossing lines
        Some(SQUARE),                  // overlapping polygons
        Some(SQUARE),                  // equal, different vertex order
    ]);
    let b = col(&[
        Some("POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))"),
        Some("LINESTRING (0 4, 4 0)"),
        Some("POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))"),
        Some("POLYGON ((0 4, 4 4, 4 0, 0 0, 0 4))"),
    ]);
    assert_eq!(
        pred(&a, &b, Predicate::Touches),
        vec![Some(true), Some(false), Some(false), Some(false)]
    );
    assert_eq!(
        pred(&a, &b, Predicate::Crosses),
        vec![Some(false), Some(true), Some(false), Some(false)]
    );
    assert_eq!(
        pred(&a, &b, Predicate::Overlaps),
        vec![Some(false), Some(false), Some(true), Some(false)]
    );
    assert_eq!(
        pred(&a, &b, Predicate::Equals),
        vec![Some(false), Some(false), Some(false), Some(true)]
    );
    assert_eq!(
        pred(&a, &b, Predicate::Intersects),
        vec![Some(true), Some(true), Some(true), Some(true)]
    );
}

#[test]
fn nulls_on_either_side_give_null() {
    let a = col(&[Some("POINT (1 1)"), None, Some("POINT (1 1)")]);
    let b = col(&[Some(SQUARE), Some(SQUARE), None]);
    for p in [Predicate::Intersects, Predicate::Within, Predicate::Touches] {
        let out = pred(&a, &b, p);
        assert_eq!(out[1], None);
        assert_eq!(out[2], None);
    }
    assert_eq!(a.distance(&b).unwrap(), vec![Some(0.0), None, None]);
    assert_eq!(a.relate(&b).unwrap()[1], None);
}

#[test]
fn empty_geometries_are_null_like_other_unrepresentable_rows() {
    let a = col(&[Some("POINT EMPTY"), Some("POINT (1 1)")]);
    let b = col(&[Some(SQUARE), Some(SQUARE)]);
    assert_eq!(pred(&a, &b, Predicate::Intersects), vec![None, Some(true)]);
}

#[test]
fn broadcast_scalar_matches_pairwise() {
    let points = col(&[
        Some("POINT (2 2)"),
        Some("POINT (0 2)"),
        Some("POINT (9 9)"),
        None,
    ]);
    let scalar = one(SQUARE);
    let full = scalar.take(&[0, 0, 0, 0]).unwrap();
    for p in [
        Predicate::Intersects,
        Predicate::Contains,
        Predicate::Within,
        Predicate::Touches,
        Predicate::Crosses,
        Predicate::Overlaps,
        Predicate::Disjoint,
        Predicate::Covers,
        Predicate::CoveredBy,
        Predicate::Equals,
    ] {
        assert_eq!(
            points.predicate(&scalar, p).unwrap(),
            points.predicate(&full, p).unwrap(),
            "{p:?}"
        );
    }
    assert_eq!(
        points.distance(&scalar).unwrap(),
        points.distance(&full).unwrap()
    );
    assert_eq!(
        points.relate(&scalar).unwrap(),
        points.relate(&full).unwrap()
    );
}

#[test]
fn length_mismatch_is_an_error() {
    let a = col(&[Some("POINT (1 1)"), Some("POINT (2 2)")]);
    let b = col(&[
        Some("POINT (1 1)"),
        Some("POINT (2 2)"),
        Some("POINT (3 3)"),
    ]);
    let err = a.predicate(&b, Predicate::Intersects).unwrap_err();
    assert!(err.contains("length"), "unexpected message: {err}");
    assert!(a.distance(&b).is_err());
    assert!(a.relate(&b).is_err());
    assert!(a.dwithin(&b, 1.0).is_err());
}

#[test]
fn crs_mismatch_is_an_error_but_unknown_is_compatible() {
    let a = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:4326")).unwrap();
    let b = a.set_crs(Some("EPSG:3857"));
    let err = a.predicate(&b, Predicate::Intersects).unwrap_err();
    assert!(err.contains("CRS"), "unexpected message: {err}");
    assert!(a.distance(&b).is_err());
    let unknown = a.set_crs(None);
    assert!(a.predicate(&unknown, Predicate::Intersects).is_ok());
    assert!(a.predicate(&a.clone(), Predicate::Intersects).is_ok());
}

#[test]
fn distance_is_planar_euclidean() {
    let a = col(&[
        Some("POINT (0 0)"),
        Some("POINT (2 2)"),
        Some(SQUARE),
        Some("LINESTRING (0 0, 4 0)"),
    ]);
    let b = col(&[
        Some("POINT (3 4)"),
        Some(SQUARE),
        Some("POLYGON ((7 0, 9 0, 9 4, 7 4, 7 0))"),
        Some("POINT (2 3)"),
    ]);
    assert_eq!(
        a.distance(&b).unwrap(),
        vec![Some(5.0), Some(0.0), Some(3.0), Some(3.0)]
    );
}

#[test]
fn dwithin_is_inclusive() {
    let a = col(&[Some("POINT (0 0)"), Some("POINT (0 0)"), None]);
    let b = col(&[
        Some("POINT (3 4)"),
        Some("POINT (3 4)"),
        Some("POINT (0 0)"),
    ]);
    assert_eq!(
        a.dwithin(&b, 5.0).unwrap(),
        vec![Some(true), Some(true), None]
    );
    assert_eq!(
        a.dwithin(&b, 4.99).unwrap(),
        vec![Some(false), Some(false), None]
    );
}

#[test]
fn relate_returns_de9im_strings() {
    let a = col(&[
        Some(SQUARE),
        Some(SQUARE),
        Some(SQUARE),
        Some("POINT (2 2)"),
    ]);
    let b = col(&[
        Some("POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))"),
        Some("POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))"),
        Some("POLYGON ((10 10, 11 10, 11 11, 10 11, 10 10))"),
        Some(SQUARE),
    ]);
    assert_eq!(
        a.relate(&b).unwrap(),
        vec![
            Some("FF2F11212".to_string()),
            Some("212101212".to_string()),
            Some("FF2FF1212".to_string()),
            Some("0FFFFF212".to_string()),
        ]
    );
}

#[test]
fn relate_broadcast_keeps_argument_order() {
    // scalar prepared on the right: the matrix must still read (row geometry, scalar).
    let points = col(&[Some("POINT (2 2)")]);
    let sq = one(SQUARE);
    assert_eq!(
        points.relate(&sq).unwrap(),
        vec![Some("0FFFFF212".to_string())]
    );
    let many = col(&[Some("POINT (2 2)"), Some("POINT (0 0)")]);
    assert_eq!(
        many.relate(&sq).unwrap(),
        vec![Some("0FFFFF212".to_string()), Some("F0FFFF212".to_string())]
    );
    let sq_twice = sq.take(&[0, 0]).unwrap();
    assert_eq!(
        sq_twice.relate(&many).unwrap(),
        vec![Some("0F2FF1FF2".to_string()), Some("FF20F1FF2".to_string())]
    );
}

#[test]
fn predicate_names_parse() {
    assert_eq!(
        Predicate::from_name("covered_by"),
        Some(Predicate::CoveredBy)
    );
    assert_eq!(Predicate::from_name("geom_equals"), Some(Predicate::Equals));
    assert_eq!(Predicate::from_name("nope"), None);
}

#[test]
fn distance_to_an_empty_geometry_is_null() {
    let left = col(&[
        Some("LINESTRING EMPTY"),
        Some("POLYGON EMPTY"),
        Some("GEOMETRYCOLLECTION EMPTY"),
        Some("POINT (0 0)"),
    ]);
    let right = one("POINT (1 1)");
    let d = left.distance(&right).unwrap();
    assert_eq!(&d[..3], &[None, None, None]);
    assert!(d[3].is_some());
    assert_eq!(left.dwithin(&right, 2.0).unwrap()[..3], [None, None, None]);
}
