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
    fn positions_without_z_in_a_3d_geometry_get_nan() {
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
        assert!(z(1).is_nan());
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
                "GEOMETRYCOLLECTION Z(POINT Z(1 2 3),POINT Z(4 5 NaN))".to_string()
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
