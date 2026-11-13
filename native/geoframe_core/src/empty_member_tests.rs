//! A `MULTIPOINT` (or a collection) with an empty member point, as GEOS 3.14 (Shapely 2.2)
//! reads and writes it; probed 2026-11-13. GEOS writes the member as `EMPTY` in WKT and as a
//! NaN point in WKB, which is also how GeoArrow stores it.

use crate::GeoColumn;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn wkt_of_wkb(wkb_hex: &str) -> Result<String, String> {
    let bytes = hex(wkb_hex);
    let column = GeoColumn::from_wkb(&[Some(bytes.as_slice())], None)?;
    Ok(column.to_wkt()?[0].clone().unwrap())
}

fn read(text: &str) -> Result<String, String> {
    let column = GeoColumn::from_wkt(&[Some(text)], None)?;
    Ok(column.to_wkt()?[0].clone().unwrap())
}

// Shapely's ISO WKB of each geometry, and the WKT it writes (in this crate's spacing).
const SHAPELY_WKB: [(&str, &str); 5] = [
    (
        "0104000000020000000101000000000000000000F87F000000000000F87F0101000000000000000000F03F\
         000000000000F03F",
        "MULTIPOINT(EMPTY,(1 1))",
    ),
    (
        "0107000000020000000104000000020000000101000000000000000000F87F000000000000F87F01010000\
         00000000000000F03F000000000000F03F010100000000000000000008400000000000000840",
        "GEOMETRYCOLLECTION(MULTIPOINT(EMPTY,(1 1)),POINT(3 3))",
    ),
    (
        "01EC0300000200000001E9030000000000000000F87F000000000000F87F000000000000F87F01E9030000\
         000000000000F03F000000000000F03F000000000000F03F",
        "MULTIPOINT Z(EMPTY,(1 1 1))",
    ),
    (
        "01D40700000100000001D1070000000000000000F87F000000000000F87F000000000000F87F",
        "MULTIPOINT M(EMPTY)",
    ),
    (
        "01BC0B00000200000001B90B0000000000000000F03F00000000000000400000000000000840000000000000\
         104001B90B0000000000000000F87F000000000000F87F000000000000F87F000000000000F87F",
        "MULTIPOINT ZM((1 2 3 4),EMPTY)",
    ),
];

#[test]
fn an_empty_member_point_is_written_as_empty_as_geos_does() {
    for (wkb_hex, expected) in SHAPELY_WKB {
        assert_eq!(wkt_of_wkb(wkb_hex).as_deref(), Ok(expected), "{expected}");
    }
}

#[test]
fn rows_without_an_empty_member_keep_the_crate_output() {
    let bytes = hex(SHAPELY_WKB[0].0);
    let column = GeoColumn::from_wkb(
        &[
            Some(hex("0101000000000000000000F03F0000000000000040").as_slice()),
            Some(bytes.as_slice()),
            None,
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        column.to_wkt().unwrap(),
        vec![
            Some("POINT(1 2)".to_string()),
            Some("MULTIPOINT(EMPTY,(1 1))".to_string()),
            None
        ]
    );
}

#[test]
fn a_multipoint_with_an_empty_member_is_read_from_wkt_as_geos_reads_it() {
    for (text, expected) in [
        ("MULTIPOINT (EMPTY, (1 1))", "MULTIPOINT(EMPTY,(1 1))"),
        ("multipoint ( empty ,(1 1))", "MULTIPOINT(EMPTY,(1 1))"),
        (
            "MULTIPOINT ((1 1), EMPTY, (2 2))",
            "MULTIPOINT((1 1),EMPTY,(2 2))",
        ),
        ("MULTIPOINT (EMPTY)", "MULTIPOINT(EMPTY)"),
        ("MULTIPOINT Z (EMPTY)", "MULTIPOINT Z(EMPTY)"),
        (
            "MULTIPOINT Z (EMPTY, (1 1 1))",
            "MULTIPOINT Z(EMPTY,(1 1 1))",
        ),
        ("MULTIPOINT (EMPTY, (1 1 1))", "MULTIPOINT Z(EMPTY,(1 1 1))"),
        (
            "MULTIPOINT (EMPTY, (1 1 1 1))",
            "MULTIPOINT ZM(EMPTY,(1 1 1 1))",
        ),
        (
            "MULTIPOINT M (EMPTY, (1 1 1))",
            "MULTIPOINT M(EMPTY,(1 1 1))",
        ),
        (
            "GEOMETRYCOLLECTION (MULTIPOINT (EMPTY, (1 1)), POINT (3 3))",
            "GEOMETRYCOLLECTION(MULTIPOINT(EMPTY,(1 1)),POINT(3 3))",
        ),
    ] {
        assert_eq!(read(text).as_deref(), Ok(expected), "{text}");
    }
    for (wkb_hex, expected) in SHAPELY_WKB {
        assert_eq!(read(expected), wkt_of_wkb(wkb_hex), "{expected}");
    }
}

#[test]
fn forms_geos_rejects_stay_errors() {
    for text in [
        "MULTIPOINT (EMPTY, 1 1)",
        "MULTIPOINT (1 1, EMPTY)",
        "MULTIPOINT (EMPTY EMPTY)",
        "MULTIPOINT ((EMPTY))",
    ] {
        assert!(read(text).is_err(), "{text}: {:?}", read(text));
    }
}

// GEOS writes an empty member line and an empty ring as `EMPTY`; the crate writes `()`,
// which no WKT reader accepts (Shapely's WKB of each, then its WKT).
#[test]
fn an_empty_member_line_or_ring_is_written_as_empty_as_geos_does() {
    for (wkb_hex, expected) in [
        (
            "01050000000200000001020000000200000000000000000000000000000000000000000000000000F03F\
             000000000000F03F010200000000000000",
            "MULTILINESTRING((0 0,1 1),EMPTY)",
        ),
        ("010500000001000000010200000000000000", "MULTILINESTRING(EMPTY)"),
        (
            "0103000000020000000400000000000000000000000000000000000000000000000000224000000000000000\
             00000000000000224000000000000022400000000000000000000000000000000000000000",
            "POLYGON((0 0,9 0,9 9,0 0),EMPTY)",
        ),
    ] {
        assert_eq!(wkt_of_wkb(wkb_hex).as_deref(), Ok(expected), "{expected}");
        assert_eq!(read(expected).as_deref(), Ok(expected), "{expected} reads back");
    }
    assert_eq!(
        read("GEOMETRYCOLLECTION (MULTILINESTRING ((0 0, 1 1), EMPTY), POINT (1 2))").as_deref(),
        Ok("GEOMETRYCOLLECTION(MULTILINESTRING((0 0,1 1),EMPTY),POINT(1 2))")
    );
    assert_eq!(
        read("MULTILINESTRING Z ((0 0 1, 1 1 2), EMPTY)").as_deref(),
        Ok("MULTILINESTRING Z((0 0 1,1 1 2),EMPTY)")
    );
}
