//! Batch E: constructive geometry (hulls, bounding shapes, triangulations, sampling, grids).

use geo::{Area, Contains, Coord, Geometry, Intersects, Point};

use crate::{GeoColumn, GridFeature, GridType};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

fn one(text: &str) -> GeoColumn {
    col(&[Some(text)])
}

fn geom(column: &GeoColumn, row: usize) -> Geometry<f64> {
    column.geo_rows()[row].clone().expect("a geometry")
}

fn area(column: &GeoColumn, row: usize) -> f64 {
    geom(column, row).unsigned_area()
}

fn parts(column: &GeoColumn, row: usize) -> Vec<Geometry<f64>> {
    match geom(column, row) {
        Geometry::GeometryCollection(c) => c.0,
        Geometry::MultiLineString(m) => m.0.into_iter().map(Geometry::LineString).collect(),
        Geometry::MultiPolygon(m) => m.0.into_iter().map(Geometry::Polygon).collect(),
        other => vec![other],
    }
}

const L_SHAPE: &str = "POLYGON ((0 0, 6 0, 6 2, 2 2, 2 6, 0 6, 0 0))";
const SQUARE: &str = "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))";
const HOLED: &str = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))";

// -- concave hull ---------------------------------------------------------------------

#[test]
fn concave_hull_ratio_zero_erodes_the_notch_and_ratio_one_is_the_convex_hull() {
    let s = one(L_SHAPE);
    let concave = s.concave_hull(0.0, false).unwrap();
    // GEOS gives (6 2, 2 6, 0 6, 2 2, 0 0, 6 0): the arm is cut diagonally, area 22.
    assert!((area(&concave, 0) - 22.0).abs() < 1e-9);
    let convex = s.concave_hull(1.0, false).unwrap();
    assert!((area(&convex, 0) - 28.0).abs() < 1e-9);
}

#[test]
fn concave_hull_covers_the_input_points_and_sits_inside_the_convex_hull() {
    let s = one("MULTIPOINT ((0 0), (10 0), (10 10), (0 10), (5 5), (2 8), (8 2), (5 9))");
    let hull = geom(&s.concave_hull(0.2, false).unwrap(), 0);
    let convex = geom(&s.unary(crate::UnaryOp::ConvexHull).unwrap(), 0);
    for p in [(0.0, 0.0), (10.0, 10.0), (5.0, 5.0), (5.0, 9.0)] {
        assert!(hull.intersects(&Point::new(p.0, p.1)), "{p:?}");
    }
    assert!(hull.unsigned_area() <= convex.unsigned_area() + 1e-9);
}

#[test]
fn concave_hull_of_degenerate_inputs_follows_geos() {
    let rows = col(&[
        Some("POINT (1 1)"),
        Some("MULTIPOINT ((0 0), (2 2))"),
        Some("LINESTRING (0 0, 10 0)"),
        Some("MULTIPOINT ((0 0), (5 0), (10 0))"),
        Some("POLYGON EMPTY"),
        None,
    ]);
    let hulls = rows.concave_hull(0.0, false).unwrap();
    let text = hulls.to_wkt().unwrap();
    assert_eq!(text[0].as_deref(), Some("POINT(1 1)"));
    assert_eq!(text[1].as_deref(), Some("LINESTRING(0 0,2 2)"));
    assert_eq!(text[2].as_deref(), Some("LINESTRING(0 0,10 0)"));
    assert_eq!(text[3].as_deref(), Some("LINESTRING(0 0,10 0)"));
    assert!(hulls.is_empty_rows()[4].unwrap_or(false));
    assert_eq!(text[5], None);
}

#[test]
fn concave_hull_with_holes_can_carve_an_interior_gap() {
    // A ring of points with nothing in the middle: long interior triangles become a hole.
    let mut points = Vec::new();
    for i in 0..12 {
        let a = f64::from(i) * std::f64::consts::TAU / 12.0;
        points.push(format!("({} {})", 10.0 * a.cos(), 10.0 * a.sin()));
        points.push(format!("({} {})", 6.0 * a.cos(), 6.0 * a.sin()));
    }
    let s = one(&format!("MULTIPOINT ({})", points.join(", ")));
    let solid = s.concave_hull(0.3, false).unwrap();
    let holed = s.concave_hull(0.3, true).unwrap();
    assert!(area(&holed, 0) <= area(&solid, 0) + 1e-9);
}

