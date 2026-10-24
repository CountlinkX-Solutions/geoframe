//! Reference cases for unary constructive operations and accessors. Expected answers are the
//! Shapely/GeoPandas ones.

use crate::{BufferOptions, GeoColumn, Origin, Predicate, UnaryOp};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(wkt: &str) -> GeoColumn {
    col(&[Some(wkt)])
}

/// Asserts that every row is topologically equal to the expected WKT.
fn assert_geom_eq(actual: &GeoColumn, expected: &[Option<&str>]) {
    assert_eq!(actual.len(), expected.len());
    let expected = col(expected);
    let eq = actual.predicate(&expected, Predicate::Equals).unwrap();
    for (i, (row, want)) in eq.iter().zip(expected.to_wkt().unwrap()).enumerate() {
        match want {
            None => assert!(
                actual.to_wkt().unwrap()[i].is_none(),
                "row {i} should be null"
            ),
            Some(_) => assert_eq!(
                *row,
                Some(true),
                "row {i}: {:?}",
                actual.to_wkt().unwrap()[i]
            ),
        }
    }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

const L_SHAPE: &str = "POLYGON ((0 0, 4 0, 4 1, 1 1, 1 4, 0 4, 0 0))";
const BOWTIE: &str = "POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))";

#[test]
fn buffer_point_area_approximates_circle() {
    let out = one("POINT (1 1)")
        .buffer(2.0, &BufferOptions::default())
        .unwrap();
    let area = out.area()[0].unwrap();
    let circle = std::f64::consts::PI * 4.0;
    assert!(close(area, circle, circle * 0.01), "area {area}");
    assert_eq!(out.geom_type(), vec![Some("Polygon".to_string())]);
}

#[test]
fn buffer_keeps_crs_and_nulls() {
    let src = GeoColumn::from_wkt(&[Some("POINT (0 0)"), None], Some("EPSG:3857")).unwrap();
    let out = src.buffer(1.0, &BufferOptions::default()).unwrap();
    assert_eq!(out.crs().as_deref(), Some("EPSG:3857"));
    assert!(out.area()[0].is_some());
    assert_eq!(out.area()[1], None);
}

#[test]
fn negative_buffer_shrinks_polygon() {
    let out = one("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))")
        .buffer(-1.0, &BufferOptions::default())
        .unwrap();
    // Offset curves carry floating-point noise, so compare area and bounds with a tolerance.
    assert!(close(out.area()[0].unwrap(), 64.0, 1e-9));
    let b = out.bounds()[0].unwrap();
    assert!(b
        .iter()
        .zip([1.0, 1.0, 9.0, 9.0])
        .all(|(g, w)| close(*g, w, 1e-9)));
}

#[test]
fn buffer_flat_cap_and_mitre_join_make_a_rectangle() {
    let options = BufferOptions {
        cap: "flat".to_string(),
        join: "mitre".to_string(),
        ..BufferOptions::default()
    };
    let out = one("LINESTRING (0 0, 4 0)").buffer(1.0, &options).unwrap();
    assert_geom_eq(&out, &[Some("POLYGON ((0 -1, 4 -1, 4 1, 0 1, 0 -1))")]);
}

#[test]
fn buffer_square_cap_extends_the_line() {
    let options = BufferOptions {
        cap: "square".to_string(),
        ..BufferOptions::default()
    };
    let out = one("LINESTRING (0 0, 4 0)").buffer(1.0, &options).unwrap();
    assert_eq!(out.bounds()[0], Some([-1.0, -1.0, 5.0, 1.0]));
}

#[test]
fn buffer_rejects_unknown_styles() {
    let bad_cap = BufferOptions {
        cap: "pointy".to_string(),
        ..BufferOptions::default()
    };
    assert!(one("POINT (0 0)").buffer(1.0, &bad_cap).is_err());
    let bad_join = BufferOptions {
        join: "pointy".to_string(),
        ..BufferOptions::default()
    };
    assert!(one("POINT (0 0)").buffer(1.0, &bad_join).is_err());
}

#[test]
fn simplify_drops_collinear_vertices_in_both_modes() {
    let line = one("LINESTRING (0 0, 1 0.001, 2 0, 3 0.001, 4 0)");
    let rdp = line.simplify(0.1, false).unwrap();
    assert_geom_eq(&rdp, &[Some("LINESTRING (0 0, 4 0)")]);
    assert_eq!(rdp.count_coordinates(), vec![Some(2)]);
    let vw = line.simplify(0.1, true).unwrap();
    assert_eq!(vw.count_coordinates(), vec![Some(2)]);
}

/// `simplify(preserve_topology=false)` is GEOS's Douglas-Peucker: it does not keep four vertices
/// per ring, so a ring that collapses is removed (issue #89). Expected answers are
/// `shapely.simplify(g, tol, preserve_topology=False)`.
#[test]
fn rdp_removes_collapsed_rings_like_geos() {
    let cases: [(&str, f64, &str); 14] = [
        ("POLYGON ((0 0, 0.1 0, 0.05 0.1, 0 0))", 1.0, "POLYGON EMPTY"),
        (
            "POLYGON ((0 0, 0.1 0, 0.1 0.1, 0 0.1, 0 0))",
            1.0,
            "POLYGON EMPTY",
        ),
        (
            "POLYGON ((0 0, 0.1 0, 0.1 0.1, 0 0.1, 0 0))",
            0.01,
            "POLYGON ((0 0, 0.1 0, 0.1 0.1, 0 0.1, 0 0))",
        ),
        ("POLYGON ((0 0, 10 0.1, 20 0, 10 -0.1, 0 0))", 1.0, "POLYGON EMPTY"),
        // a collapsed hole disappears, a surviving one stays
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 4.1 4, 4.1 4.1, 4 4.1, 4 4))",
            1.0,
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))",
        ),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 3 2, 3 3, 2 3, 2 2), (5 5, 5.1 5, 5.1 5.1, 5 5.1, 5 5))",
            1.0,
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))",
        ),
        (
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2))",
            1.0,
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2))",
        ),
        // a shell that collapses takes its holes with it
        (
            "POLYGON ((0 0, 0.5 0, 0.5 0.5, 0 0.5, 0 0), (0.1 0.1, 0.2 0.1, 0.2 0.2, 0.1 0.2, 0.1 0.1))",
            1.0,
            "POLYGON EMPTY",
        ),
        // multipolygon parts that collapse drop; one survivor is a polygon, none is empty
        (
            "MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0)), ((20 20, 20.1 20, 20.1 20.1, 20 20.1, 20 20)))",
            1.0,
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))",
        ),
        (
            "MULTIPOLYGON (((0 0, 4 0, 4 4, 0 4, 0 0)), ((20 20, 20.1 20, 20.1 20.1, 20 20.1, 20 20)), ((10 0, 14 0, 14 4, 10 4, 10 0)))",
            1.0,
            "MULTIPOLYGON (((0 0, 4 0, 4 4, 0 4, 0 0)), ((10 0, 14 0, 14 4, 10 4, 10 0)))",
        ),
        (
            "MULTIPOLYGON (((0 0, 0.1 0, 0.1 0.1, 0 0.1, 0 0)), ((20 20, 20.1 20, 20.1 20.1, 20 20.1, 20 20)))",
            1.0,
            "MULTIPOLYGON EMPTY",
        ),
        // lines collapse to their two end points; a closed one to a point pair
        (
            "LINESTRING (0 0, 0.3 0.1, 0.6 -0.1, 1 0)",
            1.0,
            "LINESTRING (0 0, 1 0)",
        ),
        (
            "LINESTRING (0 0, 0.5 0.5, 1 0, 0 0)",
            1.0,
            "LINESTRING (0 0, 0 0)",
        ),
        (
            "GEOMETRYCOLLECTION (POLYGON ((0 0, 0.1 0, 0.05 0.1, 0 0)), LINESTRING (0 0, 0.3 0.1, 1 0), POINT (1 1))",
            1.0,
            "GEOMETRYCOLLECTION (LINESTRING (0 0, 1 0), POINT (1 1))",
        ),
    ];
    for (input, tolerance, expected) in cases {
        let got = one(input).simplify(tolerance, false).unwrap();
        let want = one(expected);
        assert_eq!(got.to_wkt().unwrap(), want.to_wkt().unwrap(), "{input}");
    }
}

