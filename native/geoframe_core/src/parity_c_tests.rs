//! Reference cases for batch C: linear referencing and linestring operations. Expected
//! answers are the Shapely/GEOS ones.

use crate::test_support::wkt;
use crate::GeoColumn;

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(text: &str) -> GeoColumn {
    col(&[Some(text)])
}

/// Equal within 1e-9, rows that are null or empty on both sides counting as equal.
fn same(a: &GeoColumn, b: &GeoColumn) -> bool {
    let (wa, wb) = (wkt(a), wkt(b));
    a.equals_exact(b, 1e-9)
        .unwrap()
        .into_iter()
        .enumerate()
        .all(|(i, v)| v.unwrap_or(wa[i] == wb[i]))
}

fn assert_wkt(actual: &GeoColumn, expected: &[Option<&str>]) {
    let want = col(expected);
    assert!(
        same(actual, &want),
        "got {:?}, want {:?}",
        wkt(actual),
        wkt(&want)
    );
}

fn close(a: Option<f64>, b: f64) {
    let a = a.expect("a value");
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

const LINE: &str = "LINESTRING (0 0, 10 0, 10 10)";

// -- interpolate ----------------------------------------------------------------------

#[test]
fn interpolate_distance_and_normalized() {
    let s = col(&[
        Some(LINE),
        Some(LINE),
        Some(LINE),
        None,
        Some("POINT (1 1)"),
    ]);
    let d = [Some(5.0), Some(15.0), Some(-5.0), Some(1.0), Some(1.0)];
    let out = s.interpolate(&d, false).unwrap();
    assert_wkt(
        &out,
        &[
            Some("POINT (5 0)"),
            Some("POINT (10 5)"),
            Some("POINT (10 5)"),
            None,
            None,
        ],
    );
    let n = [Some(0.0), Some(0.5), Some(1.0), Some(0.5), Some(0.5)];
    let out = s.interpolate(&n, true).unwrap();
    assert_wkt(
        &out,
        &[
            Some("POINT (0 0)"),
            Some("POINT (10 0)"),
            Some("POINT (10 10)"),
            None,
            None,
        ],
    );
}

#[test]
fn interpolate_clamps_and_nulls_a_missing_distance() {
    let s = col(&[Some(LINE), Some(LINE), Some(LINE)]);
    let out = s
        .interpolate(&[Some(99.0), Some(-99.0), None], false)
        .unwrap();
    assert_wkt(&out, &[Some("POINT (10 10)"), Some("POINT (0 0)"), None]);
}

#[test]
fn interpolate_multilinestring_treats_parts_as_concatenated() {
    let s = one("MULTILINESTRING ((0 0, 10 0), (100 100, 100 120))");
    let out = s.interpolate(&[Some(15.0)], false).unwrap();
    assert_wkt(&out, &[Some("POINT (100 105)")]);
    let out = s.interpolate(&[Some(-5.0)], false).unwrap();
    assert_wkt(&out, &[Some("POINT (100 115)")]);
}

#[test]
fn interpolate_length_mismatch_is_an_error() {
    let s = col(&[Some(LINE), Some(LINE)]);
    assert!(s.interpolate(&[Some(1.0)], false).is_err());
}

// -- project --------------------------------------------------------------------------

#[test]
fn project_a_point_off_the_line() {
    let s = one(LINE);
    let p = one("POINT (4 3)");
    close(s.project(&p, false).unwrap()[0], 4.0);
    close(s.project(&p, true).unwrap()[0], 0.2);
    let q = one("POINT (13 7)");
    close(s.project(&q, false).unwrap()[0], 17.0);
}

#[test]
fn project_broadcasts_and_nulls() {
    let s = col(&[Some(LINE), Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"), None]);
    let p = one("POINT (10 5)");
    let out = s.project(&p, false).unwrap();
    close(out[0], 15.0);
    assert_eq!(out[1], None);
    assert_eq!(out[2], None);
    let not_a_point = one("LINESTRING (0 0, 1 1)");
    assert_eq!(s.project(&not_a_point, false).unwrap()[0], None);
}

#[test]
fn project_multilinestring_adds_earlier_parts() {
    let s = one("MULTILINESTRING ((0 0, 10 0), (100 100, 100 120))");
    close(s.project(&one("POINT (101 110)"), false).unwrap()[0], 20.0);
}

#[test]
fn project_and_interpolate_round_trip() {
    let s = one("LINESTRING (0 0, 3 4, 3 10, 8 12)");
    let total = s.length()[0].unwrap();
    for k in 0..=10 {
        let d = total * f64::from(k) / 10.0;
        let p = s.interpolate(&[Some(d)], false).unwrap();
        close(s.project(&p, false).unwrap()[0], d);
    }
}

// -- segmentize -----------------------------------------------------------------------

#[test]
fn segmentize_adds_vertices() {
    let out = one("LINESTRING (0 0, 10 0)").segmentize(3.0).unwrap();
    assert_wkt(&out, &[Some("LINESTRING (0 0, 2.5 0, 5 0, 7.5 0, 10 0)")]);
    assert_eq!(out.count_coordinates(), vec![Some(5)]);
}

#[test]
fn segmentize_polygon_and_points() {
    let s = col(&[
        Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"),
        Some("POINT (1 1)"),
        None,
    ]);
    let out = s.segmentize(2.0).unwrap();
    assert_wkt(
        &out,
        &[
            Some("POLYGON ((0 0, 2 0, 4 0, 4 2, 4 4, 2 4, 0 4, 0 2, 0 0))"),
            Some("POINT (1 1)"),
            None,
        ],
    );
}

#[test]
fn segmentize_rejects_a_bad_length() {
    let s = one("LINESTRING (0 0, 10 0)");
    assert!(s.segmentize(0.0).is_err());
    assert!(s.segmentize(-1.0).is_err());
    assert!(s.segmentize(f64::NAN).is_err());
}

// -- remove_repeated_points -----------------------------------------------------------

#[test]
fn remove_repeated_points_exact() {
    let s = col(&[
        Some("LINESTRING (0 0, 0 0, 1 1, 1 1, 2 2)"),
        Some("MULTIPOINT ((0 0), (0 0), (1 1))"),
        Some("POLYGON ((0 0, 1 0, 1 0, 1 1, 0 0))"),
        Some("POINT (1 1)"),
        None,
    ]);
    let out = s.remove_repeated_points(0.0).unwrap();
    assert_wkt(
        &out,
        &[
            Some("LINESTRING (0 0, 1 1, 2 2)"),
            Some("MULTIPOINT ((0 0), (1 1))"),
            Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
            Some("POINT (1 1)"),
            None,
        ],
    );
}

#[test]
fn remove_repeated_points_with_tolerance() {
    let s = col(&[
        Some("LINESTRING (0 0, 0.1 0, 0.2 0, 5 0, 5.05 0)"),
        Some("POLYGON ((0 0, 5 0, 5.1 0, 5 5, 0 5, 0 0.05, 0 0))"),
    ]);
    let out = s.remove_repeated_points(0.5).unwrap();
    assert_wkt(
        &out,
        &[
            Some("LINESTRING (0 0, 5.05 0)"),
            Some("POLYGON ((0 0, 5 0, 5 5, 0 5, 0 0))"),
        ],
    );
}

#[test]
fn remove_repeated_points_keeps_a_collapsed_line_at_two_points() {
    // GEOS never reduces a line below two points: a fully collapsed line keeps its point twice.
    for tolerance in [0.0, 0.5] {
        let out = one("LINESTRING (1 1, 1 1)")
            .remove_repeated_points(tolerance)
            .unwrap();
        assert_wkt(&out, &[Some("LINESTRING (1 1, 1 1)")]);
    }
}

#[test]
fn remove_repeated_points_rejects_a_bad_tolerance() {
    assert!(one("POINT (0 0)").remove_repeated_points(-1.0).is_err());
    assert!(one("POINT (0 0)").remove_repeated_points(f64::NAN).is_err());
}

// -- reverse / orient / is_ccw --------------------------------------------------------

#[test]
fn reverse_every_type() {
    let s = col(&[
        Some("LINESTRING (0 0, 1 1, 2 0)"),
        Some("POLYGON ((0 0, 4 0, 4 4, 0 0), (1 1, 2 1, 2 2, 1 1))"),
        Some("MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))"),
        Some("POINT (1 1)"),
        None,
    ]);
    let out = s.reverse().unwrap();
    assert_wkt(
        &out,
        &[
            Some("LINESTRING (2 0, 1 1, 0 0)"),
            Some("POLYGON ((0 0, 4 4, 4 0, 0 0), (1 1, 2 2, 2 1, 1 1))"),
            Some("MULTILINESTRING ((1 1, 0 0), (3 3, 2 2))"),
            Some("POINT (1 1)"),
            None,
        ],
    );
}

#[test]
fn reverse_twice_is_the_identity() {
    let s = col(&[
        Some("LINESTRING (0 0, 1 5, 2 0)"),
        Some("MULTIPOLYGON (((0 0, 4 0, 4 4, 0 0)), ((9 9, 9 8, 8 8, 9 9)))"),
        Some("GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 5 5))"),
    ]);
    let back = s.reverse().unwrap().reverse().unwrap();
    assert_eq!(
        back.equals_identical(&s).unwrap(),
        vec![Some(true), Some(true), Some(true)]
    );
}

#[test]
fn orient_polygons_both_ways() {
    // Clockwise shell, counter-clockwise hole.
    let cw = "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0), (1 1, 2 1, 2 2, 1 1))"; // hole is ccw
    let s = col(&[Some(cw), Some("LINESTRING (0 0, 1 1)"), None]);
    let ccw = s.orient_polygons(false).unwrap();
    assert_wkt(
        &ccw,
        &[
            Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 2, 2 1, 1 1))"),
            Some("LINESTRING (0 0, 1 1)"),
            None,
        ],
    );
    let back = ccw.orient_polygons(true).unwrap();
    assert_wkt(&back, &[Some(cw), Some("LINESTRING (0 0, 1 1)"), None]);
}

#[test]
fn is_ccw_follows_shapely() {
    let s = col(&[
        Some("LINESTRING (0 0, 1 0, 1 1, 0 0)"),
        Some("LINESTRING (0 0, 1 1, 1 0, 0 0)"),
        Some("LINESTRING (0 0, 1 1, 0 1)"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("POINT (0 0)"),
        None,
    ]);
    assert_eq!(
        s.is_ccw(),
        vec![
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
            None
        ]
    );
}

// -- distances ------------------------------------------------------------------------

#[test]
fn hausdorff_matches_the_shapely_docs() {
    let a = one("LINESTRING (130 0, 0 0, 0 150)");
    let b = one("LINESTRING (10 10, 10 150, 130 10)");
    close(
        a.hausdorff_distance(&b, None).unwrap()[0],
        14.142135623730951,
    );
    close(a.hausdorff_distance(&b, Some(0.5)).unwrap()[0], 70.0);
}

#[test]
fn hausdorff_between_offset_lines_and_with_vertices() {
    let a = one("LINESTRING (0 0, 10 0)");
    close(
        a.hausdorff_distance(&one("LINESTRING (0 1, 10 1)"), None)
            .unwrap()[0],
        1.0,
    );
    // Vertex-to-geometry (GEOS), not vertex-to-vertex: the extra vertex adds nothing.
    close(
        a.hausdorff_distance(&one("LINESTRING (0 0, 5 0, 10 0)"), None)
            .unwrap()[0],
        0.0,
    );
}

#[test]
fn hausdorff_self_is_zero_and_rules() {
    let a = col(&[Some("POLYGON ((0 0, 4 0, 4 4, 0 0))"), None]);
    let out = a.hausdorff_distance(&a, None).unwrap();
    close(out[0], 0.0);
    assert_eq!(out[1], None);
    assert!(a.hausdorff_distance(&a, Some(0.0)).is_err());
    assert!(a.hausdorff_distance(&a, Some(1.5)).is_err());
}

#[test]
fn frechet_of_a_reversed_line() {
    let a = one("LINESTRING (0 0, 10 0)");
    let b = one("LINESTRING (10 0, 0 0)");
    close(a.frechet_distance(&b, None).unwrap()[0], 10.0);
    close(a.frechet_distance(&a, None).unwrap()[0], 0.0);
}

#[test]
fn frechet_densified_and_non_lines() {
    let a = one("LINESTRING (0 0, 10 0)");
    let b = one("LINESTRING (0 1, 5 1, 10 1)");
    close(a.frechet_distance(&b, None).unwrap()[0], 5.0_f64.hypot(1.0));
    // Densified, a has vertices 0/5/10 and b 0/2.5/5/7.5/10: a(5) must also cover b(2.5).
    close(
        a.frechet_distance(&b, Some(0.5)).unwrap()[0],
        2.5_f64.hypot(1.0),
    );
    let p = one("POINT (0 0)");
    assert_eq!(a.frechet_distance(&p, None).unwrap()[0], None);
}

// -- shortest_line --------------------------------------------------------------------

#[test]
fn shortest_line_between_parallel_lines_and_points() {
    let a = one("LINESTRING (0 0, 10 0)");
    let b = one("LINESTRING (3 5, 20 5)");
    let line = a.shortest_line(&b).unwrap();
    let d = a.distance(&b).unwrap()[0].unwrap();
    close(line.length()[0], d);
    let p = one("POINT (4 3)");
    assert_wkt(
        &a.shortest_line(&p).unwrap(),
        &[Some("LINESTRING (4 0, 4 3)")],
    );
}

#[test]
fn shortest_line_when_geometries_intersect_is_degenerate() {
    let a = one("LINESTRING (0 0, 10 10)");
    let b = one("LINESTRING (0 10, 10 0)");
    let line = a.shortest_line(&b).unwrap();
    assert_wkt(&line, &[Some("LINESTRING (5 5, 5 5)")]);
    let poly = one("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))");
    let inside = one("POINT (1 1)");
    close(poly.shortest_line(&inside).unwrap().length()[0], 0.0);
}

#[test]
fn shortest_line_nulls_and_polygons() {
    let a = col(&[Some("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"), None]);
    let b = one("POLYGON ((3 0, 4 0, 4 1, 3 1, 3 0))");
    let out = a.shortest_line(&b).unwrap();
    close(out.length()[0], 2.0);
    assert_eq!(out.length()[1], None);
}

// -- line_merge -----------------------------------------------------------------------

#[test]
fn line_merge_three_connected_segments() {
    let s = one("MULTILINESTRING ((0 0, 1 1), (1 1, 2 2), (2 2, 3 3))");
    let out = s.line_merge(false).unwrap();
    assert_wkt(&out, &[Some("LINESTRING (0 0, 1 1, 2 2, 3 3)")]);
}

#[test]
fn line_merge_flips_parts_when_undirected() {
    let s = one("MULTILINESTRING ((0 0, 1 1), (2 2, 1 1), (2 2, 3 3))");
    let out = s.line_merge(false).unwrap();
    assert_wkt(&out, &[Some("LINESTRING (0 0, 1 1, 2 2, 3 3)")]);
}

#[test]
fn line_merge_stops_at_a_t_junction() {
    let s = one("MULTILINESTRING ((0 0, 5 0), (5 0, 10 0), (5 0, 5 5))");
    let out = s.line_merge(false).unwrap();
    assert_wkt(
        &out,
        &[Some(
            "MULTILINESTRING ((0 0, 5 0), (5 0, 10 0), (5 0, 5 5))",
        )],
    );
}

#[test]
fn line_merge_directed_only_joins_head_to_tail() {
    let s = one("MULTILINESTRING ((0 0, 1 1), (2 2, 1 1))");
    let undirected = s.line_merge(false).unwrap();
    assert_wkt(&undirected, &[Some("LINESTRING (0 0, 1 1, 2 2)")]);
    let directed = s.line_merge(true).unwrap();
    assert_wkt(
        &directed,
        &[Some("MULTILINESTRING ((0 0, 1 1), (2 2, 1 1))")],
    );
    let chain = one("MULTILINESTRING ((0 0, 1 1), (1 1, 2 2))");
    assert_wkt(
        &chain.line_merge(true).unwrap(),
        &[Some("LINESTRING (0 0, 1 1, 2 2)")],
    );
}

#[test]
fn line_merge_disjoint_closed_ring_and_passthrough() {
    let s = col(&[
        Some("MULTILINESTRING ((0 0, 1 0), (5 5, 6 6))"),
        Some("MULTILINESTRING ((0 0, 1 0), (1 0, 1 1), (1 1, 0 0))"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("POINT (1 1)"),
        None,
    ]);
    let out = s.line_merge(false).unwrap();
    assert_wkt(
        &out,
        &[
            Some("MULTILINESTRING ((0 0, 1 0), (5 5, 6 6))"),
            Some("LINESTRING (0 0, 1 0, 1 1, 0 0)"),
            Some("LINESTRING (0 0, 1 1)"),
            Some("GEOMETRYCOLLECTION EMPTY"),
            None,
        ],
    );
}

#[test]
fn every_operation_keeps_the_crs() {
    let s = GeoColumn::from_wkt(&[Some(LINE)], Some("EPSG:3857")).unwrap();
    assert_eq!(s.reverse().unwrap().crs().as_deref(), Some("EPSG:3857"));
    assert_eq!(
        s.segmentize(1.0).unwrap().crs().as_deref(),
        Some("EPSG:3857")
    );
    assert_eq!(
        s.interpolate(&[Some(1.0)], false).unwrap().crs().as_deref(),
        Some("EPSG:3857")
    );
    assert_eq!(
        s.line_merge(false).unwrap().crs().as_deref(),
        Some("EPSG:3857")
    );
    let other = GeoColumn::from_wkt(&[Some(LINE)], Some("EPSG:4326")).unwrap();
    assert!(s.shortest_line(&other).is_err());
    assert!(s.hausdorff_distance(&other, None).is_err());
    assert!(s.project(&other, false).is_err());
}