// -- minimum rotated rectangle --------------------------------------------------------

#[test]
fn minimum_rotated_rectangle_covers_the_input_and_is_never_smaller() {
    let s = one("POLYGON ((0 0, 4 2, 3 5, -1 3, 0 0))");
    let rect = s.minimum_rotated_rectangle().unwrap();
    let g = geom(&s, 0);
    let r = geom(&rect, 0);
    assert!(r.unsigned_area() >= g.unsigned_area() - 1e-9);
    assert!(r.intersects(&g));
    use geo::{CoordsIter, Distance, Euclidean};
    for c in g.coords_iter() {
        assert!(Euclidean.distance(&r, &Point::from(c)) < 1e-9, "{c:?}");
    }
}

#[test]
fn minimum_rotated_rectangle_of_a_tilted_rectangle_is_itself() {
    let s = one("POLYGON ((0 0, 4 4, 3 5, -1 1, 0 0))");
    let rect = s.minimum_rotated_rectangle().unwrap();
    assert!((area(&rect, 0) - area(&s, 0)).abs() < 1e-9);
}

#[test]
fn minimum_rotated_rectangle_of_lines_and_points_degenerates_like_geos() {
    let s = col(&[
        Some("POINT (1 1)"),
        Some("LINESTRING (0 0, 10 0)"),
        Some("MULTIPOINT ((0 0), (1 1), (3 3))"),
        None,
    ]);
    let text = s.minimum_rotated_rectangle().unwrap().to_wkt().unwrap();
    assert_eq!(text[0].as_deref(), Some("POINT(1 1)"));
    assert_eq!(text[1].as_deref(), Some("LINESTRING(0 0,10 0)"));
    assert_eq!(text[2].as_deref(), Some("LINESTRING(0 0,3 3)"));
    assert_eq!(text[3], None);
}

// -- minimum bounding circle ----------------------------------------------------------

#[test]
fn bounding_radius_of_known_shapes() {
    let s = col(&[
        Some("POINT (1 1)"),
        Some("LINESTRING (0 0, 10 0)"),
        Some(SQUARE),
        Some("POLYGON ((0 0, 6 0, 3 5, 0 0))"),
        None,
    ]);
    let r = s.minimum_bounding_radius();
    assert_eq!(r[0], Some(0.0));
    assert!((r[1].unwrap() - 5.0).abs() < 1e-12);
    assert!((r[2].unwrap() - 8.0_f64.sqrt()).abs() < 1e-12);
    // Acute triangle: circumscribed circle, R = abc / 4 area.
    let (a, b, c) = (6.0, 34.0_f64.sqrt(), 34.0_f64.sqrt());
    assert!((r[3].unwrap() - a * b * c / (4.0 * 15.0)).abs() < 1e-9);
    assert_eq!(r[4], None);
}

#[test]
fn bounding_circle_is_a_32_gon_that_follows_the_shapely_buffer_layout() {
    let s = one(SQUARE);
    let circle = s.minimum_bounding_circle().unwrap();
    let Geometry::Polygon(p) = geom(&circle, 0) else {
        panic!("a polygon");
    };
    let ring: Vec<Coord<f64>> = p.exterior().0.clone();
    assert_eq!(ring.len(), 33);
    let r = 8.0_f64.sqrt();
    // Starts at angle 0 and turns clockwise.
    assert!((ring[0].x - (2.0 + r)).abs() < 1e-9 && (ring[0].y - 2.0).abs() < 1e-9);
    assert!(ring[1].y < 2.0);
    for c in &ring {
        assert!((((c.x - 2.0).powi(2) + (c.y - 2.0).powi(2)).sqrt() - r).abs() < 1e-9);
    }
}

#[test]
fn bounding_circle_of_a_point_is_the_point() {
    let text = one("POINT (1 1)")
        .minimum_bounding_circle()
        .unwrap()
        .to_wkt()
        .unwrap();
    assert_eq!(text[0].as_deref(), Some("POINT(1 1)"));
}

// -- maximum inscribed circle ---------------------------------------------------------

