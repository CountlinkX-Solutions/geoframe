use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Float64Type, Int32Type, Int64Type};
use arrow_array::{
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Float64Array, Int32Array, Int64Array,
    StringArray,
};
use arrow_schema::DataType;
use tempfile::TempDir;

use crate::crs_wkt::{crs_to_prj_wkt, prj_wkt_to_crs};
use crate::dbf::launder_field_names;
use crate::test_support::{column, ipc, names, path, wkt};
use crate::{read_shapefile, shapefile_info, write_shapefile, GeoColumn};

fn sample_ipc() -> Vec<u8> {
    ipc(vec![
        (
            "id",
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
        ),
        (
            "small",
            Arc::new(Int32Array::from(vec![Some(-7), Some(8), None])),
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
        (
            "day",
            Arc::new(Date32Array::from(vec![Some(0), Some(19_000), None])),
        ),
    ])
}

fn sample_points(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(&[Some("POINT (1 2)"), None, Some("POINT (3.5 -4)")], crs).unwrap()
}

fn id_ipc(n: i64) -> Vec<u8> {
    ipc(vec![(
        "id",
        Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())) as ArrayRef,
    )])
}

// -- .prj mapping -----------------------------------------------------------------------

#[test]
fn prj_with_an_epsg_authority_maps_to_the_code() {
    let wkt = crs_definitions::from_code(32617).unwrap().wkt;
    assert_eq!(prj_wkt_to_crs(wkt), "EPSG:32617");
}

#[test]
fn prj_esri_names_map_to_epsg_codes() {
    let cases = [
        (
            r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#,
            "EPSG:4326",
        ),
        (
            r#"GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#,
            "EPSG:4269",
        ),
        (
            r#"PROJCS["WGS_1984_Web_Mercator_Auxiliary_Sphere",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Mercator_Auxiliary_Sphere"],UNIT["Meter",1.0]]"#,
            "EPSG:3857",
        ),
        (
            r#"PROJCS["WGS_1984_UTM_Zone_17N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],UNIT["Meter",1.0]]"#,
            "EPSG:32617",
        ),
        (
            r#"PROJCS["WGS_1984_UTM_Zone_33S",GEOGCS["GCS_WGS_1984"],PROJECTION["Transverse_Mercator"]]"#,
            "EPSG:32733",
        ),
        (
            r#"PROJCS["NAD_1983_UTM_Zone_17N",GEOGCS["GCS_North_American_1983"],PROJECTION["Transverse_Mercator"]]"#,
            "EPSG:26917",
        ),
    ];
    for (wkt, expected) in cases {
        assert_eq!(prj_wkt_to_crs(wkt), expected, "{wkt}");
    }
}

#[test]
fn prj_unknown_definitions_stay_wkt() {
    let wkt =
        r#"PROJCS["Custom_Thing",GEOGCS["GCS_Custom"],PROJECTION["Lambert_Conformal_Conic"]]"#;
    assert_eq!(prj_wkt_to_crs(&format!("  {wkt}\n")), wkt);
}

#[test]
fn crs_strings_map_to_prj_text_when_known() {
    for (crs, code) in [
        ("EPSG:4326", "4326"),
        ("epsg:3857", "3857"),
        ("EPSG:32617", "32617"),
        ("EPSG:32733", "32733"),
        ("EPSG:4269", "4269"),
        ("OGC:CRS84", "4326"),
    ] {
        let prj = crs_to_prj_wkt(crs).unwrap_or_else(|| panic!("no prj for {crs}"));
        assert_eq!(prj_wkt_to_crs(&prj), format!("EPSG:{code}"), "{crs}");
    }
    let projjson = r#"{"type":"GeographicCRS","id":{"authority":"EPSG","code":4326}}"#;
    assert!(crs_to_prj_wkt(projjson).is_some());
}

#[test]
fn crs_strings_pass_wkt_through_and_reject_unknown_codes() {
    let wkt = r#"PROJCS["Custom_Thing",GEOGCS["GCS_Custom"]]"#;
    assert_eq!(crs_to_prj_wkt(wkt).as_deref(), Some(wkt));
    assert_eq!(crs_to_prj_wkt("EPSG:999999"), None);
    assert_eq!(crs_to_prj_wkt("ESRI:54009"), None);
    assert_eq!(crs_to_prj_wkt("not a crs"), None);
}

