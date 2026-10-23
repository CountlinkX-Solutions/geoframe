use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int32Type, Int64Type};
use arrow_array::{
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Float32Array, Float64Array, Int16Array,
    Int32Array, Int64Array, Int8Array, StringArray,
};
use arrow_schema::DataType;
use rusqlite::Connection;
use tempfile::TempDir;

use crate::gpkg_io::{decode_geometry_blob, encode_geometry_blob};
use crate::test_support::{column, ipc, names, path, wkt};
use crate::{gpkg_info, gpkg_layers, read_gpkg, write_gpkg, GeoColumn, GpkgWriteOptions};

const WRITE: GpkgWriteOptions = GpkgWriteOptions {
    append: false,
    spatial_index: true,
};
const APPEND: GpkgWriteOptions = GpkgWriteOptions {
    append: true,
    spatial_index: true,
};

fn sample_ipc() -> Vec<u8> {
    ipc(vec![
        (
            "id",
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
        ),
        (
            "name",
            Arc::new(StringArray::from(vec![Some("a"), Some("b c"), None])),
        ),
        (
            "score",
            Arc::new(Float64Array::from(vec![Some(1.5), None, Some(-3.25)])),
        ),
        (
            "flag",
            Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
        ),
    ])
}

fn id_ipc(n: i64) -> Vec<u8> {
    ipc(vec![(
        "id",
        Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())) as ArrayRef,
    )])
}

fn sample_points(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(&[Some("POINT (1 2)"), None, Some("POINT (3.5 -4)")], crs).unwrap()
}

// -- GeoPackage binary header -----------------------------------------------------------

