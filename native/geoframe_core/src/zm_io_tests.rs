//! Z/M through the file formats: GeoJSON, FlatGeobuf, Shapefile, GeoPackage and GeoParquet.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array};
use tempfile::TempDir;

use crate::test_support::{ipc, path, wkt};
use crate::GeoColumn;

/// An ISO WKB `LINESTRING Z` (little endian); lets a test hold a NaN Z that WKT cannot spell.
fn line_z_wkb(points: &[[f64; 3]]) -> Vec<u8> {
    let mut out = vec![1];
    out.extend_from_slice(&1002u32.to_le_bytes());
    out.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for value in points.iter().flatten() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

/// One `n` column counting the rows.
fn no_attributes(rows: usize) -> Vec<u8> {
    let n: ArrayRef = Arc::new(Int64Array::from_iter_values(0..rows as i64));
    ipc(vec![("n", n)])
}

// -- GeoJSON ------------------------------------------------------------------------------

mod geojson {
    use super::*;
    use crate::{read_geojson, read_geojson_str, write_geojson};

    fn read(text: &str) -> GeoColumn {
        read_geojson_str(text, None).unwrap().geometry
    }

    fn feature(geometry: &str) -> String {
        format!(
            r#"{{"type":"FeatureCollection","features":[{{"type":"Feature","properties":{{}},"geometry":{geometry}}}]}}"#
        )
    }

    #[test]
    fn three_value_positions_read_as_z() {
        let c = read(&feature(
            r#"{"type":"LineString","coordinates":[[0,0,1],[1,1,2.5]]}"#,
        ));
        assert_eq!(c.has_z(), [true]);
        assert_eq!(c.has_m(), [false]);
        assert_eq!(wkt(&c), [Some("LINESTRING Z(0 0 1,1 1 2.5)".to_string())]);
    }

    #[test]
    fn every_geometry_family_keeps_z() {
        let c = read(
            r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[1,2,3]}},
            {"type":"Feature","geometry":{"type":"MultiPoint","coordinates":[[0,0,1],[1,1,2]]}},
            {"type":"Feature","geometry":{"type":"MultiLineString","coordinates":[[[0,0,1],[1,1,2]],[[2,2,3],[3,3,4]]]}},
            {"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0,1],[4,0,1],[4,4,1],[0,0,1]]]}},
            {"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[0,0,1],[4,0,1],[4,4,1],[0,0,1]]]]}},
            {"type":"Feature","geometry":{"type":"GeometryCollection","geometries":[
                {"type":"Point","coordinates":[1,2,3]},{"type":"LineString","coordinates":[[0,0,1],[1,1,2]]}]}}
            ]}"#,
        );
        assert_eq!(c.has_z(), [true; 6]);
        assert_eq!(c.has_m(), [false; 6]);
        assert_eq!(
            wkt(&c)[5].as_deref(),
            Some("GEOMETRYCOLLECTION Z(POINT Z(1 2 3),LINESTRING Z(0 0 1,1 1 2))")
        );
    }

    #[test]
    fn two_value_positions_stay_two_dimensional() {
        let c = read(&feature(r#"{"type":"Point","coordinates":[1,2]}"#));
        assert_eq!(c.has_z(), [false]);
    }

    #[test]
    fn ordinates_beyond_the_third_are_ignored() {
        // GDAL reads at most three members of a position; a fourth would be an M in no spec.
        let c = read(&feature(
            r#"{"type":"LineString","coordinates":[[0,0,1,9],[1,1,2,8,7]]}"#,
        ));
        assert_eq!(c.has_m(), [false]);
        assert_eq!(wkt(&c), [Some("LINESTRING Z(0 0 1,1 1 2)".to_string())]);
    }

    #[test]
    fn positions_without_z_in_a_3d_geometry_get_zero() {
        let c = read(&feature(
            r#"{"type":"LineString","coordinates":[[0,0,1],[1,1],[2,2,3]]}"#,
        ));
        assert_eq!(c.has_z(), [true]);
        let wkb = c.to_wkb().unwrap().remove(0).unwrap();
        // header (9 bytes) then three XYZ coordinates of 24 bytes
        let z = |i: usize| {
            f64::from_le_bytes(wkb[9 + 24 * i + 16..9 + 24 * i + 24].try_into().unwrap())
        };
        assert_eq!((z(0), z(2)), (1.0, 3.0));
        // GDAL reads a two-value position in a 3D geometry as Z = 0
        assert_eq!(z(1), 0.0);
    }

    #[test]
    fn collection_members_are_promoted_to_the_widest_dimension() {
        let c = read(&feature(
            r#"{"type":"GeometryCollection","geometries":[
                {"type":"Point","coordinates":[1,2,3]},{"type":"Point","coordinates":[4,5]}]}"#,
        ));
        assert_eq!(c.has_z(), [true]);
        assert_eq!(
            wkt(&c),
            [Some(
                "GEOMETRYCOLLECTION Z(POINT Z(1 2 3),POINT Z(4 5 0))".to_string()
            )]
        );
    }

    #[test]
    fn rows_may_differ_in_dimension() {
        let c = read(
            r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[1,2,3]}},
            {"type":"Feature","geometry":{"type":"Point","coordinates":[4,5]}}]}"#,
        );
        assert_eq!(c.has_z(), [true, false]);
    }

    fn written(dir: &TempDir, geometry: &GeoColumn) -> serde_json::Value {
        let file = path(dir, "out.geojson");
        write_geojson(&file, geometry, &no_attributes(geometry.len()), true).unwrap();
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap()
    }

    fn coordinates(doc: &serde_json::Value, row: usize) -> serde_json::Value {
        doc["features"][row]["geometry"]["coordinates"].clone()
    }

    #[test]
    fn z_is_written_as_the_third_ordinate() {
        let dir = TempDir::new().unwrap();
        let g = col(&[
            Some("POINT Z (1 2 3)"),
            Some("POLYGON Z ((0 0 1, 4 0 1, 4 4 1, 0 0 1))"),
            Some("POINT (7 8)"),
        ]);
        let doc = written(&dir, &g);
        assert_eq!(coordinates(&doc, 0), serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(
            coordinates(&doc, 1),
            serde_json::json!([[
                [0.0, 0.0, 1.0],
                [4.0, 0.0, 1.0],
                [4.0, 4.0, 1.0],
                [0.0, 0.0, 1.0]
            ]])
        );
        assert_eq!(coordinates(&doc, 2), serde_json::json!([7.0, 8.0]));
    }

    #[test]
    fn m_is_dropped_and_zm_keeps_only_z() {
        let dir = TempDir::new().unwrap();
        let g = col(&[
            Some("POINT M (1 2 9)"),
            Some("LINESTRING ZM (0 0 1 5, 1 1 2 6)"),
        ]);
        let doc = written(&dir, &g);
        assert_eq!(coordinates(&doc, 0), serde_json::json!([1.0, 2.0]));
        assert_eq!(
            coordinates(&doc, 1),
            serde_json::json!([[0.0, 0.0, 1.0], [1.0, 1.0, 2.0]])
        );
    }

    #[test]
    fn nan_z_is_written_as_a_two_value_position() {
        let dir = TempDir::new().unwrap();
        let g = GeoColumn::from_wkb(
            &[Some(line_z_wkb(&[[0.0, 0.0, 1.0], [1.0, 1.0, f64::NAN]]))],
            None,
        )
        .unwrap();
        let doc = written(&dir, &g);
        assert_eq!(
            coordinates(&doc, 0),
            serde_json::json!([[0.0, 0.0, 1.0], [1.0, 1.0]])
        );
    }

    #[test]
    fn collections_write_z_for_every_member() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some(
            "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))",
        )]);
        let doc = written(&dir, &g);
        let members = &doc["features"][0]["geometry"]["geometries"];
        assert_eq!(
            members[0]["coordinates"],
            serde_json::json!([1.0, 2.0, 3.0])
        );
        assert_eq!(
            members[1]["coordinates"],
            serde_json::json!([[0.0, 0.0, 1.0], [1.0, 1.0, 2.0]])
        );
    }

    #[test]
    fn z_round_trips_through_a_file() {
        let dir = TempDir::new().unwrap();
        let file = path(&dir, "rt.geojson");
        let g = col(&[
            Some("POINT Z (1 2 3)"),
            None,
            Some("MULTILINESTRING Z ((0 0 1, 1 1 2), (2 2 3, 3 3 4))"),
            Some("POINT (5 6)"),
        ]);
        write_geojson(&file, &g, &no_attributes(4), true).unwrap();
        let back = read_geojson(&file).unwrap().geometry;
        assert_eq!(wkt(&back), wkt(&g));
        assert_eq!(back.has_z(), [true, false, true, false]);
    }

    #[test]
    fn the_geometry_column_exposes_z_in_geojson_text() {
        let g = col(&[Some("POINT Z (1 2 3)"), Some("POINT ZM (1 2 3 4)")]);
        let out: Vec<serde_json::Value> = g
            .to_geojson()
            .into_iter()
            .map(|t| serde_json::from_str(&t.unwrap()).unwrap())
            .collect();
        assert_eq!(out[0]["coordinates"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(out[1]["coordinates"], serde_json::json!([1.0, 2.0, 3.0]));
    }
}

// -- FlatGeobuf ---------------------------------------------------------------------------

mod flatgeobuf {
    use super::*;
    use crate::{read_flatgeobuf, write_flatgeobuf};

    fn header_flags(file: &str) -> (bool, bool) {
        let input = std::io::BufReader::new(std::fs::File::open(file).unwrap());
        let reader = ::flatgeobuf::FgbReader::open(input).unwrap();
        (reader.header().has_z(), reader.header().has_m())
    }

    fn round_trip(
        dir: &TempDir,
        name: &str,
        geometry: &GeoColumn,
        index: bool,
    ) -> (String, GeoColumn) {
        let file = path(dir, name);
        write_flatgeobuf(
            &file,
            geometry,
            &no_attributes(geometry.len()),
            "layer",
            index,
        )
        .unwrap();
        let back = read_flatgeobuf(&file, None).unwrap().geometry;
        (file, back)
    }

    #[test]
    fn z_sets_the_header_flag_and_round_trips_every_family() {
        let dir = TempDir::new().unwrap();
        for (i, rows) in [
            vec![Some("POINT Z (1 2 3)"), None, Some("POINT Z (4 5 6)")],
            vec![Some("LINESTRING Z (0 0 1, 1 1 2)")],
            vec![Some(
                "POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1), (1 1 5, 2 1 5, 2 2 5, 1 1 5))",
            )],
            vec![Some("MULTIPOINT Z ((0 0 1), (1 1 2))")],
            vec![Some("MULTILINESTRING Z ((0 0 1, 1 1 2), (2 2 3, 3 3 4))")],
            vec![Some(
                "MULTIPOLYGON Z (((0 0 1, 4 0 1, 4 4 1, 0 0 1)), ((9 9 7, 9 8 7, 8 8 7, 9 9 7)))",
            )],
            vec![Some(
                "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))",
            )],
        ]
        .into_iter()
        .enumerate()
        {
            let g = col(&rows);
            let (file, back) = round_trip(&dir, &format!("z{i}.fgb"), &g, false);
            assert_eq!(header_flags(&file), (true, false), "{rows:?}");
            assert_eq!(wkt(&back), wkt(&g), "{rows:?}");
            assert_eq!(back.has_z(), g.has_z());
        }
    }

    #[test]
    fn m_and_zm_round_trip_with_their_header_flags() {
        let dir = TempDir::new().unwrap();
        for (name, rows, flags) in [
            (
                "m",
                vec![
                    Some("LINESTRING M (0 0 1, 1 1 2)"),
                    Some("LINESTRING M (2 2 3, 3 3 4)"),
                ],
                (false, true),
            ),
            (
                "zm",
                vec![Some("POLYGON ZM ((0 0 1 5, 4 0 2 6, 4 4 3 7, 0 0 1 5))")],
                (true, true),
            ),
            ("pm", vec![Some("POINT M (1 2 7)")], (false, true)),
            ("pzm", vec![Some("POINT ZM (1 2 3 4)")], (true, true)),
        ] {
            let g = col(&rows);
            let (file, back) = round_trip(&dir, &format!("{name}.fgb"), &g, false);
            assert_eq!(header_flags(&file), flags, "{name}");
            assert_eq!(wkt(&back), wkt(&g), "{name}");
            assert_eq!(
                (back.has_z(), back.has_m()),
                (g.has_z(), g.has_m()),
                "{name}"
            );
        }
    }

    #[test]
    fn a_two_dimensional_file_has_no_flags() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT (1 2)")]);
        let (file, back) = round_trip(&dir, "plain.fgb", &g, true);
        assert_eq!(header_flags(&file), (false, false));
        assert_eq!(back.has_z(), [false]);
    }

    #[test]
    fn rows_of_different_dimensions_are_promoted_to_the_widest() {
        let dir = TempDir::new().unwrap();
        let g = col(&[
            Some("POINT Z (1 2 3)"),
            Some("POINT (4 5)"),
            Some("POINT M (6 7 8)"),
        ]);
        let (file, back) = round_trip(&dir, "mix.fgb", &g, false);
        assert_eq!(header_flags(&file), (true, true));
        assert_eq!(back.has_z(), [true; 3]);
        assert_eq!(back.has_m(), [true; 3]);
        let z = back.z();
        assert_eq!(z[0], Some(3.0));
        // a row without Z reads as Z = 0, as GDAL reads a 2D row of a Z layer
        assert_eq!(z[1], Some(0.0));
        assert_eq!(z[2], Some(0.0));
        assert_eq!(back.m()[2], Some(8.0));
    }

    #[test]
    fn indexed_files_and_bbox_reads_keep_z() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT Z (1 1 10)"), Some("POINT Z (50 50 20)")]);
        let file = path(&dir, "idx.fgb");
        write_flatgeobuf(&file, &g, &no_attributes(2), "layer", true).unwrap();
        let hit = read_flatgeobuf(&file, Some([40.0, 40.0, 60.0, 60.0]))
            .unwrap()
            .geometry;
        assert_eq!(hit.z(), [Some(20.0)]);
    }

    /// A file that declares Z (or M) but whose feature lacks the ordinates must be an error.
    fn declared_but_absent(has_z: bool, has_m: bool) -> String {
        use ::flatgeobuf::{FgbWriter, FgbWriterOptions, GeometryType};
        let options = FgbWriterOptions {
            write_index: false,
            detect_type: false,
            has_z,
            has_m,
            ..Default::default()
        };
        let mut writer =
            FgbWriter::create_with_options("l", GeometryType::LineString, options).unwrap();
        let line = geo::Geometry::LineString(geo::LineString::from(vec![(0.0, 0.0), (1.0, 1.0)]));
        writer.add_feature_geom(line, |_| {}).unwrap();
        let mut bytes = Vec::new();
        writer.write(&mut bytes).unwrap();
        let dir = TempDir::new().unwrap();
        let file = path(&dir, "bad.fgb");
        std::fs::write(&file, bytes).unwrap();
        let result = read_flatgeobuf(&file, None);
        result
            .err()
            .expect("an error, not a panic or a silent 2D read")
    }

    #[test]
    fn declared_ordinates_missing_from_a_feature_are_an_error() {
        for (z, m) in [(true, false), (false, true), (true, true)] {
            let message = declared_but_absent(z, m);
            assert!(message.contains("ordinate"), "{message}");
        }
    }
}

