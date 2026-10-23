//! Reference cases for batch H: exact equality, relate patterns, bbox queries and concat.
//! Expected answers are the Shapely/GeoPandas ones.

use crate::{GeoColumn, Predicate, QueryPredicate};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

const SQUARE: &str = "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))";
const SQUARE_REVERSED: &str = "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))";

#[test]
fn equals_exact_requires_same_vertex_order() {
    let a = one(SQUARE);
    let b = one(SQUARE_REVERSED);
    assert_eq!(a.equals_exact(&b, 0.0).unwrap(), vec![Some(false)]);
    // ...while topological equality holds.
    assert_eq!(
        a.predicate(&b, Predicate::Equals).unwrap(),
        vec![Some(true)]
    );
    assert_eq!(a.equals_exact(&a, 0.0).unwrap(), vec![Some(true)]);
}

#[test]
fn equals_exact_honours_the_tolerance() {
    let a = one("LINESTRING (0 0, 1 1)");
    let b = one("LINESTRING (0 0, 1.0005 1)");
    assert_eq!(a.equals_exact(&b, 0.001).unwrap(), vec![Some(true)]);
    assert_eq!(a.equals_exact(&b, 0.0001).unwrap(), vec![Some(false)]);
}

#[test]
fn equals_exact_compares_types_and_structure() {
    let a = col(&[
        Some("POINT (1 1)"),
        Some("MULTIPOINT ((1 1), (2 2))"),
        Some("MULTIPOINT ((1 1), (2 2))"),
    ]);
    let b = col(&[
        Some("MULTIPOINT ((1 1))"),
        Some("MULTIPOINT ((2 2), (1 1))"),
        Some("MULTIPOINT ((1 1), (2 2))"),
    ]);
    assert_eq!(
        a.equals_exact(&b, 0.0).unwrap(),
        vec![Some(false), Some(false), Some(true)]
    );
}

#[test]
fn equals_exact_checks_polygon_holes() {
    let with_hole = one("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 2, 1 1))");
    let solid = one(SQUARE);
    assert_eq!(
        with_hole.equals_exact(&solid, 0.0).unwrap(),
        vec![Some(false)]
    );
    assert_eq!(
        with_hole.equals_exact(&with_hole, 0.0).unwrap(),
        vec![Some(true)]
    );
}

#[test]
fn equals_exact_broadcasts_and_propagates_nulls() {
    let a = col(&[Some("POINT (1 1)"), None, Some("POINT (2 2)")]);
    let b = one("POINT (1 1)");
    assert_eq!(
        a.equals_exact(&b, 0.0).unwrap(),
        vec![Some(true), None, Some(false)]
    );
    assert!(a
        .equals_exact(&col(&[Some("POINT (1 1)"), None]), 0.0)
        .is_err());
}

#[test]
fn equals_exact_rejects_a_negative_or_nan_tolerance() {
    let a = one("POINT (1 1)");
    assert!(a.equals_exact(&a, -1.0).is_err());
    assert!(a.equals_exact(&a, f64::NAN).is_err());
}

#[test]
fn equals_exact_rejects_crs_mismatch() {
    let a = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:4326")).unwrap();
    let b = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:3857")).unwrap();
    assert!(a.equals_exact(&b, 0.0).is_err());
    assert!(a.equals_identical(&b).is_err());
}

#[test]
fn equals_identical_is_exact_equality_without_tolerance() {
    let a = one("LINESTRING (0 0, 1 1)");
    let b = one("LINESTRING (0 0, 1.0000000001 1)");
    assert_eq!(a.equals_identical(&b).unwrap(), vec![Some(false)]);
    assert_eq!(a.equals_identical(&a).unwrap(), vec![Some(true)]);
    assert_eq!(
        one(SQUARE).equals_identical(&one(SQUARE_REVERSED)).unwrap(),
        vec![Some(false)]
    );
}

