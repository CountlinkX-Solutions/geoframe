//! Deck.gl binary buffers: structure, row mapping and skipping rules.

use geo::{Geometry, Point};

use crate::deck::gather_rows;
use crate::{deck_buffers, DeckBuffers, GeoColumn};

fn column(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, Some("EPSG:4326")).unwrap()
}

fn buffers(rows: &[Option<&str>]) -> DeckBuffers {
    DeckBuffers::from_column(&column(rows))
}

/// Total area of the triangles in `triangles`, over interleaved `positions`.
fn triangle_area(positions: &[f64], triangles: &[u32]) -> f64 {
    triangles
        .chunks(3)
        .map(|t| {
            let p = |i: u32| (positions[2 * i as usize], positions[2 * i as usize + 1]);
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() / 2.0
        })
        .sum()
}

#[test]
fn points_keep_row_index_through_nulls_empties_and_multis() {
    let b = buffers(&[
        Some("POINT (1 2)"),
        None,
        Some("MULTIPOINT ((3 4), (5 6))"),
        Some("POINT EMPTY"),
        Some("POINT (7 8)"),
    ]);
    assert_eq!(b.point_positions, vec![1., 2., 3., 4., 5., 6., 7., 8.]);
    assert_eq!(b.point_rows, vec![0, 2, 2, 4]);
    assert!(b.line_rows.is_empty() && b.polygon_rows.is_empty());
    assert_eq!(b.skipped, 0);
}

#[test]
fn lines_get_vertex_offsets_per_part() {
    let b = buffers(&[
        Some("LINESTRING (0 0, 1 1)"),
        Some("MULTILINESTRING ((2 2, 3 3, 4 4), (5 5, 6 6))"),
    ]);
    assert_eq!(b.line_starts, vec![0, 2, 5, 7]);
    assert_eq!(b.line_rows, vec![0, 1, 1]);
    assert_eq!(b.line_positions.len(), 14);
    assert_eq!(&b.line_positions[4..6], &[2., 2.]);
}

#[test]
fn polygon_with_hole_has_ring_offsets_and_exact_triangulation() {
    let b = buffers(&[Some(
        "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
    )]);
    assert_eq!(b.polygon_starts, vec![0, 10]);
    assert_eq!(b.ring_starts, vec![0, 5, 10]);
    assert_eq!(b.polygon_rows, vec![0]);
    assert_eq!(b.polygon_positions.len(), 20);
    assert!(b.triangles.len().is_multiple_of(3) && !b.triangles.is_empty());
    assert!(b.triangles.iter().all(|&i| i < 10));
    assert!((triangle_area(&b.polygon_positions, &b.triangles) - 96.0).abs() < 1e-9);
}

#[test]
fn multipolygon_offsets_triangles_by_polygon_start() {
    let b = buffers(&[
        None,
        Some("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((5 5, 7 5, 7 7, 5 7, 5 5)))"),
    ]);
    assert_eq!(b.polygon_starts, vec![0, 5, 10]);
    assert_eq!(b.ring_starts, vec![0, 5, 10]);
    assert_eq!(b.polygon_rows, vec![1, 1]);
    let second: Vec<u32> = b.triangles.iter().copied().filter(|&i| i >= 5).collect();
    assert_eq!(
        second.len(),
        6,
        "two triangles per square, second polygon at 5..10"
    );
    assert!((triangle_area(&b.polygon_positions, &b.triangles) - 5.0).abs() < 1e-9);
}

#[test]
fn collections_are_flattened_into_their_layers_under_one_row() {
    let b = buffers(&[Some(
        "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1), POLYGON ((0 0, 1 0, 1 1, 0 0)))",
    )]);
    assert_eq!(b.point_rows, vec![0]);
    assert_eq!(b.line_rows, vec![0]);
    assert_eq!(b.polygon_rows, vec![0]);
}

#[test]
fn non_finite_coordinates_skip_the_part_and_are_counted() {
    let col = column(&[Some("POINT (0 0)")])
        .rebuild(&[
            Some(Geometry::Point(Point::new(f64::NAN, 1.0))),
            Some(Geometry::Point(Point::new(2.0, 3.0))),
            Some(Geometry::Point(Point::new(f64::INFINITY, 1.0))),
        ])
        .unwrap();
    let b = DeckBuffers::from_column(&col);
    assert_eq!(b.point_positions, vec![2., 3.]);
    assert_eq!(b.point_rows, vec![1]);
    assert_eq!(b.skipped, 2);
}

#[test]
fn gather_copies_fixed_stride_values_by_row() {
    let row_values = [10u8, 11, 20, 21, 30, 31];
    assert_eq!(
        gather_rows(&[2, 0, 2], &row_values, 2),
        vec![30, 31, 10, 11, 30, 31]
    );
    assert_eq!(
        gather_rows(&[5], &row_values, 2),
        vec![0, 0],
        "out of range rows are zero"
    );
}

#[test]
fn deck_buffers_lists_little_endian_named_buffers() {
    let col = column(&[Some("POINT (1 2)"), Some("POINT (3 4)")]);
    let colors = [255u8, 0, 0, 255, 0, 255, 0, 255];
    let sizes: Vec<u8> = [1.5f32, 2.5].iter().flat_map(|v| v.to_le_bytes()).collect();
    let (list, skipped) = deck_buffers(&col, &colors, &sizes);
    assert_eq!(skipped, 0);
    let get = |name: &str| {
        list.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| b.clone())
    };
    let positions = get("points.positions").unwrap();
    assert_eq!(positions.len(), 32);
    assert_eq!(&positions[8..16], &2.0f64.to_le_bytes());
    assert_eq!(
        get("points.rows").unwrap(),
        [0i32, 1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>()
    );
    assert_eq!(get("points.colors").unwrap(), colors.to_vec());
    assert_eq!(get("points.sizes").unwrap(), sizes);
    assert!(get("lines.positions").is_none(), "empty layers are omitted");
}

#[test]
fn deck_buffers_omits_style_buffers_without_style_input() {
    let col = column(&[Some("POINT (1 2)")]);
    let (list, _) = deck_buffers(&col, &[], &[]);
    assert!(list
        .iter()
        .all(|(n, _)| !n.ends_with(".colors") && !n.ends_with(".sizes")));
}
