//! Regression tests for inputs that used to abort the process (stack overflow, huge
//! allocation). Each test uses the failing input, at the reduced scale that is still above the
//! cap where the full input would be slow; `*_huge` variants use the real attack input.

use crate::{read_flatgeobuf, write_flatgeobuf, GeoColumn, GridFeature, GridType};
use tempfile::TempDir;

use crate::test_support::{ipc, path};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

// -- 1. WKT nesting -------------------------------------------------------------------------

fn nested_wkt(levels: usize) -> String {
    let mut text = "GEOMETRYCOLLECTION (".repeat(levels);
    text.push_str("POINT (1 2)");
    text.push_str(&")".repeat(levels));
    text
}

#[test]
fn wkt_just_above_the_nesting_cap_is_rejected() {
    let err = GeoColumn::from_wkt(
        &[Some("POINT (0 0)".to_string()), Some(nested_wkt(70))],
        None,
    )
    .unwrap_err();
    assert!(err.contains("invalid WKT at row 1"), "{err}");
    assert!(err.contains("nesting too deep"), "{err}");
}

#[test]
fn wkt_within_the_cap_still_parses() {
    assert!(GeoColumn::from_wkt(&[Some(nested_wkt(30))], None).is_ok());
}

#[test]
fn wkt_100k_levels_is_an_error_not_an_abort() {
    let err = GeoColumn::from_wkt(&[Some(nested_wkt(100_000))], None).unwrap_err();
    assert!(err.contains("nesting too deep"), "{err}");
}

// -- 2. EWKB / WKB recursion ----------------------------------------------------------------

