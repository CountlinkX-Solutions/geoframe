//! Z/M dimension support: predicates, ordinate accessors, dimension forcing and the storage
//! round-trips that must not lose a Z or M ordinate.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array};
use tempfile::TempDir;

use crate::test_support::{ipc, path, wkt};
use crate::{
    from_arrow_ipc, read_geoparquet, read_gpkg, to_arrow_ipc, write_geoparquet, write_gpkg,
    GeoColumn, GeometryEncoding, GpkgWriteOptions,
};

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

/// One row of every dimension and family, a 2D control and a null.
const MIXED: [Option<&str>; 13] = [
    Some("POINT Z (1 2 3)"),
    Some("POINT M (1 2 4)"),
    Some("POINT ZM (1 2 3 4)"),
    Some("POINT (1 2)"),
    Some("LINESTRING Z (0 0 1, 1 1 2)"),
    Some("LINESTRING M (0 0 1, 1 1 2)"),
    Some("POLYGON ZM ((0 0 1 5, 4 0 2 6, 4 4 3 7, 0 0 1 5))"),
    Some("MULTIPOINT Z ((0 0 1), (1 1 2))"),
    Some("MULTILINESTRING M ((0 0 1, 1 1 2), (2 2 3, 3 3 4))"),
    Some("MULTIPOLYGON Z (((0 0 1, 4 0 1, 4 4 1, 0 0 1)))"),
    Some("GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))"),
    Some("LINESTRING (0 0, 1 1)"),
    None,
];

fn mixed() -> GeoColumn {
    col(&MIXED)
}

// -- has_z / has_m ----------------------------------------------------------------------

#[test]
fn has_z_and_has_m_report_the_stored_dimension() {
    let c = mixed();
    assert_eq!(
        c.has_z(),
        [true, false, true, false, true, false, true, true, false, true, true, false, false]
    );
    assert_eq!(
        c.has_m(),
        [false, true, true, false, false, true, true, false, true, false, false, false, false]
    );
}

#[test]
fn empty_geometries_keep_their_declared_dimension() {
    let c = col(&[
        Some("POINT Z EMPTY"),
        Some("POINT M EMPTY"),
        Some("POINT EMPTY"),
        Some("LINESTRING Z EMPTY"),
        Some("POLYGON M EMPTY"),
    ]);
    assert_eq!(c.has_z(), [true, false, false, true, false]);
    assert_eq!(c.has_m(), [false, true, false, false, true]);
}

#[test]
fn a_collection_with_mixed_member_dimensions_is_an_error_not_a_panic() {
    // The GeoArrow builder cannot hold members of different dimensions in one collection.
    let text = "GEOMETRYCOLLECTION (POINT (1 2), POINT Z (1 2 3))";
    let err = GeoColumn::from_wkt(&[Some(text)], None).unwrap_err();
    assert!(err.contains("dimension"), "{err}");
    let wkb = col(&[Some("POINT Z (1 2 3)")]).to_wkb().unwrap();
    assert!(GeoColumn::from_wkb(&wkb, None).is_ok());
}

// -- z / m ------------------------------------------------------------------------------

#[test]
fn z_and_m_read_point_ordinates_only() {
    let c = mixed();
    let z = c.z();
    let m = c.m();
    assert_eq!(z[0], Some(3.0));
    assert_eq!(z[1], None);
    assert_eq!(z[2], Some(3.0));
    assert_eq!(z[3], None);
    assert_eq!(m[0], None);
    assert_eq!(m[1], Some(4.0));
    assert_eq!(m[2], Some(4.0));
    // non-points and nulls
    assert!(z[4..].iter().all(Option::is_none));
    assert!(m[4..].iter().all(Option::is_none));
}

#[test]
fn z_of_an_empty_point_is_null() {
    let c = col(&[Some("POINT Z EMPTY"), Some("POINT M EMPTY")]);
    assert_eq!(c.z(), [None, None]);
    assert_eq!(c.m(), [None, None]);
}

#[test]
fn nan_ordinates_are_null() {
    // a missing z is NaN
    let c = col(&[Some("POINT (1 2)")]).force_3d(&[None]).unwrap();
    assert_eq!(c.has_z(), [true]);
    assert_eq!(c.z(), [None]);
}

// -- force_2d ---------------------------------------------------------------------------