#[test]
fn equals_identical_treats_nan_as_equal_but_equals_exact_does_not() {
    let nan = GeoColumn::from_xy(&[Some(f64::NAN)], &[Some(1.0)], None).unwrap();
    assert_eq!(nan.equals_identical(&nan).unwrap(), vec![Some(true)]);
    assert_eq!(nan.equals_exact(&nan, 0.0).unwrap(), vec![Some(false)]);
}

#[test]
fn contains_properly_excludes_the_boundary() {
    let sq = one(SQUARE).take(&[0, 0, 0, 0]).unwrap();
    let points = col(&[
        Some("POINT (2 2)"),
        Some("POINT (0 2)"),
        Some("POINT (9 9)"),
        Some("POINT (0 0)"),
    ]);
    assert_eq!(
        sq.predicate(&points, Predicate::ContainsProperly).unwrap(),
        vec![Some(true), Some(false), Some(false), Some(false)]
    );
    // `contains` also excludes the boundary point, but a line along the edge differs:
    let edge = one("LINESTRING (0 0, 4 0)");
    assert_eq!(
        one(SQUARE)
            .predicate(&edge, Predicate::ContainsProperly)
            .unwrap(),
        vec![Some(false)]
    );
    // a line inside touching the boundary at one end is contained, not properly contained
    let touching = one("LINESTRING (0 2, 2 2)");
    assert_eq!(
        one(SQUARE)
            .predicate(&touching, Predicate::Contains)
            .unwrap(),
        vec![Some(true)]
    );
    assert_eq!(
        one(SQUARE)
            .predicate(&touching, Predicate::ContainsProperly)
            .unwrap(),
        vec![Some(false)]
    );
}

#[test]
fn contains_properly_broadcast_scalar_on_the_right_matches_pairwise() {
    let rows = col(&[
        Some(SQUARE),
        Some("POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))"),
        Some("POLYGON ((10 10, 12 10, 12 12, 10 12, 10 10))"),
    ]);
    let inner = one("POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))");
    // row 0 contains it properly; row 1 shares the corner region (boundary contact) so no.
    assert_eq!(
        rows.predicate(&inner, Predicate::ContainsProperly).unwrap(),
        vec![Some(true), Some(false), Some(false)]
    );
    let pairwise = rows
        .predicate(
            &inner.take(&[0, 0, 0]).unwrap(),
            Predicate::ContainsProperly,
        )
        .unwrap();
    assert_eq!(
        rows.predicate(&inner, Predicate::ContainsProperly).unwrap(),
        pairwise
    );
}

#[test]
fn contains_properly_is_available_to_the_spatial_index() {
    let polys = one(SQUARE);
    let points = col(&[
        Some("POINT (2 2)"),
        Some("POINT (0 2)"),
        Some("POINT (9 9)"),
    ]);
    let q = QueryPredicate::from_name("contains_properly", None).unwrap();
    let (l, r) = polys.query_pairs(&points, q).unwrap();
    assert_eq!((l, r), (vec![0], vec![0]));
}

#[test]
fn relate_pattern_on_touching_squares() {
    let a = one(SQUARE);
    let b = one("POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))");
    // edge contact: interiors disjoint, boundaries meet in a line.
    assert_eq!(a.relate(&b).unwrap(), vec![Some("FF2F11212".to_string())]);
    assert_eq!(a.relate_pattern(&b, "FF*F1****").unwrap(), vec![Some(true)]);
    assert_eq!(
        a.relate_pattern(&b, "T********").unwrap(),
        vec![Some(false)]
    );
    assert_eq!(a.relate_pattern(&b, "FF2F11212").unwrap(), vec![Some(true)]);
    assert_eq!(a.relate_pattern(&b, "ff*f1****").unwrap(), vec![Some(true)]);
}