// -- field names ------------------------------------------------------------------------

#[test]
fn field_names_are_laundered_like_gdal() {
    let input: Vec<String> = [
        "abcdefghijkl",
        "abcdefghijkm",
        "abcdefghijkn",
        "short",
        "é_name_long_é",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let (names, changed) = launder_field_names(&input);
    assert_eq!(
        names,
        [
            "abcdefghij",
            "abcdefgh_1",
            "abcdefgh_2",
            "short",
            "é_name_lo"
        ]
    );
    assert_eq!(changed, [true, true, true, false, true]);
}

#[test]
fn laundering_handles_many_collisions_and_case() {
    let input: Vec<String> = (0..12).map(|i| format!("long_column_name_{i}")).collect();
    let (names, _) = launder_field_names(&input);
    assert_eq!(names[0], "long_colum");
    assert_eq!(names[1], "long_col_1");
    assert_eq!(names[9], "long_col_9");
    assert_eq!(names[10], "long_co_10");
    let unique: std::collections::HashSet<String> =
        names.iter().map(|n| n.to_ascii_lowercase()).collect();
    assert_eq!(unique.len(), names.len());
    let (same, _) = launder_field_names(&["Name".to_string(), "NAME".to_string()]);
    assert_eq!(same, ["Name", "NAME_1"]);
}

// -- round trips ------------------------------------------------------------------------

#[test]
fn point_round_trip_keeps_geometry_attributes_and_crs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let geometry = sample_points(Some("EPSG:4326"));
    let warnings = write_shapefile(&file, &geometry, &sample_ipc(), None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    for ext in ["shp", "shx", "dbf", "prj", "cpg"] {
        assert!(dir.path().join(format!("a.{ext}")).exists(), "{ext}");
    }

    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&geometry));
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(
        names(&read.attributes_ipc),
        ["id", "small", "name", "score", "flag", "day"]
    );

    let (t, a) = column(&read.attributes_ipc, "id");
    assert_eq!(t, DataType::Int64);
    let v: Vec<Option<i64>> = a.as_primitive::<Int64Type>().iter().collect();
    assert_eq!(v, [Some(1), None, Some(3)]);
    let (t, a) = column(&read.attributes_ipc, "small");
    assert_eq!(t, DataType::Int32);
    let v: Vec<Option<i32>> = a.as_primitive::<Int32Type>().iter().collect();
    assert_eq!(v, [Some(-7), Some(8), None]);
    let (_, a) = column(&read.attributes_ipc, "name");
    let v: Vec<Option<&str>> = a.as_string::<i32>().iter().collect();
    assert_eq!(v, [Some("a"), Some("b c"), None]);
    let (t, a) = column(&read.attributes_ipc, "score");
    assert_eq!(t, DataType::Float64);
    let v: Vec<Option<f64>> = a.as_primitive::<Float64Type>().iter().collect();
    assert_eq!(v, [Some(1.5), None, Some(-3.25)]);
    let (_, a) = column(&read.attributes_ipc, "flag");
    let v: Vec<Option<bool>> = a.as_boolean().iter().collect();
    assert_eq!(v, [Some(true), None, Some(false)]);
    let (t, a) = column(&read.attributes_ipc, "day");
    assert_eq!(t, DataType::Date32);
    let v: Vec<Option<i32>> = a.as_primitive::<Date32Type>().iter().collect();
    assert_eq!(v, [Some(0), Some(19_000), None]);
}

#[test]
fn without_a_crs_no_prj_is_written_and_a_stale_one_is_removed() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(&file, &sample_points(Some("EPSG:3857")), &id_ipc(3), None).unwrap();
    assert!(dir.path().join("a.prj").exists());
    write_shapefile(&file, &sample_points(None), &id_ipc(3), None).unwrap();
    assert!(!dir.path().join("a.prj").exists());
    assert_eq!(read_shapefile(&file, None).unwrap().geometry.crs(), None);
}