#[test]
fn encoded_blobs_carry_the_header_envelope_and_wkb() {
    let geometry = GeoColumn::from_wkt(&[Some("LINESTRING (0 1, 4 5)")], None).unwrap();
    let wkb = geometry.to_wkb().unwrap().remove(0).unwrap();
    let blob = encode_geometry_blob(&wkb, 4326, Some([0.0, 1.0, 4.0, 5.0]));
    assert_eq!(&blob[..2], b"GP");
    assert_eq!(blob[2], 0, "version");
    assert_eq!(
        blob[3], 0b0000_0011,
        "little endian, xy envelope, not empty"
    );
    assert_eq!(&blob[4..8], &4326_i32.to_le_bytes());
    let doubles: Vec<f64> = blob[8..40]
        .chunks(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(doubles, [0.0, 4.0, 1.0, 5.0], "minx, maxx, miny, maxy");
    assert_eq!(&blob[40..], &wkb[..]);

    let decoded = decode_geometry_blob(&blob).unwrap();
    assert_eq!(decoded.srs_id, 4326);
    assert!(!decoded.empty);
    assert_eq!(decoded.envelope, Some([0.0, 1.0, 4.0, 5.0]));
    assert_eq!(decoded.wkb, &wkb[..]);
}

#[test]
fn blobs_without_an_envelope_and_empty_blobs_round_trip() {
    let blob = encode_geometry_blob(&[1, 1, 0, 0, 0], 0, None);
    assert_eq!(blob[3], 0b0000_0001);
    let decoded = decode_geometry_blob(&blob).unwrap();
    assert_eq!(decoded.envelope, None);
    assert_eq!(decoded.wkb, [1, 1, 0, 0, 0]);

    let mut empty = blob.clone();
    empty[3] |= 0b0001_0000;
    assert!(decode_geometry_blob(&empty).unwrap().empty);
}

#[test]
fn big_endian_headers_and_every_envelope_size_are_decoded() {
    let wkb = [1u8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    for (indicator, doubles) in [(1u8, 4usize), (2, 6), (3, 6), (4, 8)] {
        // big-endian: flag bit 0 clear
        let mut blob = vec![b'G', b'P', 0, indicator << 1];
        blob.extend_from_slice(&3857_i32.to_be_bytes());
        for i in 0..doubles {
            blob.extend_from_slice(&(i as f64).to_be_bytes());
        }
        blob.extend_from_slice(&wkb);
        let decoded = decode_geometry_blob(&blob).unwrap();
        assert_eq!(decoded.srs_id, 3857, "indicator {indicator}");
        assert_eq!(decoded.envelope, Some([0.0, 2.0, 1.0, 3.0]));
        assert_eq!(decoded.wkb, &wkb[..]);
    }
}

#[test]
fn malformed_blobs_are_errors() {
    assert!(decode_geometry_blob(b"").is_err());
    assert!(decode_geometry_blob(b"XX\0\x01\0\0\0\0").is_err());
    assert!(decode_geometry_blob(b"GP\0\x01\0\0").is_err());
    // envelope indicator 5 does not exist
    assert!(decode_geometry_blob(&[b'G', b'P', 0, 0b0000_1011, 0, 0, 0, 0]).is_err());
    // the extended-binary flag is not standard GeoPackage binary
    assert!(decode_geometry_blob(&[b'G', b'P', 0, 0b0010_0001, 0, 0, 0, 0, 1]).is_err());
    // envelope longer than the blob
    assert!(decode_geometry_blob(&[b'G', b'P', 0, 0b0000_0011, 0, 0, 0, 0, 1, 2]).is_err());
}

// -- round trips ------------------------------------------------------------------------

#[test]
fn round_trip_keeps_geometry_attributes_and_crs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let geometry = sample_points(Some("EPSG:4326"));
    let warnings = write_gpkg(&file, &geometry, &sample_ipc(), "pts", WRITE).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(read.layer, "pts");
    assert_eq!(wkt(&read.vector.geometry), wkt(&geometry));
    assert_eq!(read.vector.geometry.crs().as_deref(), Some("EPSG:4326"));
    let ipc = &read.vector.attributes_ipc;
    assert_eq!(names(ipc), ["id", "name", "score", "flag"]);
    let (t, a) = column(ipc, "id");
    assert_eq!(t, DataType::Int64);
    let v: Vec<Option<i64>> = a.as_primitive::<Int64Type>().iter().collect();
    assert_eq!(v, [Some(1), None, Some(3)]);
    let (_, a) = column(ipc, "name");
    let v: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(v, [Some("a"), Some("b c"), None]);
    let (t, a) = column(ipc, "score");
    assert_eq!(t, DataType::Float64);
    let v: Vec<Option<f64>> = a.as_primitive::<Float64Type>().iter().collect();
    assert_eq!(v, [Some(1.5), None, Some(-3.25)]);
    let (t, a) = column(ipc, "flag");
    assert_eq!(t, DataType::Boolean);
    let v: Vec<Option<bool>> = a.as_boolean().iter().collect();
    assert_eq!(v, [Some(true), None, Some(false)]);
}

#[test]
fn all_geometry_kinds_round_trip_exactly() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let geometry = GeoColumn::from_wkt(
        &[
            Some("POINT (1 2)"),
            Some("LINESTRING (0 0, 1 1)"),
            Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0), (1 1, 2 1, 2 2, 1 1))"),
            Some("MULTIPOINT (1 2, 3 4)"),
            Some("MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))"),
            Some("MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))"),
            Some("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))"),
            None,
        ],
        Some("EPSG:3857"),
    )
    .unwrap();
    write_gpkg(&file, &geometry, &id_ipc(8), "mixed", WRITE).unwrap();
    let read = read_gpkg(&file, Some("mixed")).unwrap();
    assert_eq!(wkt(&read.vector.geometry), wkt(&geometry));
    assert_eq!(read.vector.geometry.crs().as_deref(), Some("EPSG:3857"));
    let layers = gpkg_layers(&file).unwrap();
    assert_eq!(layers[0].geometry_type.as_deref(), Some("Unknown"));
}