#[test]
fn relate_pattern_t_matches_any_dimension() {
    let a = one(SQUARE);
    let b = one("POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))");
    // overlapping polygons: interiors meet in 2 dimensions
    assert_eq!(a.relate_pattern(&b, "T********").unwrap(), vec![Some(true)]);
    assert_eq!(a.relate_pattern(&b, "2********").unwrap(), vec![Some(true)]);
    assert_eq!(
        a.relate_pattern(&b, "1********").unwrap(),
        vec![Some(false)]
    );
}

#[test]
fn relate_pattern_validates_the_pattern() {
    let a = one(SQUARE);
    assert!(a.relate_pattern(&a, "T***").is_err());
    assert!(a.relate_pattern(&a, "T********X").is_err());
    assert!(a.relate_pattern(&a, "X********").is_err());
    assert!(a.relate_pattern(&a, "").is_err());
}

#[test]
fn relate_pattern_of_its_own_matrix_is_always_true() {
    let a = col(&[
        Some(SQUARE),
        Some("LINESTRING (0 0, 4 4)"),
        Some("POINT (2 2)"),
        None,
    ]);
    let b = col(&[
        Some("POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))"),
        Some("LINESTRING (0 4, 4 0)"),
        Some(SQUARE),
        Some("POINT (0 0)"),
    ]);
    let codes = a.relate(&b).unwrap();
    for (i, code) in codes.iter().enumerate() {
        let pair_a = a.take(&[i]).unwrap();
        let pair_b = b.take(&[i]).unwrap();
        match code {
            Some(code) => assert_eq!(
                pair_a.relate_pattern(&pair_b, code).unwrap(),
                vec![Some(true)]
            ),
            None => assert_eq!(
                pair_a.relate_pattern(&pair_b, "*********").unwrap(),
                vec![None]
            ),
        }
    }
}

#[test]
fn bbox_intersection_returns_rows_whose_envelope_meets_the_box() {
    let c = col(&[
        Some("POINT (1 1)"),
        Some("LINESTRING (5 5, 9 9)"),
        None,
        Some("POLYGON ((10 10, 12 10, 12 12, 10 12, 10 10))"),
        Some("POINT (2 0)"),
    ]);
    assert_eq!(
        c.bbox_intersection([0.0, 0.0, 2.0, 2.0]).unwrap(),
        vec![0, 4]
    );
    // the diagonal line's envelope meets the box although the line does not pass through it
    assert_eq!(c.bbox_intersection([5.0, 8.0, 6.0, 9.0]).unwrap(), vec![1]);
    assert_eq!(
        c.bbox_intersection([20.0, 20.0, 30.0, 30.0]).unwrap(),
        Vec::<u32>::new()
    );
    assert!(c.bbox_intersection([2.0, 0.0, 0.0, 2.0]).is_err());
}

#[test]
fn concat_appends_rows_in_order_and_keeps_nulls() {
    let a = col(&[Some("POINT (1 1)"), None]);
    let b = col(&[Some(SQUARE)]);
    let out = GeoColumn::concat(&[&a, &b]).unwrap();
    assert_eq!(out.len(), 3);
    assert_eq!(
        out.to_wkt().unwrap(),
        vec![
            Some("POINT(1 1)".to_string()),
            None,
            Some("POLYGON((0 0,4 0,4 4,0 4,0 0))".to_string())
        ]
    );
}

#[test]
fn concat_requires_matching_crs_and_at_least_one_column() {
    let a = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:4326")).unwrap();
    let b = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:3857")).unwrap();
    let c = GeoColumn::from_wkt(&[Some("POINT (1 1)")], None).unwrap();
    assert!(GeoColumn::concat(&[&a, &b]).is_err());
    assert!(GeoColumn::concat(&[&a, &c]).is_err());
    assert!(GeoColumn::concat(&[]).is_err());
    let ok = GeoColumn::concat(&[&a, &a]).unwrap();
    assert_eq!(ok.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(ok.len(), 2);
}