// -- Shapefile ----------------------------------------------------------------------------

mod shapefile {
    use super::*;
    use crate::{read_shapefile, shapefile_info, write_shapefile};

    fn write(dir: &TempDir, name: &str, geometry: &GeoColumn) -> String {
        let file = path(dir, name);
        write_shapefile(&file, geometry, &no_attributes(geometry.len()), None).unwrap();
        file
    }

    fn shape_type(file: &str) -> i32 {
        let bytes = std::fs::read(file).unwrap();
        i32::from_le_bytes(bytes[32..36].try_into().unwrap())
    }

    /// `[xmin, ymin, xmax, ymax, zmin, zmax, mmin, mmax]` of the `.shp` header.
    fn header_ranges(file: &str) -> [f64; 8] {
        let bytes = std::fs::read(file).unwrap();
        let mut out = [0.0; 8];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = f64::from_le_bytes(bytes[36 + 8 * i..44 + 8 * i].try_into().unwrap());
        }
        out
    }

    #[test]
    fn z_geometries_use_the_z_shape_types() {
        let dir = TempDir::new().unwrap();
        for (name, rows, expected) in [
            ("pt", vec![Some("POINT Z (1 2 3)"), None], 11),
            ("ln", vec![Some("LINESTRING Z (0 0 1, 1 1 2)")], 13),
            ("mln", vec![Some("MULTILINESTRING Z ((0 0 1, 1 1 2), (2 2 3, 3 3 4))")], 13),
            ("pg", vec![Some("POLYGON Z ((0 0 1, 0 4 2, 4 4 3, 4 0 4, 0 0 1))")], 15),
            ("mpg", vec![Some("MULTIPOLYGON Z (((0 0 1, 0 1 1, 1 1 1, 0 0 1)), ((5 5 2, 5 6 2, 6 6 2, 5 5 2)))")], 15),
            ("mp", vec![Some("MULTIPOINT Z ((0 0 1), (1 1 2))")], 18),
        ] {
            let g = col(&rows);
            let file = write(&dir, &format!("{name}.shp"), &g);
            assert_eq!(shape_type(&file), expected, "{name}");
            let back = read_shapefile(&file, None).unwrap().geometry;
            assert_eq!(wkt(&back), wkt(&g), "{name}");
            assert_eq!(back.has_z(), g.has_z(), "{name}");
        }
    }

    #[test]
    fn m_only_geometries_use_the_m_shape_types() {
        let dir = TempDir::new().unwrap();
        for (name, rows, expected) in [
            ("pt", vec![Some("POINT M (1 2 3)")], 21),
            ("ln", vec![Some("LINESTRING M (0 0 1, 1 1 2)")], 23),
            (
                "pg",
                vec![Some("POLYGON M ((0 0 1, 0 4 2, 4 4 3, 4 0 4, 0 0 1))")],
                25,
            ),
            ("mp", vec![Some("MULTIPOINT M ((0 0 1), (1 1 2))")], 28),
        ] {
            let g = col(&rows);
            let file = write(&dir, &format!("{name}.shp"), &g);
            assert_eq!(shape_type(&file), expected, "{name}");
            let back = read_shapefile(&file, None).unwrap().geometry;
            assert_eq!(wkt(&back), wkt(&g), "{name}");
            assert_eq!(back.has_z(), g.has_z(), "{name}");
            assert_eq!(back.has_m(), g.has_m(), "{name}");
        }
    }

    #[test]
    fn zm_geometries_use_z_types_with_an_m_section() {
        let dir = TempDir::new().unwrap();
        for (name, rows) in [
            ("pt", vec![Some("POINT ZM (1 2 3 4)")]),
            ("ln", vec![Some("LINESTRING ZM (0 0 1 5, 1 1 2 6)")]),
            (
                "pg",
                vec![Some(
                    "POLYGON ZM ((0 0 1 5, 0 4 2 6, 4 4 3 7, 4 0 4 8, 0 0 1 5))",
                )],
            ),
            ("mp", vec![Some("MULTIPOINT ZM ((0 0 1 5), (1 1 2 6))")]),
        ] {
            let g = col(&rows);
            let file = write(&dir, &format!("{name}.shp"), &g);
            assert!(matches!(shape_type(&file), 11 | 13 | 15 | 18), "{name}");
            let back = read_shapefile(&file, None).unwrap().geometry;
            assert_eq!(wkt(&back), wkt(&g), "{name}");
            assert_eq!(back.has_z(), g.has_z(), "{name}");
            assert_eq!(back.has_m(), g.has_m(), "{name}");
        }
    }

    #[test]
    fn header_carries_the_z_and_m_ranges() {
        let dir = TempDir::new().unwrap();
        let g = col(&[
            Some("LINESTRING ZM (0 0 1 5, 1 1 9 6)"),
            Some("LINESTRING ZM (2 2 -3 7, 3 3 4 -8)"),
        ]);
        let file = write(&dir, "r.shp", &g);
        assert_eq!(
            header_ranges(&file),
            [0.0, 0.0, 3.0, 3.0, -3.0, 9.0, -8.0, 7.0]
        );
        let shx = file.replace(".shp", ".shx");
        let bytes = std::fs::read(shx).unwrap();
        let mut ranges = [0.0; 4];
        for (i, slot) in ranges.iter_mut().enumerate() {
            *slot = f64::from_le_bytes(bytes[68 + 8 * i..76 + 8 * i].try_into().unwrap());
        }
        assert_eq!(ranges, [-3.0, 9.0, -8.0, 7.0]);
    }

    #[test]
    fn a_z_only_file_has_no_m_section_and_zero_m_range() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT Z (1 2 3)")]);
        let file = write(&dir, "zonly.shp", &g);
        let bytes = std::fs::read(&file).unwrap();
        // type + x + y + z = 28 bytes of content (GDAL writes the same)
        assert_eq!(i32::from_be_bytes(bytes[104..108].try_into().unwrap()), 14);
        assert_eq!(&header_ranges(&file)[4..], [3.0, 3.0, 0.0, 0.0]);
    }

    #[test]
    fn two_dimensional_files_are_unchanged() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT (1 2)")]);
        let file = write(&dir, "plain.shp", &g);
        assert_eq!(shape_type(&file), 1);
        assert_eq!(&header_ranges(&file)[4..], [0.0; 4]);
    }

    #[test]
    fn rows_without_z_in_a_z_file_read_back_with_zero() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT Z (1 2 3)"), Some("POINT (4 5)"), None]);
        let file = write(&dir, "mix.shp", &g);
        assert_eq!(shape_type(&file), 11);
        let back = read_shapefile(&file, None).unwrap().geometry;
        assert_eq!(back.has_z(), [true, true, false]);
        // GDAL writes Z = 0 for a 2D row of a Z shapefile
        assert_eq!(back.z(), [Some(3.0), Some(0.0), None]);
    }

    #[test]
    fn nan_m_is_no_data_and_a_shape_without_measures_reads_as_z() {
        let dir = TempDir::new().unwrap();
        // Row 0 has measures; row 1 is 2D+Z so its M is no data and it reads back as Z only.
        let g = col(&[Some("POINT ZM (1 2 3 4)"), Some("POINT Z (5 6 7)")]);
        let file = write(&dir, "nodata.shp", &g);
        let back = read_shapefile(&file, None).unwrap().geometry;
        assert_eq!(back.has_m(), [true, false]);
        assert_eq!(back.m(), [Some(4.0), None]);
        assert_eq!(back.z(), [Some(3.0), Some(7.0)]);
    }

    #[test]
    fn ring_orientation_moves_z_with_its_vertices() {
        let dir = TempDir::new().unwrap();
        // Counter-clockwise exterior: the writer reverses it, and must reverse Z with it.
        let g = col(&[Some("POLYGON Z ((0 0 10, 4 0 20, 4 4 30, 0 4 40, 0 0 10))")]);
        let file = write(&dir, "ring.shp", &g);
        let back = read_shapefile(&file, None).unwrap().geometry;
        let expect = col(&[Some("POLYGON Z ((0 0 10, 0 4 40, 4 4 30, 4 0 20, 0 0 10))")]);
        assert_eq!(wkt(&back), wkt(&expect));
    }

    #[test]
    fn info_names_the_z_and_m_types() {
        let dir = TempDir::new().unwrap();
        let z = write(&dir, "iz.shp", &col(&[Some("LINESTRING Z (0 0 1, 1 1 2)")]));
        assert_eq!(
            shapefile_info(&z, None).unwrap().geometry_type.as_deref(),
            Some("LineString Z")
        );
        let m = write(&dir, "im.shp", &col(&[Some("POINT M (0 0 1)")]));
        assert_eq!(
            shapefile_info(&m, None).unwrap().geometry_type.as_deref(),
            Some("Point M")
        );
    }

    #[test]
    fn zipped_z_shapefiles_round_trip() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT ZM (1 2 3 4)")]);
        let file = path(&dir, "z.zip");
        write_shapefile(&file, &g, &no_attributes(1), None).unwrap();
        let back = read_shapefile(&file, None).unwrap().geometry;
        assert_eq!(wkt(&back), wkt(&g));
    }

    /// The `.shp` of a one-polyline-Z file with the point count patched.
    fn patched_point_count(count: i32) -> Result<(), String> {
        let dir = TempDir::new().unwrap();
        let file = write(
            &dir,
            "evil.shp",
            &col(&[Some("LINESTRING Z (0 0 1, 1 1 2)")]),
        );
        let mut bytes = std::fs::read(&file).unwrap();
        // record content starts at 108: type, box (32), numParts, numPoints
        bytes[108 + 4 + 32 + 4..108 + 4 + 32 + 8].copy_from_slice(&count.to_le_bytes());
        std::fs::write(&file, bytes).unwrap();
        read_shapefile(&file, None).map(|_| ())
    }

    #[test]
    fn huge_declared_point_counts_are_errors_not_allocations() {
        assert!(patched_point_count(i32::MAX).is_err());
        assert!(patched_point_count(-1).is_err());
        assert!(patched_point_count(1_000_000).is_err());
    }

    #[test]
    fn a_truncated_z_array_is_an_error() {
        let dir = TempDir::new().unwrap();
        let file = write(
            &dir,
            "cut.shp",
            &col(&[Some("LINESTRING Z (0 0 1, 1 1 2)")]),
        );
        let mut bytes = std::fs::read(&file).unwrap();
        bytes.truncate(bytes.len() - 12);
        std::fs::write(&file, bytes).unwrap();
        assert!(read_shapefile(&file, None).is_err());
    }
}