/// GEOS also drops the start vertex of a ring when it lies within the tolerance of the chord
/// between its neighbours, and closes the ring on the next vertex.
#[test]
fn rdp_removes_the_ring_start_vertex_when_it_is_redundant() {
    let cases: [(&str, f64, &str); 6] = [
        // start on a straight edge of a triangle
        (
            "POLYGON ((2.5 5, 0 0, 10 0, 5 10, 2.5 5))",
            0.5,
            "POLYGON ((0 0, 10 0, 5 10, 0 0))",
        ),
        // start in the middle of an edge of a square: the ring now closes on the next corner
        (
            "POLYGON ((5 0, 10 0, 10 10, 0 10, 0 0, 5 0))",
            0.5,
            "POLYGON ((10 0, 10 10, 0 10, 0 0, 10 0))",
        ),
        (
            "POLYGON ((5 0, 10 0, 10 10, 0 10, 0 0, 5 0))",
            0.0,
            "POLYGON ((10 0, 10 10, 0 10, 0 0, 10 0))",
        ),
        // a start vertex that is a corner stays
        (
            "POLYGON ((0 0, 5 0.1, 10 0, 10 10, 0 10, 0 0))",
            0.5,
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))",
        ),
        // holes too
        (
            "POLYGON ((0 0, 20 0, 20 20, 0 20, 0 0), (5 7, 5 5, 10 5, 10 10, 5 10, 5 7))",
            0.5,
            "POLYGON ((0 0, 20 0, 20 20, 0 20, 0 0), (5 5, 10 5, 10 10, 5 10, 5 5))",
        ),
        // a closed line string keeps its end points
        (
            "LINESTRING (5 0, 10 0, 10 10, 0 10, 0 0, 5 0)",
            0.5,
            "LINESTRING (5 0, 10 0, 10 10, 0 10, 0 0, 5 0)",
        ),
    ];
    for (input, tolerance, expected) in cases {
        let got = one(input).simplify(tolerance, false).unwrap();
        let want = one(expected);
        assert_eq!(got.to_wkt().unwrap(), want.to_wkt().unwrap(), "{input}");
    }
}