#[test]
fn unknown_crs_codes_write_no_prj_and_warn() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let warnings =
        write_shapefile(&file, &sample_points(Some("EPSG:999999")), &id_ipc(3), None).unwrap();
    assert!(!dir.path().join("a.prj").exists());
    assert!(
        warnings.iter().any(|w| w.contains("EPSG:999999")),
        "{warnings:?}"
    );
}

#[test]
fn wkt_crs_strings_are_kept_verbatim() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let custom =
        r#"PROJCS["Custom_Thing",GEOGCS["GCS_Custom"],PROJECTION["Lambert_Conformal_Conic"]]"#;
    write_shapefile(&file, &sample_points(Some(custom)), &id_ipc(3), None).unwrap();
    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(read.geometry.crs().as_deref(), Some(custom));
}

#[test]
fn polygons_with_holes_and_multipolygons_round_trip_as_polygon_shapes() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "poly.shp");
    let geometry = GeoColumn::from_wkt(
        &[
            Some("POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0), (1 1, 2 1, 2 2, 1 1))"),
            Some("MULTIPOLYGON (((10 10, 10 11, 11 11, 10 10)), ((20 20, 20 21, 21 21, 20 20)))"),
            None,
        ],
        Some("EPSG:4326"),
    )
    .unwrap();
    write_shapefile(&file, &geometry, &id_ipc(3), None).unwrap();

    // Every row is stored as a Polygon shape; the independent reader sees valid shapes.
    let shapes = shapefile::ShapeReader::from_path(&file)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(shapes.len(), 3);
    assert!(matches!(shapes[0], shapefile::Shape::Polygon(_)));
    assert!(matches!(shapes[1], shapefile::Shape::Polygon(_)));
    assert!(matches!(shapes[2], shapefile::Shape::NullShape));
    if let shapefile::Shape::Polygon(polygon) = &shapes[0] {
        let kinds: Vec<bool> = polygon
            .rings()
            .iter()
            .map(|r| matches!(r, shapefile::PolygonRing::Outer(_)))
            .collect();
        assert_eq!(
            kinds,
            [true, false],
            "exterior clockwise, hole counter-clockwise"
        );
    }

    let read = read_shapefile(&file, None).unwrap();
    let rows = wkt(&read.geometry);
    assert_eq!(rows[2], None);
    assert!(rows[0].as_deref().unwrap().starts_with("POLYGON("));
    assert!(rows[1].as_deref().unwrap().starts_with("MULTIPOLYGON("));
    // Orientation changes, the shape does not: areas agree.
    assert_eq!(read.geometry.area(), geometry.area());
}

#[test]
fn lines_and_multipoints_round_trip() {
    let dir = TempDir::new().unwrap();
    let lines = GeoColumn::from_wkt(
        &[
            Some("LINESTRING (0 0, 1 1, 2 0)"),
            Some("MULTILINESTRING ((0 0, 1 1), (5 5, 6 6, 7 5))"),
        ],
        None,
    )
    .unwrap();
    let file = path(&dir, "lines.shp");
    write_shapefile(&file, &lines, &id_ipc(2), None).unwrap();
    let read = read_shapefile(&file, None).unwrap();
    let rows = wkt(&read.geometry);
    assert_eq!(rows[0].as_deref(), Some("LINESTRING(0 0,1 1,2 0)"));
    assert_eq!(
        rows[1].as_deref(),
        Some("MULTILINESTRING((0 0,1 1),(5 5,6 6,7 5))")
    );

    let multi = GeoColumn::from_wkt(&[Some("MULTIPOINT (1 2, 3 4)"), None], None).unwrap();
    let file = path(&dir, "multi.shp");
    write_shapefile(&file, &multi, &id_ipc(2), None).unwrap();
    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&multi));
}

#[test]
fn mixed_geometry_types_are_rejected() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "mixed.shp");
    let geometry =
        GeoColumn::from_wkt(&[Some("POINT (1 2)"), Some("LINESTRING (0 0, 1 1)")], None).unwrap();
    let error = write_shapefile(&file, &geometry, &id_ipc(2), None).unwrap_err();
    assert!(error.contains("single geometry type"), "{error}");
    assert!(!dir.path().join("mixed.shp").exists());

    let collection = GeoColumn::from_wkt(
        &[Some(
            "GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))",
        )],
        None,
    )
    .unwrap();
    let error = write_shapefile(&file, &collection, &id_ipc(1), None).unwrap_err();
    assert!(error.contains("GeometryCollection"), "{error}");
}