// -- GeoPackage ---------------------------------------------------------------------------

mod gpkg {
    use super::*;
    use crate::gpkg_io::{decode_geometry_blob, encode_geometry_blob_zm};
    use crate::{gpkg_layers, read_gpkg, write_gpkg, GpkgWriteOptions};
    use rusqlite::Connection;

    const WRITE: GpkgWriteOptions = GpkgWriteOptions {
        append: false,
        spatial_index: true,
    };
    const APPEND: GpkgWriteOptions = GpkgWriteOptions {
        append: true,
        spatial_index: true,
    };

    fn write(dir: &TempDir, name: &str, geometry: &GeoColumn, options: GpkgWriteOptions) -> String {
        let file = path(dir, name);
        write_gpkg(
            &file,
            geometry,
            &no_attributes(geometry.len()),
            "layer",
            options,
        )
        .unwrap();
        file
    }

    /// `(geometry_type_name, z, m)` of the layer in `gpkg_geometry_columns`.
    fn flags(file: &str) -> (String, i64, i64) {
        Connection::open(file)
            .unwrap()
            .query_row(
                "SELECT geometry_type_name, z, m FROM gpkg_geometry_columns WHERE table_name = 'layer'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    fn blobs(file: &str) -> Vec<Option<Vec<u8>>> {
        let conn = Connection::open(file).unwrap();
        let mut statement = conn.prepare("SELECT geom FROM layer ORDER BY fid").unwrap();
        statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn geometry_columns_flags_follow_the_data() {
        let dir = TempDir::new().unwrap();
        for (name, rows, expected) in [
            ("plain", vec![Some("POINT (1 2)")], ("POINT", 0, 0)),
            ("z", vec![Some("POINT Z (1 2 3)"), None], ("POINT", 1, 0)),
            (
                "m",
                vec![Some("LINESTRING M (0 0 1, 1 1 2)")],
                ("LINESTRING", 0, 1),
            ),
            (
                "zm",
                vec![Some("POLYGON ZM ((0 0 1 5, 4 0 2 6, 4 4 3 7, 0 0 1 5))")],
                ("POLYGON", 1, 1),
            ),
            // some rows with and some without: the ordinate is optional (2)
            (
                "opt",
                vec![Some("POINT Z (1 2 3)"), Some("POINT (4 5)")],
                ("POINT", 2, 0),
            ),
            (
                "optm",
                vec![Some("POINT ZM (1 2 3 4)"), Some("POINT Z (4 5 6)")],
                ("POINT", 1, 2),
            ),
        ] {
            let g = col(&rows);
            let file = write(&dir, &format!("{name}.gpkg"), &g, WRITE);
            assert_eq!(
                flags(&file),
                (expected.0.to_string(), expected.1, expected.2),
                "{name}"
            );
        }
    }

    #[test]
    fn layers_report_the_dimension() {
        let dir = TempDir::new().unwrap();
        let file = write(&dir, "l.gpkg", &col(&[Some("POINT ZM (1 2 3 4)")]), WRITE);
        let layers = gpkg_layers(&file).unwrap();
        assert_eq!(layers[0].geometry_type.as_deref(), Some("Point ZM"));
    }

    #[test]
    fn blob_envelopes_carry_the_z_and_m_ranges() {
        let dir = TempDir::new().unwrap();
        let g = col(&[
            Some("POINT (1 2)"),
            Some("LINESTRING Z (0 0 5, 1 1 9)"),
            Some("LINESTRING M (0 0 -1, 1 1 4)"),
            Some("LINESTRING ZM (0 0 5 -1, 3 4 9 4)"),
            None,
        ]);
        let file = write(&dir, "env.gpkg", &g, WRITE);
        let blobs = blobs(&file);
        let indicator = |row: usize| (blobs[row].as_ref().unwrap()[3] >> 1) & 0b111;
        assert_eq!((0..4).map(indicator).collect::<Vec<_>>(), [1, 2, 3, 4]);
        let blob = |row: usize| decode_geometry_blob(blobs[row].as_ref().unwrap()).unwrap();
        assert_eq!(blob(1).z_range, Some([5.0, 9.0]));
        assert_eq!(blob(1).m_range, None);
        assert_eq!(blob(2).z_range, None);
        assert_eq!(blob(2).m_range, Some([-1.0, 4.0]));
        assert_eq!(blob(3).z_range, Some([5.0, 9.0]));
        assert_eq!(blob(3).m_range, Some([-1.0, 4.0]));
        assert_eq!(blob(3).envelope, Some([0.0, 0.0, 3.0, 4.0]));
        assert_eq!(blob(0).z_range, None);
    }

    #[test]
    fn nan_ordinates_do_not_enter_the_ranges() {
        let g = GeoColumn::from_wkb(
            &[Some(line_z_wkb(&[
                [0.0, 0.0, f64::NAN],
                [1.0, 1.0, 7.0],
                [2.0, 2.0, 3.0],
            ]))],
            None,
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let file = write(&dir, "nan.gpkg", &g, WRITE);
        let blobs = blobs(&file);
        assert_eq!(
            decode_geometry_blob(blobs[0].as_ref().unwrap())
                .unwrap()
                .z_range,
            Some([3.0, 7.0])
        );
    }

    #[test]
    fn encoder_picks_the_indicator_from_the_ranges() {
        let wkb = [1, 1, 0, 0, 0];
        for (z, m, indicator, length) in [
            (None, None, 1, 32),
            (Some([1.0, 2.0]), None, 2, 48),
            (None, Some([3.0, 4.0]), 3, 48),
            (Some([1.0, 2.0]), Some([3.0, 4.0]), 4, 64),
        ] {
            let blob = encode_geometry_blob_zm(&wkb, 4326, Some([0.0, 1.0, 2.0, 3.0]), z, m);
            assert_eq!((blob[3] >> 1) & 0b111, indicator);
            assert_eq!(blob.len(), 8 + length + wkb.len());
        }
    }

    #[test]
    fn z_and_m_survive_a_round_trip() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT ZM (1 2 3 4)"), Some("POINT Z (4 5 6)"), None]);
        let file = write(&dir, "rt.gpkg", &g, WRITE);
        let back = read_gpkg(&file, None).unwrap().vector.geometry;
        assert_eq!(wkt(&back), wkt(&g));
    }

    #[test]
    fn appending_a_different_dimension_makes_the_ordinate_optional() {
        let dir = TempDir::new().unwrap();
        let file = write(&dir, "app.gpkg", &col(&[Some("POINT (1 2)")]), WRITE);
        write_gpkg(
            &file,
            &col(&[Some("POINT Z (3 4 5)")]),
            &no_attributes(1),
            "layer",
            APPEND,
        )
        .unwrap();
        assert_eq!(flags(&file), ("POINT".to_string(), 2, 0));

        let file = write(&dir, "app2.gpkg", &col(&[Some("POINT Z (1 2 3)")]), WRITE);
        write_gpkg(
            &file,
            &col(&[Some("POINT Z (3 4 5)")]),
            &no_attributes(1),
            "layer",
            APPEND,
        )
        .unwrap();
        assert_eq!(flags(&file), ("POINT".to_string(), 1, 0));
        write_gpkg(
            &file,
            &col(&[Some("POINT (7 8)")]),
            &no_attributes(1),
            "layer",
            APPEND,
        )
        .unwrap();
        assert_eq!(flags(&file), ("POINT".to_string(), 2, 0));

        let all = read_gpkg(&file, None).unwrap().vector.geometry;
        assert_eq!(all.has_z(), [true, true, false]);
    }
}

// -- GeoParquet and Arrow -----------------------------------------------------------------

mod parquet {
    use super::*;
    use crate::{
        from_arrow_ipc, geoparquet_metadata, read_feather, read_geoparquet,
        read_geoparquet_filtered, to_arrow_ipc, write_feather, write_geoparquet_with,
        FeatherCompression, GeometryEncoding, ParquetOptions,
    };

    fn options(encoding: GeometryEncoding) -> ParquetOptions {
        ParquetOptions {
            geometry_encoding: encoding,
            ..ParquetOptions::default()
        }
    }

    fn write(dir: &TempDir, name: &str, g: &GeoColumn, options: &ParquetOptions) -> String {
        let file = path(dir, name);
        write_geoparquet_with(&file, g, "geometry", &no_attributes(g.len()), options).unwrap();
        file
    }

    fn types(file: &str) -> Vec<String> {
        let meta: serde_json::Value =
            serde_json::from_str(&geoparquet_metadata(file).unwrap()).unwrap();
        let mut types: Vec<String> = meta["columns"]["geometry"]["geometry_types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_string())
            .collect();
        types.sort();
        types
    }

    #[test]
    fn wkb_geometry_types_carry_the_dimension_suffix() {
        let dir = TempDir::new().unwrap();
        for (name, rows, expected) in [
            ("plain", vec![Some("POINT (1 2)")], vec!["Point"]),
            (
                "z",
                vec![Some("POINT Z (1 2 3)"), Some("POINT Z (4 5 6)")],
                vec!["Point Z"],
            ),
            (
                "m",
                vec![Some("LINESTRING M (0 0 1, 1 1 2)")],
                vec!["LineString M"],
            ),
            (
                "zm",
                vec![Some("POLYGON ZM ((0 0 1 5, 4 0 2 6, 4 4 3 7, 0 0 1 5))")],
                vec!["Polygon ZM"],
            ),
            (
                "mixed",
                vec![Some("POINT Z (1 2 3)"), Some("POINT (4 5)"), None],
                vec!["Point", "Point Z"],
            ),
            (
                "multi",
                vec![
                    Some("POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))"),
                    Some("MULTIPOLYGON Z (((0 0 1, 4 0 2, 4 4 3, 0 0 1)))"),
                ],
                vec!["MultiPolygon Z", "Polygon Z"],
            ),
        ] {
            let g = col(&rows);
            let file = write(
                &dir,
                &format!("{name}.parquet"),
                &g,
                &options(GeometryEncoding::Wkb),
            );
            assert_eq!(types(&file), expected, "{name}");
            let back = read_geoparquet(&file, None).unwrap().geometry;
            assert_eq!(wkt(&back), wkt(&g), "{name}");
        }
    }

    #[test]
    fn native_encodings_store_z_and_m() {
        let dir = TempDir::new().unwrap();
        for (name, rows, expected) in [
            (
                "pz",
                vec![Some("POINT Z (1 2 3)"), None, Some("POINT Z (4 5 6)")],
                "Point Z",
            ),
            ("pm", vec![Some("POINT M (1 2 3)")], "Point M"),
            ("pzm", vec![Some("POINT ZM (1 2 3 4)")], "Point ZM"),
            (
                "lz",
                vec![Some("LINESTRING Z (0 0 1, 1 1 2)")],
                "LineString Z",
            ),
            (
                "gz",
                vec![Some("POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))")],
                "Polygon Z",
            ),
            (
                "mpz",
                vec![Some("MULTIPOINT Z ((0 0 1), (1 1 2))")],
                "MultiPoint Z",
            ),
            (
                "mlz",
                vec![Some("MULTILINESTRING Z ((0 0 1, 1 1 2), (2 2 3, 3 3 4))")],
                "MultiLineString Z",
            ),
            (
                "mgz",
                vec![Some("MULTIPOLYGON Z (((0 0 1, 4 0 2, 4 4 3, 0 0 1)))")],
                "MultiPolygon Z",
            ),
        ] {
            let g = col(&rows);
            let file = write(
                &dir,
                &format!("{name}.parquet"),
                &g,
                &options(GeometryEncoding::GeoArrow),
            );
            assert_eq!(types(&file), [expected], "{name}");
            let back = read_geoparquet(&file, None).unwrap().geometry;
            assert_eq!(wkt(&back), wkt(&g), "{name}");
        }
    }

    #[test]
    fn single_and_multi_geometries_with_z_are_promoted_together() {
        let dir = TempDir::new().unwrap();
        for (name, rows, expected) in [
            (
                "poly",
                vec![
                    Some("POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1), (1 1 5, 2 1 5, 2 2 5, 1 1 5))"),
                    Some("MULTIPOLYGON Z (((0 0 1, 4 0 2, 4 4 3, 0 0 1)), ((9 9 7, 9 8 7, 8 8 7, 9 9 7)))"),
                    None,
                ],
                vec![
                    Some("MULTIPOLYGON Z(((0 0 1,4 0 2,4 4 3,0 0 1),(1 1 5,2 1 5,2 2 5,1 1 5)))"),
                    Some("MULTIPOLYGON Z(((0 0 1,4 0 2,4 4 3,0 0 1)),((9 9 7,9 8 7,8 8 7,9 9 7)))"),
                    None,
                ],
            ),
            (
                "line",
                vec![Some("LINESTRING ZM (0 0 1 2, 1 1 2 3)"), Some("MULTILINESTRING ZM ((2 2 3 4, 3 3 4 5))")],
                vec![
                    Some("MULTILINESTRING ZM((0 0 1 2,1 1 2 3))"),
                    Some("MULTILINESTRING ZM((2 2 3 4,3 3 4 5))"),
                ],
            ),
            (
                "point",
                vec![Some("POINT M (1 2 3)"), Some("MULTIPOINT M ((4 5 6), (7 8 9))")],
                vec![Some("MULTIPOINT M((1 2 3))"), Some("MULTIPOINT M((4 5 6),(7 8 9))")],
            ),
        ] {
            let g = col(&rows);
            let file = write(&dir, &format!("{name}.parquet"), &g, &options(GeometryEncoding::GeoArrow));
            let back = read_geoparquet(&file, None).unwrap().geometry;
            assert_eq!(wkt(&back).iter().map(|s| s.as_deref().map(|t| t.replace(' ', ""))).collect::<Vec<_>>(),
                       expected.iter().map(|s| s.map(|t: &str| t.replace(' ', ""))).collect::<Vec<_>>(), "{name}");
        }
    }

    #[test]
    fn native_encoding_still_needs_one_dimension() {
        let g = col(&[Some("POINT Z (1 2 3)"), Some("POINT (4 5)")]);
        let dir = TempDir::new().unwrap();
        let file = path(&dir, "bad.parquet");
        let result = write_geoparquet_with(
            &file,
            &g,
            "geometry",
            &no_attributes(2),
            &options(GeometryEncoding::GeoArrow),
        );
        assert!(result.unwrap_err().contains("dimension"));
    }

    #[test]
    fn the_covering_bbox_stays_two_dimensional() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("POINT Z (1 2 30)"), Some("POINT Z (50 60 7)")]);
        for encoding in [GeometryEncoding::Wkb, GeometryEncoding::GeoArrow] {
            let o = ParquetOptions {
                covering_bbox: true,
                row_group_size: Some(1),
                ..options(encoding)
            };
            let file = write(&dir, "cov.parquet", &g, &o);
            let meta: serde_json::Value =
                serde_json::from_str(&geoparquet_metadata(&file).unwrap()).unwrap();
            assert_eq!(
                meta["columns"]["geometry"]["bbox"]
                    .as_array()
                    .unwrap()
                    .len(),
                4
            );
            let hit =
                read_geoparquet_filtered(&file, None, Some([40.0, 40.0, 60.0, 70.0])).unwrap();
            assert_eq!(hit.geometry.z(), [Some(7.0)]);
        }
    }

    #[test]
    fn arrow_files_and_streams_store_native_z() {
        let dir = TempDir::new().unwrap();
        let g = col(&[Some("LINESTRING ZM (0 0 1 5, 1 1 2 6)"), None]);
        let file = path(&dir, "z.feather");
        write_feather(
            &file,
            &g,
            "geometry",
            &no_attributes(2),
            GeometryEncoding::GeoArrow,
            FeatherCompression::Uncompressed,
        )
        .unwrap();
        let back = read_feather(&file, None, None).unwrap();
        assert_eq!(wkt(&back.geometry), wkt(&g));

        let bytes = to_arrow_ipc(
            &g,
            "geometry",
            &no_attributes(2),
            GeometryEncoding::GeoArrow,
        )
        .unwrap();
        let stream = from_arrow_ipc(&bytes, None, None).unwrap();
        assert_eq!(wkt(&stream.geometry), wkt(&g));
    }
}