#[test]
fn rdp_zero_tolerance_drops_collinear_vertices() {
    // GEOS: a vertex at distance 0 is within tolerance 0.
    let out = one("LINESTRING (0 0, 1 1, 2 2, 3 0)")
        .simplify(0.0, false)
        .unwrap();
    assert_eq!(
        out.to_wkt().unwrap(),
        one("LINESTRING (0 0, 2 2, 3 0)").to_wkt().unwrap()
    );
}

#[test]
fn simplify_leaves_points_and_nulls() {
    let src = col(&[Some("POINT (1 2)"), None]);
    let out = src.simplify(5.0, true).unwrap();
    assert_geom_eq(&out, &[Some("POINT (1 2)"), None]);
}

#[test]
fn convex_hull_of_l_shape() {
    let out = one(L_SHAPE).unary(UnaryOp::ConvexHull).unwrap();
    assert_geom_eq(&out, &[Some("POLYGON ((0 0, 4 0, 4 1, 1 4, 0 4, 0 0))")]);
}

#[test]
fn convex_hull_of_degenerate_inputs() {
    let src = col(&[Some("POINT (1 1)"), Some("MULTIPOINT (0 0, 2 2)")]);
    let out = src.unary(UnaryOp::ConvexHull).unwrap();
    assert_geom_eq(&out, &[Some("POINT (1 1)"), Some("LINESTRING (0 0, 2 2)")]);
}