#[test]
fn frames_without_attributes_get_a_fid_column_like_gdal() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let empty = crate::attributes::ipc_from_columns(Vec::new(), Vec::new(), 3).unwrap();
    // An attribute-less table has no row count of its own, so the writer takes the geometry's.
    write_shapefile(&file, &sample_points(None), &empty, None).unwrap();
    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["FID"]);
    let (_, a) = column(&read.attributes_ipc, "FID");
    let v: Vec<Option<i32>> = a.as_primitive::<Int32Type>().iter().collect();
    assert_eq!(v, [Some(0), Some(1), Some(2)]);
}

#[test]
fn long_names_are_truncated_and_reported() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let attrs = ipc(vec![
        (
            "abcdefghijkl",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
        ),
        ("abcdefghijkm", Arc::new(Int64Array::from(vec![4, 5, 6]))),
    ]);
    let warnings = write_shapefile(&file, &sample_points(None), &attrs, None).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("abcdefgh_1")),
        "{warnings:?}"
    );
    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["abcdefghij", "abcdefgh_1"]);
}

#[test]
fn strings_longer_than_254_bytes_are_truncated_with_a_warning() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let long = "x".repeat(300);
    let attrs = ipc(vec![(
        "s",
        Arc::new(StringArray::from(vec![
            Some(long.as_str()),
            Some("ok"),
            None,
        ])) as ArrayRef,
    )]);
    let warnings = write_shapefile(&file, &sample_points(None), &attrs, None).unwrap();
    assert!(warnings.iter().any(|w| w.contains("254")), "{warnings:?}");
    let read = read_shapefile(&file, None).unwrap();
    let (_, a) = column(&read.attributes_ipc, "s");
    assert_eq!(a.as_string::<i32>().value(0).len(), 254);
}

#[test]
fn binary_columns_are_rejected() {
    let dir = TempDir::new().unwrap();
    let attrs = ipc(vec![(
        "b",
        Arc::new(BinaryArray::from_vec(vec![b"a", b"b", b"c"])) as ArrayRef,
    )]);
    let error =
        write_shapefile(&path(&dir, "a.shp"), &sample_points(None), &attrs, None).unwrap_err();
    assert!(error.contains("binary"), "{error}");
}

#[test]
fn row_count_mismatch_is_an_error() {
    let dir = TempDir::new().unwrap();
    let error =
        write_shapefile(&path(&dir, "a.shp"), &sample_points(None), &id_ipc(5), None).unwrap_err();
    assert!(error.contains("rows"), "{error}");
}

// -- encodings --------------------------------------------------------------------------

fn text_ipc(value: &str) -> Vec<u8> {
    ipc(vec![(
        "t",
        Arc::new(StringArray::from(vec![
            Some(value),
            Some(value),
            Some(value),
        ])) as ArrayRef,
    )])
}

#[test]
fn utf8_is_the_default_and_a_cpg_is_written() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(&file, &sample_points(None), &text_ipc("é"), None).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.cpg")).unwrap(),
        "UTF-8"
    );
    let dbf = std::fs::read(dir.path().join("a.dbf")).unwrap();
    assert!(dbf.windows(2).any(|w| w == [0xC3, 0xA9]));
    let read = read_shapefile(&file, None).unwrap();
    let (_, a) = column(&read.attributes_ipc, "t");
    assert_eq!(a.as_string::<i32>().value(0), "é");
}

#[test]
fn latin1_files_are_written_and_read_through_their_cpg() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(
        &file,
        &sample_points(None),
        &text_ipc("é"),
        Some("ISO-8859-1"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.cpg")).unwrap(),
        "ISO-8859-1"
    );
    let dbf = std::fs::read(dir.path().join("a.dbf")).unwrap();
    assert!(dbf.contains(&0xE9) && !dbf.windows(2).any(|w| w == [0xC3, 0xA9]));
    let read = read_shapefile(&file, None).unwrap();
    let (_, a) = column(&read.attributes_ipc, "t");
    assert_eq!(a.as_string::<i32>().value(0), "é");
}