#[test]
fn force_2d_drops_z_and_m_and_keeps_nulls_and_crs() {
    let c = GeoColumn::from_wkt(&MIXED, Some("EPSG:4326")).unwrap();
    let flat = c.force_2d().unwrap();
    assert_eq!(flat.crs().as_deref(), Some("EPSG:4326"));
    assert!(flat.has_z().iter().all(|&b| !b));
    assert!(flat.has_m().iter().all(|&b| !b));
    let expected = col(&[
        Some("POINT (1 2)"),
        Some("POINT (1 2)"),
        Some("POINT (1 2)"),
        Some("POINT (1 2)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("POLYGON ((0 0, 4 0, 4 4, 0 0))"),
        Some("MULTIPOINT ((0 0), (1 1))"),
        Some("MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))"),
        Some("MULTIPOLYGON (((0 0, 4 0, 4 4, 0 0)))"),
        Some("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))"),
        Some("LINESTRING (0 0, 1 1)"),
        None,
    ]);
    assert_eq!(wkt(&flat), wkt(&expected));
}

#[test]
fn force_2d_keeps_empties_empty() {
    let c = col(&[Some("POINT Z EMPTY"), Some("LINESTRING M EMPTY")]);
    let flat = c.force_2d().unwrap();
    assert_eq!(flat.has_z(), [false, false]);
    assert_eq!(flat.has_m(), [false, false]);
    assert_eq!(flat.is_empty_rows(), [Some(true), Some(true)]);
}

// -- force_3d ---------------------------------------------------------------------------

#[test]
fn force_3d_adds_z_to_2d_and_keeps_an_existing_z() {
    let c = col(&[
        Some("POINT (1 2)"),
        Some("POINT Z (1 2 3)"),
        Some("LINESTRING (0 0, 1 1)"),
        Some("POLYGON ((0 0, 4 0, 4 4, 0 0))"),
        None,
    ]);
    let out = c.force_3d(&[Some(9.0)]).unwrap();
    let expected = col(&[
        Some("POINT Z (1 2 9)"),
        Some("POINT Z (1 2 3)"),
        Some("LINESTRING Z (0 0 9, 1 1 9)"),
        Some("POLYGON Z ((0 0 9, 4 0 9, 4 4 9, 0 0 9))"),
        None,
    ]);
    assert_eq!(wkt(&out), wkt(&expected));
}

#[test]
fn force_3d_takes_one_z_per_row() {
    let c = col(&[Some("POINT (1 2)"), Some("POINT (3 4)"), None]);
    let out = c.force_3d(&[Some(5.0), Some(6.0), Some(7.0)]).unwrap();
    assert_eq!(out.z(), [Some(5.0), Some(6.0), None]);
}

#[test]
fn force_3d_rejects_a_mismatched_z_length() {
    let c = col(&[Some("POINT (1 2)"), Some("POINT (3 4)")]);
    let err = c.force_3d(&[Some(1.0), Some(2.0), Some(3.0)]).unwrap_err();
    assert!(err.contains("length"), "{err}");
}

#[test]
fn force_3d_of_xyzm_drops_m_and_xym_is_left_alone() {
    // Shapely 2.2 leaves a measured geometry without Z as it is.
    let c = col(&[Some("POINT ZM (1 2 3 4)"), Some("POINT M (1 2 4)")]);
    let out = c.force_3d(&[Some(9.0)]).unwrap();
    assert_eq!(out.has_z(), [true, false]);
    assert_eq!(out.has_m(), [false, true]);
    assert_eq!(out.z()[0], Some(3.0));
}

#[test]
fn force_3d_makes_empties_three_dimensional() {
    let c = col(&[Some("POINT EMPTY"), Some("LINESTRING EMPTY")]);
    let out = c.force_3d(&[Some(1.0)]).unwrap();
    assert_eq!(out.has_z(), [true, true]);
    assert_eq!(out.is_empty_rows(), [Some(true), Some(true)]);
}

// -- storage round-trips ----------------------------------------------------------------

#[test]
fn wkb_round_trip_keeps_z_and_m() {
    let c = mixed();
    let back = GeoColumn::from_wkb(&c.to_wkb().unwrap(), None).unwrap();
    assert_eq!(wkt(&back), wkt(&c));
    assert_eq!(back.has_z(), c.has_z());
    assert_eq!(back.has_m(), c.has_m());
}

#[test]
fn wkb_is_iso_for_z_and_m() {
    let c = col(&[
        Some("POINT Z (1 2 3)"),
        Some("POINT M (1 2 4)"),
        Some("POINT ZM (1 2 3 4)"),
    ]);
    let codes: Vec<u32> = c
        .to_wkb()
        .unwrap()
        .iter()
        .map(|b| u32::from_le_bytes(b.as_ref().unwrap()[1..5].try_into().unwrap()))
        .collect();
    assert_eq!(codes, [1001, 2001, 3001]);
}

#[test]
fn wkt_round_trip_keeps_z_and_m() {
    let c = mixed();
    let text = c.to_wkt().unwrap();
    let back = GeoColumn::from_wkt(&text, None).unwrap();
    assert_eq!(wkt(&back), text);
    assert_eq!(back.has_z(), c.has_z());
    assert_eq!(back.has_m(), c.has_m());
    assert!(text[0].as_ref().unwrap().starts_with("POINT Z"));
    assert!(text[1].as_ref().unwrap().starts_with("POINT M"));
    assert!(text[2].as_ref().unwrap().starts_with("POINT ZM"));
}

#[test]
fn ewkb_round_trip_keeps_z_and_m() {
    let c = mixed();
    let ewkb = c.to_ewkb(4326).unwrap();
    // PostGIS flag bits instead of the ISO offsets
    let code = |i: usize| u32::from_le_bytes(ewkb[i].as_ref().unwrap()[1..5].try_into().unwrap());
    assert_eq!(code(0) & 0xE000_0000, 0xA000_0000); // Z + SRID
    assert_eq!(code(1) & 0xE000_0000, 0x6000_0000); // M + SRID
    assert_eq!(code(2) & 0xE000_0000, 0xE000_0000); // Z + M + SRID
    let back = GeoColumn::from_wkb(&ewkb, None).unwrap();
    assert_eq!(back.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(wkt(&back), wkt(&c));
    assert_eq!(back.has_m(), c.has_m());
}

fn ids(n: usize) -> Vec<u8> {
    let values: Vec<i64> = (0..n as i64).collect();
    ipc(vec![("id", Arc::new(Int64Array::from(values)) as ArrayRef)])
}

#[test]
fn geoparquet_round_trip_keeps_z_and_m() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "zm.parquet");
    let c = GeoColumn::from_wkt(&MIXED, Some("EPSG:4326")).unwrap();
    write_geoparquet(&file, &c, "geometry", &ids(MIXED.len())).unwrap();
    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&c));
    assert_eq!(read.geometry.has_z(), c.has_z());
    assert_eq!(read.geometry.has_m(), c.has_m());
}