#[test]
fn envelope_of_triangle_point_and_line() {
    let src = col(&[
        Some("POLYGON ((1 1, 5 2, 3 6, 1 1))"),
        Some("POINT (2 3)"),
        Some("LINESTRING (0 1, 4 1)"),
    ]);
    let out = src.unary(UnaryOp::Envelope).unwrap();
    assert_geom_eq(
        &out,
        &[
            Some("POLYGON ((1 1, 5 1, 5 6, 1 6, 1 1))"),
            Some("POINT (2 3)"),
            Some("LINESTRING (0 1, 4 1)"),
        ],
    );
}

#[test]
fn boundary_of_each_kind() {
    let src = col(&[
        Some("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"),
        Some("POLYGON ((0 0, 8 0, 8 8, 0 8, 0 0), (2 2, 4 2, 4 4, 2 2))"),
        Some("LINESTRING (0 0, 1 1, 2 0)"),
        Some("LINESTRING (0 0, 1 1, 1 0, 0 0)"),
    ]);
    let out = src.unary(UnaryOp::Boundary).unwrap();
    assert_geom_eq(
        &out,
        &[
            Some("LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)"),
            Some("MULTILINESTRING ((0 0, 8 0, 8 8, 0 8, 0 0), (2 2, 4 2, 4 4, 2 2))"),
            Some("MULTIPOINT (0 0, 2 0)"),
            Some("MULTIPOINT EMPTY"),
        ],
    );
}

#[test]
fn make_valid_repairs_a_bowtie() {
    let src = one(BOWTIE);
    assert_eq!(src.is_valid(), vec![Some(false)]);
    let out = src.unary(UnaryOp::MakeValid).unwrap();
    assert_eq!(out.is_valid(), vec![Some(true)]);
    // Two triangles meeting at (1 1), each of area 1.
    assert!(close(out.area()[0].unwrap(), 2.0, 1e-9));
    assert_eq!(out.geom_type(), vec![Some("MultiPolygon".to_string())]);
}

#[test]
fn make_valid_keeps_valid_geometry() {
    let out = col(&[
        Some("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"),
        Some("POINT (1 1)"),
    ])
    .unary(UnaryOp::MakeValid)
    .unwrap();
    assert_geom_eq(
        &out,
        &[
            Some("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"),
            Some("POINT (1 1)"),
        ],
    );
}

#[test]
fn representative_point_lies_inside_a_concave_polygon() {
    let src = col(&[Some(L_SHAPE), Some("POINT (3 4)"), None]);
    let out = src.unary(UnaryOp::RepresentativePoint).unwrap();
    assert_eq!(out.geom_type()[0].as_deref(), Some("Point"));
    assert_eq!(
        out.predicate(&src, Predicate::Within).unwrap()[0],
        Some(true)
    );
    assert_geom_eq(&out.take(&[1]).unwrap(), &[Some("POINT (3 4)")]);
    assert!(out.to_wkt().unwrap()[2].is_none());
}

#[test]
fn exterior_only_for_polygons() {
    let src = col(&[
        Some("POLYGON ((0 0, 8 0, 8 8, 0 8, 0 0), (2 2, 4 2, 4 4, 2 2))"),
        Some("POINT (1 1)"),
        Some("LINESTRING (0 0, 1 1)"),
    ]);
    let out = src.unary(UnaryOp::Exterior).unwrap();
    assert_geom_eq(
        &out,
        &[Some("LINESTRING (0 0, 8 0, 8 8, 0 8, 0 0)"), None, None],
    );
}