#[test]
fn metadata_tables_follow_the_geopackage_spec() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    write_gpkg(
        &file,
        &sample_points(Some("EPSG:4326")),
        &sample_ipc(),
        "pts",
        WRITE,
    )
    .unwrap();
    let c = Connection::open(&file).unwrap();

    let application_id: i64 = c
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(application_id, 0x4750_4B47);
    let user_version: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(user_version, 10300);

    let (data_type, identifier, min_x, min_y, max_x, max_y, srs): (
        String,
        String,
        f64,
        f64,
        f64,
        f64,
        i64,
    ) = c
        .query_row(
            "SELECT data_type, identifier, min_x, min_y, max_x, max_y, srs_id \
             FROM gpkg_contents WHERE table_name = 'pts'",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(data_type, "features");
    assert_eq!(identifier, "pts");
    assert_eq!((min_x, min_y, max_x, max_y), (1.0, -4.0, 3.5, 2.0));
    assert_eq!(srs, 4326);

    let (column_name, geometry_type, srs_id, z, m): (String, String, i64, i64, i64) = c
        .query_row(
            "SELECT column_name, geometry_type_name, srs_id, z, m \
             FROM gpkg_geometry_columns WHERE table_name = 'pts'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (column_name.as_str(), geometry_type.as_str(), srs_id, z, m),
        ("geom", "POINT", 4326, 0, 0)
    );

    let defaults: Vec<i64> = c
        .prepare("SELECT srs_id FROM gpkg_spatial_ref_sys ORDER BY srs_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(defaults, [-1, 0, 4326]);
    let definition: String = c
        .query_row(
            "SELECT definition FROM gpkg_spatial_ref_sys WHERE srs_id = 4326",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(definition.starts_with("GEOGCS[\"WGS 84\""), "{definition}");

    let sql: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'pts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        sql.contains("\"fid\" INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL"),
        "{sql}"
    );
    assert!(sql.contains("\"geom\" POINT"), "{sql}");
    assert!(
        sql.contains("\"id\" INTEGER") && sql.contains("\"name\" TEXT"),
        "{sql}"
    );
    assert!(
        sql.contains("\"score\" REAL") && sql.contains("\"flag\" BOOLEAN"),
        "{sql}"
    );

    // The geometry blob is a GeoPackageBinary with an envelope.
    let blob: Vec<u8> = c
        .query_row("SELECT geom FROM pts WHERE fid = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(&blob[..2], b"GP");
    let null_geometry: Option<Vec<u8>> = c
        .query_row("SELECT geom FROM pts WHERE fid = 2", [], |r| r.get(0))
        .unwrap();
    assert!(null_geometry.is_none());
}

#[test]
fn the_rtree_index_is_populated_and_registered() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    write_gpkg(
        &file,
        &sample_points(Some("EPSG:4326")),
        &sample_ipc(),
        "pts",
        WRITE,
    )
    .unwrap();
    let c = Connection::open(&file).unwrap();
    let ids: Vec<i64> = c
        .prepare(
            "SELECT id FROM rtree_pts_geom \
             WHERE minx <= 1.5 AND maxx >= 0.5 AND miny <= 2.5 AND maxy >= 1.5",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(ids, [1]);
    let total: i64 = c
        .query_row("SELECT count(*) FROM rtree_pts_geom", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "the null geometry is not indexed");
    let extension: String = c
        .query_row(
            "SELECT extension_name FROM gpkg_extensions \
             WHERE table_name = 'pts' AND column_name = 'geom'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(extension, "gpkg_rtree_index");
    let triggers: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'rtree_pts_geom_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(triggers >= 6);
}

#[test]
fn the_spatial_index_can_be_skipped() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let options = GpkgWriteOptions {
        append: false,
        spatial_index: false,
    };
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "pts", options).unwrap();
    let c = Connection::open(&file).unwrap();
    let tables: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'rtree_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    assert_eq!(read_gpkg(&file, None).unwrap().vector.geometry.len(), 3);
}

#[test]
fn column_types_map_to_geopackage_types_and_back() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let attrs = ipc(vec![
        ("i8", Arc::new(Int8Array::from(vec![1, 2, 3])) as ArrayRef),
        ("i16", Arc::new(Int16Array::from(vec![1, 2, 3]))),
        ("i32", Arc::new(Int32Array::from(vec![1, 2, 3]))),
        ("i64", Arc::new(Int64Array::from(vec![1, 2, 3]))),
        ("f32", Arc::new(Float32Array::from(vec![1.5, 2.5, 3.5]))),
        ("f64", Arc::new(Float64Array::from(vec![1.5, 2.5, 3.5]))),
        (
            "day",
            Arc::new(Date32Array::from(vec![Some(0), None, Some(19_000)])),
        ),
        (
            "blob",
            Arc::new(BinaryArray::from_vec(vec![b"a", b"b", b"c"])),
        ),
    ]);
    write_gpkg(&file, &sample_points(None), &attrs, "t", WRITE).unwrap();
    let c = Connection::open(&file).unwrap();
    let declared: Vec<(String, String)> = c
        .prepare("SELECT name, type FROM pragma_table_info('t')")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let pairs: Vec<(&str, &str)> = declared
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("fid", "INTEGER"),
            ("geom", "POINT"),
            ("i8", "TINYINT"),
            ("i16", "SMALLINT"),
            ("i32", "MEDIUMINT"),
            ("i64", "INTEGER"),
            ("f32", "FLOAT"),
            ("f64", "REAL"),
            ("day", "DATE"),
            ("blob", "BLOB")
        ]
    );

    let read = read_gpkg(&file, None).unwrap();
    let ipc = &read.vector.attributes_ipc;
    let types: Vec<DataType> = crate::test_support::columns(ipc)
        .into_iter()
        .map(|(_, t, _)| t)
        .collect();
    assert_eq!(
        types,
        [
            DataType::Int8,
            DataType::Int16,
            DataType::Int32,
            DataType::Int64,
            DataType::Float32,
            DataType::Float64,
            DataType::Date32,
            DataType::Binary
        ]
    );
    let (_, a) = column(ipc, "i32");
    let v: Vec<Option<i32>> = a.as_primitive::<Int32Type>().iter().collect();
    assert_eq!(v, [Some(1), Some(2), Some(3)]);
}