/// `01 07000000 01000000` repeated, closed by an empty collection.
fn nested_wkb(levels: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..levels {
        bytes.extend_from_slice(&[1, 7, 0, 0, 0, 1, 0, 0, 0]);
    }
    bytes.extend_from_slice(&[1, 7, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

#[test]
fn wkb_just_above_the_nesting_cap_is_rejected() {
    let err = GeoColumn::from_wkb(&[Some(nested_wkb(70))], None).unwrap_err();
    assert!(err.contains("invalid WKB at row 0"), "{err}");
    assert!(err.contains("nesting too deep"), "{err}");
}

#[test]
fn wkb_within_the_cap_still_parses() {
    assert!(GeoColumn::from_wkb(&[Some(nested_wkb(20))], None).is_ok());
}

#[test]
fn wkb_20k_levels_is_an_error_not_an_abort() {
    let mut bytes = Vec::new();
    for _ in 0..20_000 {
        bytes.extend_from_slice(&[1, 7, 0, 0, 0, 1, 0, 0, 0]);
    }
    let err = GeoColumn::from_wkb(&[Some(bytes)], None).unwrap_err();
    assert!(err.contains("invalid WKB at row 0"), "{err}");
}

// -- 3. FlatGeobuf --------------------------------------------------------------------------

fn small_fgb(dir: &TempDir, name: &str, index: bool) -> (String, Vec<u8>) {
    let geometry = col(&[Some("POINT (1 2)"), Some("POINT (3 4)")]);
    let p = path(dir, name);
    let attrs = ipc(vec![(
        "id",
        std::sync::Arc::new(arrow_array::Int64Array::from(vec![1, 2])) as arrow_array::ArrayRef,
    )]);
    write_flatgeobuf(&p, &geometry, &attrs, "t", index).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    (p, bytes)
}

fn header_end(bytes: &[u8]) -> usize {
    12 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize
}

#[test]
fn flatgeobuf_feature_size_beyond_the_file_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (_, bytes) = small_fgb(&dir, "ok.fgb", false);
    let mut crafted = bytes[..header_end(&bytes)].to_vec();
    crafted.extend_from_slice(&[0xFF; 4]);
    let bad = path(&dir, "bad.fgb");
    std::fs::write(&bad, crafted).unwrap();
    let err = read_flatgeobuf(&bad, None).err().expect("error");
    assert!(
        err.contains("truncated or has a corrupt feature size"),
        "{err}"
    );
}

#[test]
fn flatgeobuf_second_feature_oversized_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (_, bytes) = small_fgb(&dir, "ok.fgb", false);
    let start = header_end(&bytes);
    let first = u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
    let mut crafted = bytes[..start + 4 + first].to_vec();
    crafted.extend_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
    crafted.extend_from_slice(&[0; 16]);
    let bad = path(&dir, "bad2.fgb");
    std::fs::write(&bad, crafted).unwrap();
    assert!(read_flatgeobuf(&bad, None).is_err());
}

#[test]
fn flatgeobuf_valid_files_still_read_with_and_without_index() {
    let dir = TempDir::new().unwrap();
    for index in [false, true] {
        let (p, _) = small_fgb(&dir, "v.fgb", index);
        assert_eq!(read_flatgeobuf(&p, None).unwrap().geometry.len(), 2);
    }
    let (p, _) = small_fgb(&dir, "i.fgb", true);
    let hit = read_flatgeobuf(&p, Some([0.0, 0.0, 1.5, 2.5])).unwrap();
    assert_eq!(hit.geometry.len(), 1);
}

#[test]
fn flatgeobuf_bbox_with_a_non_default_index_node_size_is_rejected() {
    // A header the crate would accept, but with index node size 8 (its bbox search assumes 16).
    let mut fbb = flatbuffers::FlatBufferBuilder::new();
    let name = fbb.create_string("t");
    let header = flatgeobuf::Header::create(
        &mut fbb,
        &flatgeobuf::HeaderArgs {
            name: Some(name),
            geometry_type: flatgeobuf::GeometryType::Point,
            features_count: 1,
            index_node_size: 8,
            ..Default::default()
        },
    );
    fbb.finish_size_prefixed(header, None);
    let mut file = vec![b'f', b'g', b'b', 3, b'f', b'g', b'b', 0];
    file.extend_from_slice(fbb.finished_data());
    file.extend_from_slice(&[0u8; 2 * 40 + 8]); // two index nodes, then filler
    let dir = TempDir::new().unwrap();
    let bad = path(&dir, "node8.fgb");
    std::fs::write(&bad, file).unwrap();
    let err = read_flatgeobuf(&bad, Some([0.0, 0.0, 10.0, 10.0]))
        .err()
        .expect("error");
    assert!(err.contains("index node size 8"), "{err}");
}

// -- 4. make_grid ---------------------------------------------------------------------------

fn extent(max: f64) -> GeoColumn {
    col(&[Some(&format!(
        "POLYGON ((0 0, {max} 0, {max} {max}, 0 {max}, 0 0))"
    ))])
}

#[test]
fn square_grid_above_the_cell_cap_is_rejected() {
    let err = extent(3300.0)
        .make_grid(
            1.0,
            GridType::Square,
            GridFeature::Centers,
            None,
            false,
            true,
        )
        .unwrap_err();
    assert!(err.contains("grid too large"), "{err}");
    assert!(err.contains("max 10_000_000"), "{err}");
}

#[test]
fn hexagon_grid_above_the_cell_cap_is_rejected() {
    let err = extent(3000.0)
        .make_grid(
            0.5,
            GridType::Hexagon,
            GridFeature::Centers,
            None,
            false,
            true,
        )
        .unwrap_err();
    assert!(err.contains("grid too large"), "{err}");
}

#[test]
fn grid_cell_count_overflow_is_an_error_not_an_abort() {
    let s = col(&[Some("LINESTRING (0 0, 1e300 1e300)")]);
    for grid in [GridType::Square, GridType::Hexagon] {
        let err = s
            .make_grid(1e-300, grid, GridFeature::Polygons, None, false, true)
            .unwrap_err();
        assert!(err.contains("grid too large"), "{err}");
    }
}

#[test]
fn grid_below_the_cap_is_unchanged() {
    let out = extent(10.0)
        .make_grid(
            1.0,
            GridType::Square,
            GridFeature::Polygons,
            None,
            false,
            true,
        )
        .unwrap();
    assert_eq!(out.len(), 100);
}

// -- 5. sample_points -----------------------------------------------------------------------

#[test]
fn sample_points_over_the_total_cap_is_rejected() {
    let s = col(&[Some("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))")]);
    let err = s.sample_points(&[10_000_001], Some(1)).unwrap_err();
    assert!(err.contains("exceeds the maximum of 10000000"), "{err}");
    // A broadcast size multiplies by the row count.
    let two = col(&[Some("POINT (0 0)"), Some("POINT (1 1)")]);
    assert!(two.sample_points(&[6_000_000], None).is_err());
    assert!(s.sample_points(&[usize::MAX], None).is_err());
    assert!(two.sample_points(&[usize::MAX, usize::MAX], None).is_err());
}

#[test]
fn sample_points_within_the_cap_still_works() {
    let s = col(&[Some("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))")]);
    assert_eq!(s.sample_points(&[100], Some(1)).unwrap().len(), 1);
}

// -- 6. maximum_inscribed_circle ------------------------------------------------------------

#[test]
fn inscribed_circle_of_a_very_thin_polygon_still_returns_a_result() {
    let s = col(&[Some("POLYGON ((0 0, 12000000 0, 12000000 1, 0 1, 0 0))")]);
    let out = s.maximum_inscribed_circle(Some(0.4)).unwrap();
    let wkt = out.to_wkt().unwrap();
    assert!(
        wkt[0].as_deref().unwrap().starts_with("LINESTRING"),
        "{wkt:?}"
    );
}

// -- 7. segmentize / densify ----------------------------------------------------------------

#[test]
fn segmentize_over_the_vertex_cap_is_rejected() {
    let s = col(&[Some("LINESTRING (0 0, 20000000 0)")]);
    let err = s.segmentize(1.0).unwrap_err();
    assert!(err.contains("segmentize would create"), "{err}");
    assert!(s.segmentize(1e-300).is_err());
}

#[test]
fn segmentize_within_the_cap_still_works() {
    let s = col(&[Some("LINESTRING (0 0, 10 0)")]);
    assert_eq!(s.segmentize(2.0).unwrap().len(), 1);
}

#[test]
fn densify_fraction_below_the_piece_cap_is_rejected() {
    let a = col(&[Some("LINESTRING (0 0, 1 0, 2 1)")]);
    let err = a.hausdorff_distance(&a, Some(1e-5)).unwrap_err();
    assert!(err.contains("densify must be in"), "{err}");
    assert!(a.frechet_distance(&a, Some(1e-5)).is_err());
    assert!(a.hausdorff_distance(&a, Some(1e-4)).is_ok());
}

#[test]
fn densify_total_vertices_over_the_cap_is_rejected() {
    let coords: Vec<String> = (0..2000).map(|i| format!("{i} 0")).collect();
    let big = col(&[Some(&format!("LINESTRING ({})", coords.join(", ")))]);
    assert!(big.hausdorff_distance(&big, Some(1e-4)).is_err());
}

// -- union_groups ---------------------------------------------------------------------------

#[test]
fn union_groups_rejects_oversized_group_counts_and_ids_before_allocating() {
    let s = col(&[Some("POINT (0 0)"), Some("POINT (1 1)")]);
    let err = s.union_groups(&[0, 1], usize::MAX).unwrap_err();
    assert!(err.contains("cannot exceed"), "{err}");
    assert!(s.union_groups(&[0, 1], 3).is_err());
    let err = s.union_groups(&[0, 2], 2).unwrap_err();
    assert!(err.contains("out of range"), "{err}");
}

// -- 8. atomic writes -----------------------------------------------------------------------

fn leftovers(dir: &TempDir) -> Vec<String> {
    std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect()
}

#[test]
fn a_failed_write_keeps_the_existing_file_and_removes_only_the_temp_file() {
    use std::io::Write;
    let dir = TempDir::new().unwrap();
    let target = path(&dir, "out.bin");
    std::fs::write(&target, b"original").unwrap();
    let err = crate::atomic::write_atomic(&target, |mut file| {
        file.write_all(b"partial").unwrap();
        Err("encoding failed".to_string())
    })
    .unwrap_err();
    assert_eq!(err, "encoding failed");
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert!(leftovers(&dir).is_empty());
}

#[test]
fn a_successful_write_replaces_the_file() {
    use std::io::Write;
    let dir = TempDir::new().unwrap();
    let target = path(&dir, "out.bin");
    std::fs::write(&target, b"original").unwrap();
    crate::atomic::write_atomic(&target, |mut file| {
        file.write_all(b"new").map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(leftovers(&dir).is_empty());
}

#[test]
fn every_writer_leaves_an_unreplaceable_destination_untouched() {
    use crate::{
        write_feather, write_geojson, write_geoparquet, FeatherCompression, GeometryEncoding,
    };
    let geometry = col(&[Some("POINT (1 2)")]);
    let attrs = ipc(vec![(
        "id",
        std::sync::Arc::new(arrow_array::Int64Array::from(vec![1])) as arrow_array::ArrayRef,
    )]);
    // A non-empty directory cannot be replaced by a file, so the final rename fails.
    let dir = TempDir::new().unwrap();
    for name in ["a.geojson", "a.parquet", "a.feather", "a.fgb"] {
        let target = path(&dir, name);
        std::fs::create_dir(&target).unwrap();
        std::fs::write(format!("{target}/keep"), b"x").unwrap();
    }
    let results = [
        write_geojson(&path(&dir, "a.geojson"), &geometry, &attrs, true),
        write_geoparquet(&path(&dir, "a.parquet"), &geometry, "geometry", &attrs),
        write_feather(
            &path(&dir, "a.feather"),
            &geometry,
            "geometry",
            &attrs,
            GeometryEncoding::Wkb,
            FeatherCompression::Uncompressed,
        ),
        write_flatgeobuf(&path(&dir, "a.fgb"), &geometry, &attrs, "t", false),
    ];
    for result in results {
        assert!(result.is_err());
    }
    for name in ["a.geojson", "a.parquet", "a.feather", "a.fgb"] {
        assert!(std::path::Path::new(&path(&dir, name))
            .join("keep")
            .exists());
    }
    assert!(leftovers(&dir).is_empty());
}

#[test]
fn every_writer_replaces_an_existing_file() {
    use crate::{
        write_feather, write_geojson, write_geoparquet, FeatherCompression, GeometryEncoding,
    };
    let geometry = col(&[Some("POINT (1 2)")]);
    let attrs = ipc(vec![(
        "id",
        std::sync::Arc::new(arrow_array::Int64Array::from(vec![1])) as arrow_array::ArrayRef,
    )]);
    let dir = TempDir::new().unwrap();
    for name in ["b.geojson", "b.parquet", "b.feather", "b.fgb"] {
        std::fs::write(path(&dir, name), b"old").unwrap();
    }
    write_geojson(&path(&dir, "b.geojson"), &geometry, &attrs, true).unwrap();
    write_geoparquet(&path(&dir, "b.parquet"), &geometry, "geometry", &attrs).unwrap();
    write_feather(
        &path(&dir, "b.feather"),
        &geometry,
        "geometry",
        &attrs,
        GeometryEncoding::Wkb,
        FeatherCompression::Uncompressed,
    )
    .unwrap();
    write_flatgeobuf(&path(&dir, "b.fgb"), &geometry, &attrs, "t", false).unwrap();
    for name in ["b.geojson", "b.parquet", "b.feather", "b.fgb"] {
        assert_ne!(std::fs::read(path(&dir, name)).unwrap(), b"old");
    }
    assert!(leftovers(&dir).is_empty());
}

// -- 9. CRS equivalence ---------------------------------------------------------------------

#[test]
fn crs_equivalence_ignores_case_and_treats_crs84_as_epsg_4326() {
    use crate::crs_equivalent;
    assert!(crs_equivalent("epsg:4326", "EPSG:4326"));
    assert!(crs_equivalent("OGC:CRS84", "EPSG:4326"));
    assert!(crs_equivalent("ogc:crs84", "epsg:4326"));
    assert!(crs_equivalent("EPSG:3857", "epsg:3857"));
    assert!(!crs_equivalent("EPSG:4326", "EPSG:3857"));
    assert!(!crs_equivalent("OGC:CRS84", "EPSG:3857"));
    // Unresolvable labels fall back to a case-insensitive comparison.
    assert!(crs_equivalent("custom:1", "CUSTOM:1"));
    assert!(!crs_equivalent("custom:1", "custom:2"));
}

#[test]
fn binary_ops_and_concat_accept_equivalent_crs_labels() {
    let a = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("OGC:CRS84")).unwrap();
    let b = GeoColumn::from_wkt(
        &[Some("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))")],
        Some("EPSG:4326"),
    )
    .unwrap();
    assert_eq!(
        b.predicate(&a, crate::Predicate::Intersects).unwrap(),
        vec![Some(true)]
    );
    let c = GeoColumn::concat(&[&a, &b]).unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(
        c.crs().as_deref(),
        Some("OGC:CRS84"),
        "labels stay as stored"
    );
    let other = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:3857")).unwrap();
    assert!(b.predicate(&other, crate::Predicate::Intersects).is_err());
    assert!(GeoColumn::concat(&[&a, &other]).is_err());
}

// -- 10. point dedup is not quadratic -------------------------------------------------------

fn multipoint(points: impl Iterator<Item = (i64, i64)>) -> String {
    let body: Vec<String> = points.map(|(x, y)| format!("({x} {y})")).collect();
    format!("MULTIPOINT ({})", body.join(", "))
}

#[test]
fn point_union_dedups_in_linear_time_and_keeps_first_seen_order() {
    let n = 150_000i64;
    // Every point twice (second half repeats the first), so dedup has real work to do.
    let a = col(&[Some(&multipoint((0..n).map(|i| (i, -i))))]);
    let b = col(&[Some(&multipoint((0..n).map(|i| (n - 1 - i, -(n - 1 - i)))))]);
    let started = std::time::Instant::now();
    let out = a.set_op(&b, crate::SetOp::Union).unwrap();
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    let wkt = out.to_wkt().unwrap()[0].clone().unwrap();
    assert_eq!(wkt.matches(',').count() + 1, n as usize);
    assert!(
        wkt.starts_with("MULTIPOINT((0 0),(1 -1),(2 -2)")
            || wkt.starts_with("MULTIPOINT (0 0, 1 -1, 2 -2"),
        "{}",
        &wkt[..60]
    );
}

#[test]
fn point_dedup_treats_negative_zero_as_zero() {
    let a = col(&[Some("MULTIPOINT ((0 0), (1 1))")]);
    let b = col(&[Some("MULTIPOINT ((-0 -0), (2 2))")]);
    let wkt = a.set_op(&b, crate::SetOp::Union).unwrap().to_wkt().unwrap()[0]
        .clone()
        .unwrap();
    assert_eq!(wkt.matches(',').count() + 1, 3, "{wkt}");
}
