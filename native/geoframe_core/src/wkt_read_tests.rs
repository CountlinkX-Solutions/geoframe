//! WKT read as GEOS 3.14 reads it (Shapely 2.2 `from_wkt`, probed 2026-11-13).

use crate::GeoColumn;

fn read(text: &str) -> Result<String, String> {
    let column = GeoColumn::from_wkt(&[Some(text)], None)?;
    Ok(column.to_wkt()?[0].clone().unwrap())
}

// GEOS takes the dimension of an untagged geometry from its first coordinate: three
// ordinates are Z, four are ZM (never M). Each untagged text reads as its tagged twin.
#[test]
fn an_untagged_geometry_takes_its_dimension_from_its_first_coordinate() {
    for (untagged, tagged) in [
        ("POINT (1 2 3)", "POINT Z (1 2 3)"),
        ("POINT (1 2 3 4)", "POINT ZM (1 2 3 4)"),
        ("point(1 2 3)", "POINT Z (1 2 3)"),
        ("POINT ( 1  2\t3 )", "POINT Z (1 2 3)"),
        ("POINT (1 2 -3.5e2)", "POINT Z (1 2 -350)"),
        ("LINESTRING (1 2 3,4 5 6)", "LINESTRING Z (1 2 3, 4 5 6)"),
        (
            "LINESTRING (1 2 3 4, 5 6 7 8)",
            "LINESTRING ZM (1 2 3 4, 5 6 7 8)",
        ),
        (
            "POLYGON ((0 0 1, 1 0 1, 1 1 1, 0 0 1))",
            "POLYGON Z ((0 0 1, 1 0 1, 1 1 1, 0 0 1))",
        ),
        (
            "POLYGON ((0 0 1, 9 0 1, 9 9 1, 0 0 1), (1 1 2, 2 1 2, 2 2 2, 1 1 2))",
            "POLYGON Z ((0 0 1, 9 0 1, 9 9 1, 0 0 1), (1 1 2, 2 1 2, 2 2 2, 1 1 2))",
        ),
        (
            "MULTIPOINT (1 2 3, 4 5 6)",
            "MULTIPOINT Z ((1 2 3), (4 5 6))",
        ),
        (
            "MULTIPOINT ((1 2 3), (4 5 6))",
            "MULTIPOINT Z ((1 2 3), (4 5 6))",
        ),
        (
            "MULTILINESTRING ((1 2 3, 4 5 6), (7 8 9, 1 2 3))",
            "MULTILINESTRING Z ((1 2 3, 4 5 6), (7 8 9, 1 2 3))",
        ),
        (
            "MULTILINESTRING (EMPTY, (1 2 3, 4 5 6))",
            "MULTILINESTRING Z (EMPTY, (1 2 3, 4 5 6))",
        ),
        (
            "MULTIPOLYGON (((0 0 1, 1 0 1, 1 1 1, 0 0 1)), ((5 5 1, 6 5 1, 6 6 1, 5 5 1)))",
            "MULTIPOLYGON Z (((0 0 1, 1 0 1, 1 1 1, 0 0 1)), ((5 5 1, 6 5 1, 6 6 1, 5 5 1)))",
        ),
    ] {
        assert_eq!(read(untagged), read(tagged), "{untagged}");
        assert!(read(untagged).is_ok(), "{untagged}: {:?}", read(untagged));
    }
}

// A collection member infers its own dimension; an untagged collection then follows the
// 0.3.0 rule (the union of its members' dimensions, a lower member promoted with NaN).
#[test]
fn a_collection_member_infers_its_own_dimension() {
    for (untagged, tagged) in [
        (
            "GEOMETRYCOLLECTION Z (POINT (1 2 3), LINESTRING (0 0 1, 1 1 2))",
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT (1 2 3), LINESTRING (0 0 1, 1 1 2))",
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))",
        ),
        (
            "GEOMETRYCOLLECTION ZM (POINT (1 2 3 4), POINT ZM (4 5 6 7))",
            "GEOMETRYCOLLECTION ZM (POINT ZM (1 2 3 4), POINT ZM (4 5 6 7))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT (1 2 3), POINT (4 5))",
            "GEOMETRYCOLLECTION (POINT Z (1 2 3), POINT (4 5))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT (1 2 3 4), LINESTRING (0 0 1, 1 1 1))",
            "GEOMETRYCOLLECTION (POINT ZM (1 2 3 4), LINESTRING Z (0 0 1, 1 1 1))",
        ),
        (
            "GEOMETRYCOLLECTION (POINT EMPTY, POINT (1 2 3))",
            "GEOMETRYCOLLECTION (POINT EMPTY, POINT Z (1 2 3))",
        ),
        (
            "GEOMETRYCOLLECTION (MULTIPOINT (1 2 3), POINT (1 2))",
            "GEOMETRYCOLLECTION (MULTIPOINT Z ((1 2 3)), POINT (1 2))",
        ),
    ] {
        assert!(read(untagged).is_ok(), "{untagged}: {:?}", read(untagged));
        assert_eq!(read(untagged), read(tagged), "{untagged}");
    }
}