// -- layers and modes -------------------------------------------------------------------

#[test]
fn several_layers_need_an_explicit_choice() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "m.gpkg");
    write_gpkg(
        &file,
        &sample_points(Some("EPSG:4326")),
        &id_ipc(3),
        "pts",
        WRITE,
    )
    .unwrap();
    let polygons = GeoColumn::from_wkt(
        &[Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"), None],
        Some("EPSG:3857"),
    )
    .unwrap();
    write_gpkg(&file, &polygons, &id_ipc(2), "polys", WRITE).unwrap();

    let layers = gpkg_layers(&file).unwrap();
    let listed: Vec<(&str, Option<&str>)> = layers
        .iter()
        .map(|l| (l.name.as_str(), l.geometry_type.as_deref()))
        .collect();
    assert_eq!(listed, [("pts", Some("Point")), ("polys", Some("Polygon"))]);

    let error = read_gpkg(&file, None).err().unwrap();
    assert!(
        error.contains("pts") && error.contains("polys") && error.contains("layer"),
        "{error}"
    );
    let read = read_gpkg(&file, Some("polys")).unwrap();
    assert_eq!(read.vector.geometry.len(), 2);
    assert_eq!(read.vector.geometry.crs().as_deref(), Some("EPSG:3857"));
    assert!(read_gpkg(&file, Some("nope"))
        .err()
        .unwrap()
        .contains("nope"));
}

#[test]
fn writing_an_existing_layer_replaces_it_and_keeps_the_others() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "m.gpkg");
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "a", WRITE).unwrap();
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "b", WRITE).unwrap();
    let one = GeoColumn::from_wkt(&[Some("POINT (9 9)")], None).unwrap();
    write_gpkg(&file, &one, &id_ipc(1), "a", WRITE).unwrap();
    assert_eq!(
        read_gpkg(&file, Some("a")).unwrap().vector.geometry.len(),
        1
    );
    assert_eq!(
        read_gpkg(&file, Some("b")).unwrap().vector.geometry.len(),
        3
    );
    let c = Connection::open(&file).unwrap();
    let rtree: i64 = c
        .query_row("SELECT count(*) FROM rtree_a_geom", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rtree, 1);
}