#[test]
fn inscribed_circle_of_a_holed_square_matches_the_geos_search() {
    let s = one(HOLED);
    let line = s.maximum_inscribed_circle(None).unwrap();
    let Geometry::LineString(l) = geom(&line, 0) else {
        panic!("a line");
    };
    let (center, edge) = (l.0[0], l.0[1]);
    let radius = ((center.x - edge.x).powi(2) + (center.y - edge.y).powi(2)).sqrt();
    // GEOS (tolerance diag / 1000): center (6.484375, 6.484375), radius 3.51...
    assert!((center.x - 6.484375).abs() < 1e-9 && (center.y - 6.484375).abs() < 1e-9);
    assert!(radius > 3.5 && radius < 3.6, "{radius}");
    assert!(geom(&s, 0).contains(&Point::new(center.x, center.y)));
}

#[test]
fn inscribed_circle_only_accepts_polygons() {
    let s = col(&[Some("POINT (1 1)"), Some("LINESTRING (0 0, 1 1)"), None]);
    let out = s.maximum_inscribed_circle(None).unwrap().to_wkt().unwrap();
    assert_eq!(out, vec![None, None, None]);
}

// -- triangulations -------------------------------------------------------------------

fn total_area(column: &GeoColumn) -> f64 {
    (0..column.len()).map(|i| area(column, i)).sum()
}

#[test]
fn delaunay_triangles_tile_the_convex_hull_one_row_per_triangle() {
    let s = one("MULTIPOINT ((0 0), (4 0), (4 4), (0 4), (2 1), (1 3))");
    let tri = s.delaunay_triangles(0.0, false).unwrap();
    assert_eq!(tri.len(), 6);
    let hull = geom(&s.unary(crate::UnaryOp::ConvexHull).unwrap(), 0);
    assert!((total_area(&tri) - hull.unsigned_area()).abs() < 1e-9);
    assert!((0..tri.len()).all(|i| matches!(geom(&tri, i), Geometry::Polygon(_))));
}

#[test]
fn delaunay_treats_the_whole_series_as_one_point_cloud() {
    let s = col(&[
        Some("POINT (0 0)"),
        Some("POINT (4 0)"),
        None,
        Some("LINESTRING (0 4, 0 5)"),
    ]);
    let tri = s.delaunay_triangles(0.0, false).unwrap();
    // (0 0), (4 0), (0 4), (0 5): two triangles.
    assert_eq!(tri.len(), 2);
}

#[test]
fn delaunay_of_too_few_or_collinear_points_has_no_rows() {
    for wkt in [
        "POINT (1 1)",
        "MULTIPOINT ((0 0), (2 2))",
        "MULTIPOINT ((0 0), (1 1), (2 2))",
    ] {
        assert_eq!(
            one(wkt).delaunay_triangles(0.0, false).unwrap().len(),
            0,
            "{wkt}"
        );
    }
}

#[test]
fn delaunay_only_edges_returns_unique_lines() {
    let s = one("MULTIPOINT ((0 0), (4 0), (0 4))");
    let edges = s.delaunay_triangles(0.0, true).unwrap();
    assert_eq!(edges.len(), 3);
    assert!((0..3).all(|i| matches!(geom(&edges, i), Geometry::LineString(_))));
    // Collinear points chain consecutive neighbours.
    let line = one("MULTIPOINT ((0 0), (2 0), (5 0))");
    assert_eq!(line.delaunay_triangles(0.0, true).unwrap().len(), 2);
}

#[test]
fn delaunay_tolerance_snaps_close_vertices() {
    let s = one("MULTIPOINT ((0 0), (0.0001 0), (5 5), (5 0))");
    assert_eq!(s.delaunay_triangles(0.0, false).unwrap().len(), 2);
    assert_eq!(s.delaunay_triangles(0.01, false).unwrap().len(), 1);
}

#[test]
fn constrained_delaunay_respects_holes_and_concavity() {
    let l = one(L_SHAPE).constrained_delaunay_triangles().unwrap();
    assert_eq!(parts(&l, 0).len(), 4);
    let total: f64 = parts(&l, 0).iter().map(Area::unsigned_area).sum();
    assert!((total - 20.0).abs() < 1e-9);
    let holed = one(HOLED).constrained_delaunay_triangles().unwrap();
    assert_eq!(parts(&holed, 0).len(), 8);
    let total: f64 = parts(&holed, 0).iter().map(Area::unsigned_area).sum();
    assert!((total - 96.0).abs() < 1e-9);
}

#[test]
fn constrained_delaunay_of_non_polygons_is_empty() {
    let s = col(&[Some("POINT (1 1)"), Some("LINESTRING (0 0, 3 4)"), None]);
    let out = s.constrained_delaunay_triangles().unwrap();
    assert_eq!(out.is_empty_rows(), vec![Some(true), Some(true), None]);
}

