//! EWKB parse and emit: SRID flag, Z/M flags, nesting, byte order, round trips.

use crate::ewkb::{ewkb_to_iso, wkb_to_ewkb};
use crate::GeoColumn;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// `SELECT ST_AsEWKB(ST_SetSRID(ST_MakePoint(1, 2), 4326))`
const POINT_SRID: &str = "0101000020E6100000000000000000F03F0000000000000040";
const POINT_PLAIN: &str = "0101000000000000000000F03F0000000000000040";

#[test]
fn srid_is_stripped_and_returned() {
    let (iso, srid) = ewkb_to_iso(&hex(POINT_SRID)).unwrap();
    assert_eq!(srid, Some(4326));
    assert_eq!(iso, hex(POINT_PLAIN));
}

#[test]
fn plain_wkb_passes_through_without_srid() {
    let (iso, srid) = ewkb_to_iso(&hex(POINT_PLAIN)).unwrap();
    assert_eq!(srid, None);
    assert_eq!(iso, hex(POINT_PLAIN));
}

#[test]
fn z_and_m_flags_become_iso_offsets() {
    // POINT Z (1 2 3), EWKB Z flag, little endian
    let mut z = vec![1u8, 1, 0, 0, 0x80];
    for v in [1.0f64, 2.0, 3.0] {
        z.extend_from_slice(&v.to_le_bytes());
    }
    let (iso, _) = ewkb_to_iso(&z).unwrap();
    assert_eq!(&iso[..5], &[1, 0xE9, 3, 0, 0]); // 1001
    assert_eq!(iso.len(), z.len());

    let mut m = vec![1u8, 1, 0, 0, 0x40];
    for v in [1.0f64, 2.0, 3.0] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(&ewkb_to_iso(&m).unwrap().0[..5], &[1, 0xD1, 7, 0, 0]); // 2001

    let mut zm = vec![1u8, 1, 0, 0, 0xC0];
    for v in [1.0f64, 2.0, 3.0, 4.0] {
        zm.extend_from_slice(&v.to_le_bytes());
    }
    let (iso, _) = ewkb_to_iso(&zm).unwrap();
    assert_eq!(&iso[..5], &[1, 0xB9, 0xB, 0, 0]); // 3001
    assert_eq!(iso.len(), zm.len());
}

#[test]
fn big_endian_srid_and_nested_collections() {
    // MULTIPOINT with SRID 3857, big endian, members without flags
    let mut b = vec![0u8, 0x20, 0, 0, 4];
    b.extend_from_slice(&3857i32.to_be_bytes());
    b.extend_from_slice(&1u32.to_be_bytes());
    b.extend_from_slice(&[0, 0, 0, 0, 1]);
    b.extend_from_slice(&5.0f64.to_be_bytes());
    b.extend_from_slice(&6.0f64.to_be_bytes());
    let (iso, srid) = ewkb_to_iso(&b).unwrap();
    assert_eq!(srid, Some(3857));
    assert_eq!(iso.len(), b.len() - 4);
    assert_eq!(&iso[..5], &[0, 0, 0, 0, 4]);
}

#[test]
fn emit_sets_the_srid_flag_and_round_trips() {
    let ewkb = wkb_to_ewkb(&hex(POINT_PLAIN), 4326).unwrap();
    assert_eq!(ewkb, hex(POINT_SRID));
    let (back, srid) = ewkb_to_iso(&ewkb).unwrap();
    assert_eq!((back, srid), (hex(POINT_PLAIN), Some(4326)));
}

#[test]
fn emit_converts_iso_dimensions_to_flags() {
    let mut iso = vec![1u8, 0xE9, 3, 0, 0];
    for v in [1.0f64, 2.0, 3.0] {
        iso.extend_from_slice(&v.to_le_bytes());
    }
    let ewkb = wkb_to_ewkb(&iso, 4326).unwrap();
    assert_eq!(&ewkb[..5], &[1, 1, 0, 0, 0xA0]);
    assert_eq!(&ewkb[5..9], &4326i32.to_le_bytes());
}

#[test]
fn truncated_and_bad_input_is_rejected() {
    let full = hex(POINT_SRID);
    assert!(ewkb_to_iso(&full[..full.len() - 1]).is_err());
    assert!(ewkb_to_iso(&[]).is_err());
    assert!(ewkb_to_iso(&[7, 1, 0, 0, 0]).is_err());
    assert!(wkb_to_ewkb(&[1, 99, 0, 0, 0], 1).is_err());
}

#[test]
fn column_infers_crs_from_the_srid_and_emits_ewkb() {
    let rows = [Some(hex(POINT_SRID)), None];
    let column = GeoColumn::from_wkb(&rows, None).unwrap();
    assert_eq!(column.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(column.len(), 2);

    // An explicit CRS wins over the embedded SRID.
    let column = GeoColumn::from_wkb(&rows, Some("EPSG:3857")).unwrap();
    assert_eq!(column.crs().as_deref(), Some("EPSG:3857"));

    let out = column.to_ewkb(4326).unwrap();
    assert_eq!(out[0].as_deref(), Some(hex(POINT_SRID).as_slice()));
    assert!(out[1].is_none());
}

#[test]
fn mixed_srids_are_rejected_without_an_override() {
    let other = wkb_to_ewkb(&hex(POINT_PLAIN), 3857).unwrap();
    let rows = [Some(hex(POINT_SRID)), Some(other)];
    let err = GeoColumn::from_wkb(&rows, None).err().unwrap();
    assert!(err.contains("mixed SRIDs"), "{err}");
    assert!(GeoColumn::from_wkb(&rows, Some("EPSG:4326")).is_ok());
}

#[test]
fn z_ewkb_reads_through_the_column() {
    let mut z = vec![1u8, 1, 0, 0, 0xA0];
    z.extend_from_slice(&4326i32.to_le_bytes());
    for v in [1.0f64, 2.0, 3.0] {
        z.extend_from_slice(&v.to_le_bytes());
    }
    let column = GeoColumn::from_wkb(&[Some(z)], None).unwrap();
    assert_eq!(column.crs().as_deref(), Some("EPSG:4326"));
}