#[test]
fn append_adds_rows_extends_the_extent_and_checks_the_schema() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    write_gpkg(
        &file,
        &sample_points(Some("EPSG:4326")),
        &sample_ipc(),
        "pts",
        WRITE,
    )
    .unwrap();
    let more = GeoColumn::from_wkt(&[Some("POINT (50 60)")], Some("EPSG:4326")).unwrap();
    let attrs = ipc(vec![
        ("id", Arc::new(Int64Array::from(vec![9])) as ArrayRef),
        ("name", Arc::new(StringArray::from(vec!["z"]))),
    ]);
    write_gpkg(&file, &more, &attrs, "pts", APPEND).unwrap();

    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(read.vector.geometry.len(), 4);
    assert_eq!(
        wkt(&read.vector.geometry)[3].as_deref(),
        Some("POINT(50 60)")
    );
    let (_, a) = column(&read.vector.attributes_ipc, "score");
    assert!(a.is_null(3), "missing columns are null");
    let info = gpkg_info(&file, None).unwrap();
    assert_eq!(info.bounds, Some([1.0, -4.0, 50.0, 60.0]));
    assert_eq!(info.features, 4);

    let unknown = ipc(vec![(
        "zzz",
        Arc::new(Int64Array::from(vec![1])) as ArrayRef,
    )]);
    let error = write_gpkg(&file, &more, &unknown, "pts", APPEND).unwrap_err();
    assert!(error.contains("zzz"), "{error}");
    let other_crs = GeoColumn::from_wkt(&[Some("POINT (1 1)")], Some("EPSG:3857")).unwrap();
    let error = write_gpkg(&file, &other_crs, &id_ipc(1), "pts", APPEND).unwrap_err();
    assert!(error.contains("CRS"), "{error}");
    let rtree: i64 = Connection::open(&file)
        .unwrap()
        .query_row("SELECT count(*) FROM rtree_pts_geom", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rtree, 3, "triggers index appended rows");
}

#[test]
fn append_to_a_missing_layer_or_file_creates_it() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "a", APPEND).unwrap();
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "b", APPEND).unwrap();
    assert_eq!(gpkg_layers(&file).unwrap().len(), 2);
}

#[test]
fn odd_layer_and_column_names_are_quoted() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let attrs = ipc(vec![(
        "we\"ird col",
        Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
    )]);
    write_gpkg(&file, &sample_points(None), &attrs, "my \"layer\" 1", WRITE).unwrap();
    let read = read_gpkg(&file, Some("my \"layer\" 1")).unwrap();
    assert_eq!(names(&read.vector.attributes_ipc), ["we\"ird col"]);
    assert!(write_gpkg(&file, &sample_points(None), &id_ipc(3), "", WRITE).is_err());
}

#[test]
fn unknown_epsg_codes_warn_and_still_round_trip() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let warnings = write_gpkg(
        &file,
        &sample_points(Some("EPSG:999999")),
        &id_ipc(3),
        "pts",
        WRITE,
    )
    .unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("EPSG:999999")),
        "{warnings:?}"
    );
    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(read.vector.geometry.crs().as_deref(), Some("EPSG:999999"));
}

#[test]
fn wkt_crs_strings_get_a_custom_srs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    let custom =
        r#"PROJCS["Custom_Thing",GEOGCS["GCS_Custom"],PROJECTION["Lambert_Conformal_Conic"]]"#;
    write_gpkg(
        &file,
        &sample_points(Some(custom)),
        &id_ipc(3),
        "pts",
        WRITE,
    )
    .unwrap();
    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(read.vector.geometry.crs().as_deref(), Some(custom));
}

#[test]
fn undefined_srs_reads_back_as_no_crs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.gpkg");
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "pts", WRITE).unwrap();
    assert_eq!(read_gpkg(&file, None).unwrap().vector.geometry.crs(), None);
}

// -- files written by other tools -------------------------------------------------------