// -- Voronoi --------------------------------------------------------------------------

#[test]
fn voronoi_has_one_cell_per_unique_site_and_covers_the_default_envelope() {
    let s = one("MULTIPOINT ((0 0), (2 2), (5 1), (0 0))");
    let cells = s.voronoi_polygons(0.0, None, false, false).unwrap();
    assert_eq!(cells.len(), 3);
    // Envelope 0..5 x 0..2 expanded by max(width, height) = 5: [-5, 10] x [-5, 7].
    assert!((total_area(&cells) - 15.0 * 12.0).abs() < 1e-6);
    // Cells come in site order and contain their site.
    for (i, site) in [(0.0, 0.0), (2.0, 2.0), (5.0, 1.0)].into_iter().enumerate() {
        assert!(geom(&cells, i).intersects(&Point::new(site.0, site.1)));
    }
}

#[test]
fn voronoi_of_collinear_sites_gives_strips() {
    let s = one("MULTIPOINT ((0 0), (2 0), (5 0))");
    let cells = s.voronoi_polygons(0.0, None, false, false).unwrap();
    assert_eq!(cells.len(), 3);
    assert!((total_area(&cells) - 15.0 * 10.0).abs() < 1e-6);
}

#[test]
fn voronoi_of_two_sites_splits_the_window_by_the_bisector() {
    let s = one("MULTIPOINT ((0 0), (2 2))");
    let cells = s.voronoi_polygons(0.0, None, false, false).unwrap();
    assert_eq!(cells.len(), 2);
    assert!((area(&cells, 0) - area(&cells, 1)).abs() < 1e-9);
    assert!((total_area(&cells) - 36.0).abs() < 1e-9);
}

#[test]
fn voronoi_extend_to_grows_the_clip_envelope() {
    let s = one("MULTIPOINT ((0 0), (2 2), (5 1))");
    let big = one("POLYGON ((-20 -20, 30 -20, 30 20, -20 20, -20 -20))");
    let cells = s.voronoi_polygons(0.0, Some(&big), false, false).unwrap();
    assert!((total_area(&cells) - 50.0 * 40.0).abs() < 1e-6);
    let small = one("POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))");
    let cells = s.voronoi_polygons(0.0, Some(&small), false, false).unwrap();
    assert!((total_area(&cells) - 15.0 * 12.0).abs() < 1e-6);
    assert!(s
        .voronoi_polygons(
            0.0,
            Some(&col(&[Some("POINT (1 1)"), Some("POINT (2 2)")])),
            false,
            false
        )
        .is_err());
}

#[test]
fn voronoi_only_edges_returns_the_interior_sides() {
    let s = one("MULTIPOINT ((0 0), (2 2), (5 1))");
    let edges = s.voronoi_polygons(0.0, None, true, false).unwrap();
    assert_eq!(edges.len(), 3);
    assert!((0..3).all(|i| matches!(geom(&edges, i), Geometry::LineString(_))));
}

#[test]
fn voronoi_of_fewer_than_two_sites_has_no_rows() {
    for wkt in ["POINT (1 1)", "MULTIPOINT ((1 1), (1 1))"] {
        let out = one(wkt).voronoi_polygons(0.0, None, false, false).unwrap();
        assert_eq!(out.len(), 0, "{wkt}");
    }
}

// -- sample points --------------------------------------------------------------------

#[test]
fn sample_points_are_inside_reproducible_and_counted() {
    let s = col(&[Some(L_SHAPE), Some(HOLED), Some("LINESTRING (0 0, 10 0)")]);
    let a = s.sample_points(&[50], Some(7)).unwrap();
    let b = s.sample_points(&[50], Some(7)).unwrap();
    let c = s.sample_points(&[50], Some(8)).unwrap();
    assert_eq!(a.to_wkt().unwrap(), b.to_wkt().unwrap());
    assert_ne!(a.to_wkt().unwrap(), c.to_wkt().unwrap());
    for row in 0..3 {
        let pts = match geom(&a, row) {
            Geometry::MultiPoint(m) => m.0,
            other => panic!("expected a multipoint, got {other:?}"),
        };
        assert_eq!(pts.len(), 50);
    }
    let source = s.geo_rows();
    for (row, polygon) in [(0, true), (1, true), (2, false)] {
        let Geometry::MultiPoint(m) = geom(&a, row) else {
            unreachable!()
        };
        let g = source[row].as_ref().unwrap();
        for p in &m.0 {
            if polygon {
                assert!(g.contains(p), "row {row}: {p:?}");
            } else {
                assert!(p.y().abs() < 1e-12 && (0.0..=10.0).contains(&p.x()));
            }
        }
    }
}