#[test]
fn arrow_ipc_wkb_round_trip_keeps_z_and_m() {
    let c = mixed();
    let bytes = to_arrow_ipc(&c, "geometry", &ids(MIXED.len()), GeometryEncoding::Wkb).unwrap();
    let back = from_arrow_ipc(&bytes, None, None).unwrap();
    assert_eq!(wkt(&back.geometry), wkt(&c));
    assert_eq!(back.geometry.has_m(), c.has_m());
}

#[test]
fn arrow_ipc_native_round_trip_keeps_z_and_m() {
    for rows in [
        [Some("POINT Z (1 2 3)"), None, Some("POINT Z (4 5 6)")],
        [Some("POINT M (1 2 3)"), None, Some("POINT M (4 5 6)")],
        [Some("POINT ZM (1 2 3 4)"), None, Some("POINT ZM (5 6 7 8)")],
    ] {
        let c = col(&rows);
        let bytes = to_arrow_ipc(&c, "geometry", &ids(3), GeometryEncoding::GeoArrow).unwrap();
        let back = from_arrow_ipc(&bytes, None, None).unwrap();
        assert_eq!(wkt(&back.geometry), wkt(&c), "{rows:?}");
        assert_eq!(back.geometry.has_z(), c.has_z());
        assert_eq!(back.geometry.has_m(), c.has_m());
    }
}

#[test]
fn gpkg_round_trip_keeps_z_and_m() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "zm.gpkg");
    let c = GeoColumn::from_wkt(&MIXED, Some("EPSG:4326")).unwrap();
    let options = GpkgWriteOptions {
        append: false,
        spatial_index: true,
    };
    write_gpkg(&file, &c, &ids(MIXED.len()), "zm", options).unwrap();
    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(wkt(&read.vector.geometry), wkt(&c));
    assert_eq!(read.vector.geometry.has_z(), c.has_z());
    assert_eq!(read.vector.geometry.has_m(), c.has_m());
}

#[test]
fn take_concat_and_fill_null_keep_z_and_m() {
    let c = mixed();
    let taken = c.take(&[2, 0, 1, 12, 6]).unwrap();
    assert_eq!(taken.has_z(), [true, true, false, false, true]);
    assert_eq!(taken.has_m(), [true, false, true, false, true]);
    assert_eq!(taken.z()[0], Some(3.0));

    let both = GeoColumn::concat(&[&c, &taken]).unwrap();
    assert_eq!(both.len(), c.len() + taken.len());
    assert_eq!(both.has_z()[13..], taken.has_z()[..]);
    assert_eq!(both.has_m()[13..], taken.has_m()[..]);

    let fill = col(&[Some("POINT ZM (9 9 8 7)")]);
    let filled = c.fill_null(&fill).unwrap();
    assert!(filled.has_z()[12]);
    assert!(filled.has_m()[12]);
    assert_eq!(filled.z()[12], Some(8.0));
    assert_eq!(filled.m()[12], Some(7.0));
}