#[test]
fn an_inferred_member_of_another_dimension_in_a_tagged_collection_is_an_error_as_in_geos() {
    for text in [
        "GEOMETRYCOLLECTION M (POINT (1 2 3))",
        "GEOMETRYCOLLECTION Z (POINT (1 2 3 4))",
        "GEOMETRYCOLLECTION ZM (POINT (1 2 3))",
        "GEOMETRYCOLLECTION Z (POINT (1 2))",
    ] {
        let err = read(text).unwrap_err();
        assert!(err.contains("cannot mix dimensionality"), "{text}: {err}");
    }
}

// GEOS raises a ParseException for each of these: the coordinates of one geometry must all
// have the count of its first one, and a coordinate has at most four ordinates.
#[test]
fn coordinates_that_disagree_with_the_first_one_are_an_error() {
    for text in [
        "POINT (1 2 3 4 5)",
        "LINESTRING (1 2 3, 4 5)",
        "LINESTRING (1 2, 4 5 6)",
        "LINESTRING (1 2 3, 5 6 7 8)",
        "LINESTRING (1 2 3 4, 5 6 7)",
        "MULTIPOINT ((1 2), (4 5 6))",
        "MULTIPOINT ((1 2 3), (4 5))",
        "MULTIPOINT (1 2 3, 5 6 7 8)",
        "POLYGON ((0 0 1, 1 0 1, 1 1 1, 0 0 1), (0.1 0.1, 0.2 0.1, 0.2 0.2, 0.1 0.1))",
        "POLYGON ((0 0, 1 0, 1 1, 0 0), (0.1 0.1 5, 0.2 0.1 5, 0.2 0.2 5, 0.1 0.1 5))",
        "MULTILINESTRING ((1 2 3, 4 5 6), (7 8, 9 10))",
        "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5 1, 6 5 1, 6 6 1, 5 5 1)))",
        "GEOMETRYCOLLECTION Z (MULTIPOINT ((1 2 3), (4 5)))",
        "POINT Z (1 2 3 4)",
        "LINESTRING Z (1 2, 4 5 6)",
    ] {
        let err = read(text).unwrap_err();
        assert!(err.starts_with("invalid WKT at row 0"), "{text}: {err}");
    }
}

// Deviation (documented): GEOS reads the unparenthesized MULTIPOINT form with four untagged
// ordinates as `MULTIPOINT Z ((1 2 3), (5 6 7))`, dropping M, while its parenthesized form
// keeps ZM. Both forms keep ZM here.
#[test]
fn the_unparenthesized_multipoint_form_keeps_m() {
    assert_eq!(
        read("MULTIPOINT (1 2 3 4, 5 6 7 8)"),
        read("MULTIPOINT ZM ((1 2 3 4), (5 6 7 8))")
    );
    assert!(read("MULTIPOINT (1 2 3 4, 5 6 7 8)").is_ok());
}

#[test]
fn rows_infer_their_dimension_independently() {
    let column = GeoColumn::from_wkt(
        &[Some("POINT (1 2 3)"), None, Some("POINT Z (4 5 6)")],
        None,
    )
    .unwrap();
    assert_eq!(
        column.to_wkt().unwrap(),
        vec![
            Some("POINT Z(1 2 3)".to_string()),
            None,
            Some("POINT Z(4 5 6)".to_string())
        ]
    );
    let err =
        GeoColumn::from_wkt(&[Some("POINT (1 2)"), Some("POINT (1 2 3 4 5)")], None).unwrap_err();
    assert!(err.starts_with("invalid WKT at row 1"), "{err}");
}

#[test]
fn text_the_wkt_crate_reads_is_unchanged() {
    for (text, expected) in [
        ("POINT (1 2)", "POINT(1 2)"),
        ("POINT EMPTY", "POINT EMPTY"),
        ("POINT M (1 2 3)", "POINT M(1 2 3)"),
        ("POINTZ (1 2 3)", "POINT Z(1 2 3)"),
        ("LINESTRING (0 0, 1 1)", "LINESTRING(0 0,1 1)"),
    ] {
        assert_eq!(read(text).unwrap(), expected, "{text}");
    }
}