/// A GeoPackage laid out like GDAL writes it: points without an envelope, a geometry column
/// named `geom`, `fid`, `gpkg_ogr_contents`, a plain attributes table next to the features.
fn gdal_like(file: &str) {
    let c = Connection::open(file).unwrap();
    c.execute_batch(
        r#"
        PRAGMA application_id = 1196444487; PRAGMA user_version = 10400;
        CREATE TABLE gpkg_spatial_ref_sys (srs_name TEXT NOT NULL, srs_id INTEGER NOT NULL PRIMARY KEY,
          organization TEXT NOT NULL, organization_coordsys_id INTEGER NOT NULL, definition TEXT NOT NULL, description TEXT);
        INSERT INTO gpkg_spatial_ref_sys VALUES ('Undefined Cartesian SRS',-1,'NONE',-1,'undefined',NULL);
        INSERT INTO gpkg_spatial_ref_sys VALUES ('Undefined geographic SRS',0,'NONE',0,'undefined',NULL);
        INSERT INTO gpkg_spatial_ref_sys VALUES ('WGS 84 geodetic',4326,'epsg',4326,'GEOGCS["WGS 84"]',NULL);
        CREATE TABLE gpkg_contents (table_name TEXT NOT NULL PRIMARY KEY, data_type TEXT NOT NULL, identifier TEXT UNIQUE,
          description TEXT DEFAULT '', last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER);
        CREATE TABLE gpkg_geometry_columns (table_name TEXT NOT NULL, column_name TEXT NOT NULL, geometry_type_name TEXT NOT NULL,
          srs_id INTEGER NOT NULL, z TINYINT NOT NULL, m TINYINT NOT NULL);
        CREATE TABLE gpkg_ogr_contents(table_name TEXT NOT NULL PRIMARY KEY, feature_count INTEGER DEFAULT NULL);
        CREATE TABLE "places" ( "fid" INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, "geom" POINT, "name" TEXT(20), "n" INTEGER, "when" DATETIME, "ok" BOOLEAN);
        INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id) VALUES ('places','features','places',4326);
        INSERT INTO gpkg_contents (table_name, data_type, identifier) VALUES ('lookup','attributes','lookup');
        INSERT INTO gpkg_geometry_columns VALUES ('places','geom','POINT',4326,0,0);
        CREATE TABLE "lookup" ("fid" INTEGER PRIMARY KEY, "k" TEXT);
        "#,
    )
    .unwrap();
    // x = 7, y = 8, no envelope (flags 0x01, as GDAL writes for points)
    let mut point = vec![b'G', b'P', 0, 1];
    point.extend_from_slice(&4326_i32.to_le_bytes());
    point.extend_from_slice(&[1, 1, 0, 0, 0]);
    point.extend_from_slice(&7.0_f64.to_le_bytes());
    point.extend_from_slice(&8.0_f64.to_le_bytes());
    c.execute(
        "INSERT INTO places (geom, name, n, \"when\", ok) VALUES (?1, 'x', 5, '2020-01-02T03:04:05.000Z', 1)",
        [&point],
    )
    .unwrap();
    c.execute("INSERT INTO places (geom, name) VALUES (NULL, NULL)", [])
        .unwrap();
}

#[test]
fn files_written_by_gdal_are_read() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "g.gpkg");
    gdal_like(&file);

    let layers = gpkg_layers(&file).unwrap();
    let listed: Vec<(&str, Option<&str>)> = layers
        .iter()
        .map(|l| (l.name.as_str(), l.geometry_type.as_deref()))
        .collect();
    assert_eq!(listed, [("places", Some("Point")), ("lookup", None)]);

    // Only one feature layer: no layer name needed. The fid column is not an attribute.
    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(read.layer, "places");
    assert_eq!(
        wkt(&read.vector.geometry),
        [Some("POINT(7 8)".to_string()), None]
    );
    assert_eq!(read.vector.geometry.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(
        names(&read.vector.attributes_ipc),
        ["name", "n", "when", "ok"]
    );
    let (t, a) = column(&read.vector.attributes_ipc, "when");
    assert!(matches!(t, DataType::Timestamp(_, _)), "{t:?}");
    assert!(!a.is_null(0) && a.is_null(1));
    let (t, _) = column(&read.vector.attributes_ipc, "ok");
    assert_eq!(t, DataType::Boolean);

    assert!(read_gpkg(&file, Some("lookup"))
        .err()
        .unwrap()
        .contains("geometry"));

    let info = gpkg_info(&file, None).unwrap();
    assert_eq!(info.driver, "GPKG");
    assert_eq!(info.features, 2);
    assert_eq!(info.bounds, Some([7.0, 8.0, 7.0, 8.0]));
    assert_eq!(info.geometry_type.as_deref(), Some("Point"));
}

#[test]
fn files_that_are_not_geopackages_are_errors() {
    let dir = TempDir::new().unwrap();
    let plain = path(&dir, "plain.gpkg");
    Connection::open(&plain)
        .unwrap()
        .execute_batch("CREATE TABLE t (a INTEGER)")
        .unwrap();
    assert!(read_gpkg(&plain, None)
        .err()
        .unwrap()
        .contains("GeoPackage"));
    assert!(gpkg_layers(&plain).is_err());
    // writing never clobbers a foreign database
    let error = write_gpkg(&plain, &sample_points(None), &id_ipc(3), "pts", WRITE).unwrap_err();
    assert!(error.contains("GeoPackage"), "{error}");

    let text = path(&dir, "text.gpkg");
    std::fs::write(
        &text,
        b"definitely not sqlite, just some text that is long enough",
    )
    .unwrap();
    assert!(read_gpkg(&text, None).is_err());
    assert!(read_gpkg(&path(&dir, "missing.gpkg"), None).is_err());
}