#[test]
fn sample_points_per_row_sizes_and_unsupported_types() {
    let s = col(&[
        Some(SQUARE),
        Some(SQUARE),
        Some("POINT (1 1)"),
        Some("POLYGON EMPTY"),
        None,
    ]);
    let out = s.sample_points(&[3, 0, 5, 5, 5], Some(1)).unwrap();
    let counts: Vec<Option<usize>> = (0..5)
        .map(|i| match out.geo_rows()[i].clone() {
            Some(Geometry::MultiPoint(m)) => Some(m.0.len()),
            _ => None,
        })
        .collect();
    assert_eq!(counts, vec![Some(3), Some(0), Some(0), Some(0), None]);
}

#[test]
fn sample_points_rejects_a_size_list_of_the_wrong_length() {
    let s = col(&[Some(SQUARE), Some(SQUARE)]);
    assert!(s.sample_points(&[1, 2, 3], None).is_err());
}

// -- make_grid ------------------------------------------------------------------------

#[test]
fn square_grid_matches_geopandas_cell_layout() {
    let s = one("POLYGON ((0 0, 5 0, 5 3, 0 3, 0 0))");
    let polys = s
        .make_grid(
            2.0,
            GridType::Square,
            GridFeature::Polygons,
            None,
            false,
            false,
        )
        .unwrap();
    // arange(0, 5 + 2, 2) = 0, 2, 4, 6 -> 3 x 2 cells; x is the outer loop.
    assert_eq!(polys.len(), 6);
    assert_eq!(
        polys.to_wkt().unwrap()[0].as_deref(),
        Some("POLYGON((0 0,2 0,2 2,0 2,0 0))")
    );
    assert_eq!(
        polys.to_wkt().unwrap()[1].as_deref(),
        Some("POLYGON((0 2,2 2,2 4,0 4,0 2))")
    );
    let centers = s
        .make_grid(
            2.0,
            GridType::Square,
            GridFeature::Centers,
            None,
            false,
            false,
        )
        .unwrap();
    assert_eq!(centers.to_wkt().unwrap()[0].as_deref(), Some("POINT(1 1)"));
    let corners = s
        .make_grid(
            2.0,
            GridType::Square,
            GridFeature::Corners,
            None,
            false,
            false,
        )
        .unwrap();
    assert_eq!(corners.len(), 12);
}

#[test]
fn grid_intersect_filters_to_cells_touching_the_input() {
    let s = one("POLYGON ((0 0, 3 0, 0 3, 0 0))");
    let all = s
        .make_grid(
            2.0,
            GridType::Square,
            GridFeature::Polygons,
            None,
            false,
            false,
        )
        .unwrap();
    let hit = s
        .make_grid(
            2.0,
            GridType::Square,
            GridFeature::Polygons,
            None,
            true,
            false,
        )
        .unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(hit.len(), 3);
}

#[test]
fn hexagon_grid_polygons_have_six_corners_and_the_requested_width() {
    let s = one("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))");
    for flat in [false, true] {
        let hex = s
            .make_grid(
                2.0,
                GridType::Hexagon,
                GridFeature::Polygons,
                None,
                true,
                flat,
            )
            .unwrap();
        assert!(hex.len() > 10);
        let g = geom(&hex, 0);
        let Geometry::Polygon(p) = g else { panic!() };
        assert_eq!(p.exterior().0.len(), 7);
        // Area of a regular hexagon with apothem 1: 2 * sqrt(3).
        assert!((p.unsigned_area() - 2.0 * 3.0_f64.sqrt()).abs() < 1e-9);
    }
}

#[test]
fn grid_rejects_a_non_positive_cell_size_and_empty_input() {
    let s = one(SQUARE);
    assert!(s
        .make_grid(
            0.0,
            GridType::Square,
            GridFeature::Polygons,
            None,
            true,
            false
        )
        .is_err());
    let empty = col(&[None]);
    assert!(empty
        .make_grid(
            1.0,
            GridType::Square,
            GridFeature::Polygons,
            None,
            true,
            false
        )
        .is_err());
}