#[test]
fn translate_is_exact() {
    let src = GeoColumn::from_wkt(
        &[Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"), None],
        Some("EPSG:4326"),
    )
    .unwrap();
    let out = src.translate(10.0, -2.0).unwrap();
    assert_eq!(out.crs().as_deref(), Some("EPSG:4326"));
    assert_geom_eq(
        &out,
        &[Some("POLYGON ((10 -2, 11 -2, 11 -1, 10 -2))"), None],
    );
}

#[test]
fn scale_about_center_centroid_and_coordinate() {
    let tri = "POLYGON ((0 0, 6 0, 0 3, 0 0))";
    // Bounding box centre (3, 1.5): the box [0,6]x[0,3] doubles to [-3,9]x[-1.5,4.5].
    let center = one(tri).scale(2.0, 2.0, &Origin::Center).unwrap();
    assert_eq!(center.bounds()[0], Some([-3.0, -1.5, 9.0, 4.5]));
    // Centroid (2, 1).
    let centroid = one(tri).scale(2.0, 2.0, &Origin::Centroid).unwrap();
    assert_eq!(centroid.bounds()[0], Some([-2.0, -1.0, 10.0, 5.0]));
    let coord = one(tri).scale(2.0, 3.0, &Origin::Coord(0.0, 0.0)).unwrap();
    assert_geom_eq(&coord, &[Some("POLYGON ((0 0, 12 0, 0 9, 0 0))")]);
}

#[test]
fn rotate_quarter_turn_counter_clockwise() {
    let sq = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";
    let out = one(sq).rotate(90.0, &Origin::Coord(0.0, 0.0)).unwrap();
    let b = out.bounds()[0].unwrap();
    for (got, want) in b.iter().zip([-2.0, 0.0, 0.0, 2.0]) {
        assert!(close(*got, want, 1e-9), "{b:?}");
    }
    // Around its own centre a square keeps its bounds.
    let spun = one(sq).rotate(90.0, &Origin::Center).unwrap();
    let b = spun.bounds()[0].unwrap();
    for (got, want) in b.iter().zip([0.0, 0.0, 2.0, 2.0]) {
        assert!(close(*got, want, 1e-9), "{b:?}");
    }
}

#[test]
fn accessors_x_y() {
    let src = col(&[
        Some("POINT (1.5 -2)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("POINT EMPTY"),
        None,
    ]);
    assert_eq!(src.x(), vec![Some(1.5), None, None, None]);
    assert_eq!(src.y(), vec![Some(-2.0), None, None, None]);
}

#[test]
fn accessor_geom_type_uses_shapely_names() {
    let src = col(&[
        Some("POINT (0 0)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("MULTIPOINT (0 0, 1 1)"),
        Some("MULTILINESTRING ((0 0, 1 1))"),
        Some("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))"),
        Some("GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (0 0, 1 1))"),
        Some("POINT EMPTY"),
        None,
    ]);
    let types = src.geom_type();
    let names: Vec<Option<&str>> = types.iter().map(|n| n.as_deref()).collect();
    assert_eq!(
        names,
        vec![
            Some("Point"),
            Some("LineString"),
            Some("Polygon"),
            Some("MultiPoint"),
            Some("MultiLineString"),
            Some("MultiPolygon"),
            Some("GeometryCollection"),
            Some("Point"),
            None,
        ]
    );
}

#[test]
fn accessor_is_empty_and_count_coordinates() {
    let src = col(&[
        Some("POINT EMPTY"),
        Some("POLYGON EMPTY"),
        Some("POINT (0 0)"),
        Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        Some("MULTIPOINT (0 0, 1 1)"),
        None,
    ]);
    assert_eq!(
        src.is_empty_rows(),
        vec![
            Some(true),
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            None
        ]
    );
    assert_eq!(
        src.count_coordinates(),
        vec![Some(0), Some(0), Some(1), Some(4), Some(2), None]
    );
}

#[test]
fn boundary_of_many_lines_follows_the_mod_2_rule_with_signed_zero() {
    // (0 0) is an end of three lines (odd: kept), (1 0) of two (even: dropped), -0.0 and 0.0
    // are the same vertex, and the far ends are each used once.
    let col = GeoColumn::from_wkt(
        &[Some(
            "MULTILINESTRING ((0 0, 1 0), (-0 0, 5 5), (0 0, 9 9), (1 0, 7 7), (3 3, 4 4, 3 3))",
        )],
        None,
    )
    .unwrap();
    let out = col.unary(UnaryOp::Boundary).unwrap().to_wkt().unwrap();
    assert_eq!(
        out,
        vec![Some("MULTIPOINT((0 0),(5 5),(9 9),(7 7))".to_string())]
    );
}