#[test]
fn the_sql_helper_functions_agree_with_the_header() {
    // Appending to a file written by GDAL fires the rtree triggers, which call ST_* functions
    // that this crate registers on its own connection.
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "g.gpkg");
    gdal_like(&file);
    {
        let c = Connection::open(&file).unwrap();
        c.execute_batch(
            r#"CREATE VIRTUAL TABLE rtree_places_geom USING rtree(id, minx, maxx, miny, maxy);
               CREATE TABLE gpkg_extensions (table_name TEXT, column_name TEXT, extension_name TEXT NOT NULL,
                 definition TEXT NOT NULL, scope TEXT NOT NULL, CONSTRAINT ge_tce UNIQUE (table_name, column_name, extension_name));
               INSERT INTO gpkg_extensions VALUES ('places','geom','gpkg_rtree_index','x','write-only');
               CREATE TRIGGER rtree_places_geom_insert AFTER INSERT ON "places"
                 WHEN (new."geom" NOT NULL AND NOT ST_IsEmpty(NEW."geom"))
                 BEGIN INSERT OR REPLACE INTO rtree_places_geom VALUES (NEW."fid",ST_MinX(NEW."geom"), ST_MaxX(NEW."geom"),ST_MinY(NEW."geom"), ST_MaxY(NEW."geom")); END;"#,
        )
        .unwrap();
    }
    let more = GeoColumn::from_wkt(&[Some("POINT (2 3)")], Some("EPSG:4326")).unwrap();
    let attrs = ipc(vec![("n", Arc::new(Int64Array::from(vec![1])) as ArrayRef)]);
    write_gpkg(&file, &more, &attrs, "places", APPEND).unwrap();
    let (minx, maxy): (f64, f64) = Connection::open(&file)
        .unwrap()
        .query_row("SELECT minx, maxy FROM rtree_places_geom", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((minx, maxy), (2.0, 3.0));
}

// -- hardening ---------------------------------------------------------------------------

#[test]
fn a_failed_first_write_leaves_no_file_and_a_retry_succeeds() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "new.gpkg");
    let bad = GeoColumn::from_wkt(&[Some("POINT (1 2)")], Some("not-a-crs")).unwrap();
    let error = write_gpkg(&file, &bad, &id_ipc(1), "pts", WRITE).unwrap_err();
    assert!(error.contains("CRS"), "{error}");
    assert!(!std::path::Path::new(&file).exists(), "no half-made file");

    write_gpkg(
        &file,
        &sample_points(Some("EPSG:4326")),
        &id_ipc(3),
        "pts",
        WRITE,
    )
    .unwrap();
    assert_eq!(read_gpkg(&file, None).unwrap().vector.geometry.len(), 3);
}

#[test]
fn a_failed_write_keeps_an_existing_geopackage_usable() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "keep.gpkg");
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "pts", WRITE).unwrap();
    let bad = GeoColumn::from_wkt(&[Some("POINT (1 2)")], Some("not-a-crs")).unwrap();
    assert!(write_gpkg(&file, &bad, &id_ipc(1), "pts", WRITE).is_err());
    assert_eq!(read_gpkg(&file, None).unwrap().vector.geometry.len(), 3);
}

#[test]
fn append_rejects_another_geometry_type() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "t.gpkg");
    write_gpkg(&file, &sample_points(None), &id_ipc(3), "pts", WRITE).unwrap();
    let lines = GeoColumn::from_wkt(&[Some("LINESTRING (0 0, 1 1)")], None).unwrap();
    let error = write_gpkg(&file, &lines, &id_ipc(1), "pts", APPEND).unwrap_err();
    assert!(error.contains("geometry type"), "{error}");
    assert_eq!(read_gpkg(&file, None).unwrap().vector.geometry.len(), 3);

    // A GEOMETRY layer takes anything; the same type appends fine.
    let mixed =
        GeoColumn::from_wkt(&[Some("POINT (1 1)"), Some("LINESTRING (0 0, 1 1)")], None).unwrap();
    let mixed_file = path(&dir, "m.gpkg");
    write_gpkg(&mixed_file, &mixed, &id_ipc(2), "m", WRITE).unwrap();
    write_gpkg(&mixed_file, &lines, &id_ipc(1), "m", APPEND).unwrap();
    let points = GeoColumn::from_wkt(&[Some("POINT (5 5)")], None).unwrap();
    write_gpkg(&file, &points, &id_ipc(1), "pts", APPEND).unwrap();
}