#[test]
fn an_explicit_encoding_overrides_the_cpg() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(&file, &sample_points(None), &text_ipc("é"), None).unwrap();
    let read = read_shapefile(&file, Some("latin1")).unwrap();
    let (_, a) = column(&read.attributes_ipc, "t");
    assert_eq!(a.as_string::<i32>().value(0), "Ã©");
}

#[test]
fn cp1252_maps_the_c1_range() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(
        &file,
        &sample_points(None),
        &text_ipc("€ “x”"),
        Some("cp1252"),
    )
    .unwrap();
    let dbf = std::fs::read(dir.path().join("a.dbf")).unwrap();
    assert!(dbf.contains(&0x80));
    let read = read_shapefile(&file, None).unwrap();
    let (_, a) = column(&read.attributes_ipc, "t");
    assert_eq!(a.as_string::<i32>().value(0), "€ “x”");
}

#[test]
fn unsupported_encodings_are_errors() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    let error =
        write_shapefile(&file, &sample_points(None), &id_ipc(3), Some("EBCDIC")).unwrap_err();
    assert!(error.contains("EBCDIC"), "{error}");
    write_shapefile(&file, &sample_points(None), &id_ipc(3), None).unwrap();
    assert!(read_shapefile(&file, Some("EBCDIC")).is_err());
}

// -- zip, info, errors ------------------------------------------------------------------

#[test]
fn zipped_shapefiles_round_trip() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "bundle.zip");
    let geometry = sample_points(Some("EPSG:4326"));
    write_shapefile(&file, &geometry, &sample_ipc(), None).unwrap();
    let archive = zip::ZipArchive::new(std::fs::File::open(&file).unwrap()).unwrap();
    let mut members: Vec<String> = archive
        .file_names()
        .map(|n| n.unwrap().into_owned())
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
            "bundle.cpg",
            "bundle.dbf",
            "bundle.prj",
            "bundle.shp",
            "bundle.shx"
        ]
    );
    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&geometry));
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
}

#[test]
fn a_zip_without_a_shapefile_is_an_error() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "empty.zip");
    {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        writer
            .start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"hi").unwrap();
        writer.finish().unwrap();
    }
    let error = read_shapefile(&file, None).err().unwrap();
    assert!(error.contains(".shp"), "{error}");
}

#[test]
fn info_reports_type_count_bounds_fields_and_crs() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(
        &file,
        &sample_points(Some("EPSG:4326")),
        &sample_ipc(),
        None,
    )
    .unwrap();
    let info = shapefile_info(&file, None).unwrap();
    assert_eq!(info.name, "a");
    assert_eq!(info.driver, "ESRI Shapefile");
    assert_eq!(info.geometry_type.as_deref(), Some("Point"));
    assert_eq!(info.features, 3);
    assert_eq!(info.bounds, Some([1.0, -4.0, 3.5, 2.0]));
    assert_eq!(info.crs.as_deref(), Some("EPSG:4326"));
    assert_eq!(info.encoding, "UTF-8");
    let fields: Vec<(&str, &str)> = info
        .fields
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            ("id", "int64"),
            ("small", "int32"),
            ("name", "object"),
            ("score", "float64"),
            ("flag", "bool"),
            ("day", "datetime64[D]")
        ]
    );
}

#[test]
fn missing_and_garbage_files_are_errors() {
    let dir = TempDir::new().unwrap();
    assert!(read_shapefile(&path(&dir, "nope.shp"), None).is_err());
    std::fs::write(dir.path().join("bad.shp"), b"not a shapefile").unwrap();
    assert!(read_shapefile(&path(&dir, "bad.shp"), None).is_err());
}

#[test]
fn a_shapefile_without_dbf_reads_geometry_only() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "a.shp");
    write_shapefile(&file, &sample_points(None), &id_ipc(3), None).unwrap();
    std::fs::remove_file(dir.path().join("a.dbf")).unwrap();
    let read = read_shapefile(&file, None).unwrap();
    assert_eq!(read.geometry.len(), 3);
    assert!(names(&read.attributes_ipc).is_empty());
}