#[test]
fn append_rejects_incompatible_column_types() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "c.gpkg");
    let one = GeoColumn::from_wkt(&[Some("POINT (1 1)")], None).unwrap();
    let text = ipc(vec![(
        "v",
        Arc::new(StringArray::from(vec!["a"])) as ArrayRef,
    )]);
    write_gpkg(&file, &one, &text, "pts", WRITE).unwrap();
    let number = ipc(vec![("v", Arc::new(Int64Array::from(vec![1])) as ArrayRef)]);
    let error = write_gpkg(&file, &one, &number, "pts", APPEND).unwrap_err();
    assert!(error.contains("\"v\""), "{error}");

    let ints = path(&dir, "i.gpkg");
    write_gpkg(&ints, &one, &number, "pts", WRITE).unwrap();
    let real = ipc(vec![(
        "v",
        Arc::new(Float64Array::from(vec![1.5])) as ArrayRef,
    )]);
    assert!(write_gpkg(&ints, &one, &real, "pts", APPEND).is_err());
    // Integers fit a real column.
    let reals = path(&dir, "r.gpkg");
    write_gpkg(&reals, &one, &real, "pts", WRITE).unwrap();
    write_gpkg(&reals, &one, &number, "pts", APPEND).unwrap();
    assert_eq!(read_gpkg(&reals, None).unwrap().vector.geometry.len(), 2);
}

#[test]
fn timestamps_with_an_offset_round_trip_to_the_same_instant() {
    use arrow_array::TimestampMicrosecondArray;
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "ts.gpkg");
    // 2024-01-01T10:00:00Z, shown as 12:00 at +02:00.
    let instant = 1_704_103_200_000_000_i64;
    let zoned = TimestampMicrosecondArray::from(vec![Some(instant)]).with_timezone("+02:00");
    let attrs = ipc(vec![("t", Arc::new(zoned) as ArrayRef)]);
    let one = GeoColumn::from_wkt(&[Some("POINT (1 1)")], None).unwrap();
    write_gpkg(&file, &one, &attrs, "pts", WRITE).unwrap();
    let stored: String = Connection::open(&file)
        .unwrap()
        .query_row("SELECT t FROM pts", [], |r| r.get(0))
        .unwrap();
    assert!(!stored.ends_with('Z'), "{stored}");
    let read = read_gpkg(&file, None).unwrap();
    let (_, a) = column(&read.vector.attributes_ipc, "t");
    let values = a
        .as_any()
        .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
        .unwrap();
    assert_eq!(values.iter().collect::<Vec<_>>(), [Some(instant)]);
}

#[test]
fn small_integer_columns_widen_instead_of_nulling_out_of_range_values() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "w.gpkg");
    let one = GeoColumn::from_wkt(&[Some("POINT (1 1)"), Some("POINT (2 2)")], None).unwrap();
    let attrs = ipc(vec![(
        "n",
        Arc::new(Int16Array::from(vec![1, 2])) as ArrayRef,
    )]);
    let no_index = GpkgWriteOptions {
        append: false,
        spatial_index: false,
    };
    write_gpkg(&file, &one, &attrs, "pts", no_index).unwrap();
    let read = read_gpkg(&file, None).unwrap();
    assert_eq!(column(&read.vector.attributes_ipc, "n").0, DataType::Int16);

    Connection::open(&file)
        .unwrap()
        .execute("UPDATE pts SET n = 100000 WHERE fid = 2", [])
        .unwrap();
    let read = read_gpkg(&file, None).unwrap();
    let (t, a) = column(&read.vector.attributes_ipc, "n");
    assert_eq!(t, DataType::Int64);
    let values: Vec<Option<i64>> = a.as_primitive::<Int64Type>().iter().collect();
    assert_eq!(values, [Some(1), Some(100_000)]);
}
