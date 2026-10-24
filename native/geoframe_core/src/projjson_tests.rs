//! PROJJSON written for GeoParquet must be complete enough for PROJ to parse (issue #88): every
//! CRS object carries a top-level `type`, projected ones their base CRS, conversion and axes.

use serde_json::Value;

use crate::projjson::known_projjson;

fn epsg(code: u64) -> Value {
    known_projjson("EPSG", code).unwrap_or_else(|| panic!("no PROJJSON for EPSG:{code}"))
}

fn parameter(crs: &Value, id: u64) -> f64 {
    crs["conversion"]["parameters"]
        .as_array()
        .expect("parameters")
        .iter()
        .find(|p| p["id"]["code"] == id)
        .unwrap_or_else(|| panic!("no parameter {id}"))["value"]
        .as_f64()
        .expect("numeric value")
}

#[test]
fn laea_europe_is_a_complete_projected_crs() {
    let crs = epsg(3035);
    assert_eq!(crs["type"], "ProjectedCRS");
    assert_eq!(crs["name"], "ETRS89 / LAEA Europe");
    assert_eq!(crs["id"]["code"], 3035);
    assert_eq!(crs["base_crs"]["type"], "GeographicCRS");
    assert_eq!(crs["base_crs"]["id"]["code"], 4258);
    assert_eq!(
        crs["base_crs"]["datum"]["ellipsoid"]["inverse_flattening"],
        298.257222101
    );
    assert_eq!(crs["conversion"]["method"]["id"]["code"], 9820);
    assert_eq!(parameter(&crs, 8801), 52.0);
    assert_eq!(parameter(&crs, 8802), 10.0);
    assert_eq!(parameter(&crs, 8806), 4_321_000.0);
    assert_eq!(parameter(&crs, 8807), 3_210_000.0);
    assert_eq!(crs["coordinate_system"]["subtype"], "Cartesian");
    assert_eq!(crs["coordinate_system"]["axis"][0]["direction"], "east");
    assert_eq!(crs["coordinate_system"]["axis"][0]["unit"], "metre");
}

#[test]
fn other_projections_map_to_their_epsg_methods() {
    // Lambert-93: Lambert Conic Conformal (2SP).
    let lambert = epsg(2154);
    assert_eq!(lambert["conversion"]["method"]["id"]["code"], 9802);
    assert_eq!(parameter(&lambert, 8823), 49.0);
    assert_eq!(parameter(&lambert, 8826), 700_000.0);
    // CONUS Albers.
    let albers = epsg(5070);
    assert_eq!(albers["conversion"]["method"]["id"]["code"], 9822);
    // Antarctic Polar Stereographic is variant B.
    let polar = epsg(3031);
    assert_eq!(polar["conversion"]["method"]["id"]["code"], 9829);
    assert_eq!(parameter(&polar, 8832), -71.0);
    // British National Grid: Transverse Mercator.
    let bng = epsg(27700);
    assert_eq!(bng["conversion"]["method"]["id"]["code"], 9807);
    assert_eq!(parameter(&bng, 8805), 0.9996012717);
}

#[test]
fn geographic_crs_are_complete_too() {
    let crs = epsg(4258);
    assert_eq!(crs["type"], "GeographicCRS");
    assert_eq!(crs["id"]["code"], 4258);
    assert_eq!(crs["datum"]["ellipsoid"]["semi_major_axis"], 6378137);
    assert_eq!(crs["coordinate_system"]["axis"][0]["direction"], "north");
}

#[test]
fn every_definition_is_schema_shaped() {
    let mut converted = 0;
    for code in 2000..=32767_u64 {
        let Some(crs) = known_projjson("EPSG", code) else {
            continue;
        };
        converted += 1;
        let kind = crs["type"]
            .as_str()
            .unwrap_or_else(|| panic!("EPSG:{code}: no type"));
        assert_eq!(crs["id"]["code"], code, "EPSG:{code}: id");
        let has_axes = crs["coordinate_system"]["axis"].is_array();
        match kind {
            "ProjectedCRS" => {
                assert!(crs["name"].is_string() && has_axes, "EPSG:{code}");
                assert_eq!(crs["base_crs"]["type"], "GeographicCRS", "EPSG:{code}");
                assert!(
                    crs["conversion"]["method"]["name"].is_string(),
                    "EPSG:{code}"
                );
                assert!(
                    !crs["conversion"]["parameters"]
                        .as_array()
                        .unwrap()
                        .is_empty(),
                    "EPSG:{code}: parameters"
                );
            }
            "GeographicCRS" | "GeodeticCRS" => {
                assert!(crs["name"].is_string() && has_axes, "EPSG:{code}");
                assert!(
                    crs["datum"]["ellipsoid"].is_object(),
                    "EPSG:{code}: ellipsoid"
                );
            }
            "CompoundCRS" => {
                let parts = crs["components"].as_array().unwrap();
                assert_eq!(parts.len(), 2, "EPSG:{code}");
            }
            other => panic!("EPSG:{code}: unexpected type {other}"),
        }
    }
    assert!(converted > 4000, "only {converted} definitions converted");
}

#[test]
fn geocentric_crs_are_complete() {
    let crs = epsg(3822);
    assert_eq!(crs["type"], "GeodeticCRS");
    assert_eq!(crs["id"]["code"], 3822);
    assert_eq!(crs["coordinate_system"]["subtype"], "Cartesian");
    let directions: Vec<&str> = crs["coordinate_system"]["axis"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["direction"].as_str().unwrap())
        .collect();
    assert_eq!(directions, ["geocentricX", "geocentricY", "geocentricZ"]);
    assert_eq!(crs["datum"]["ellipsoid"]["semi_major_axis"], 6378137);
}

#[test]
fn three_dimensional_geographic_crs_keep_their_height_axis() {
    let crs = epsg(4327);
    assert_eq!(crs["type"], "GeographicCRS");
    assert_eq!(crs["id"]["code"], 4327);
    assert_eq!(
        crs["datum"]["ellipsoid"]["inverse_flattening"],
        298.257223563
    );
    let axes = crs["coordinate_system"]["axis"].as_array().unwrap();
    assert_eq!(axes.len(), 3);
    assert_eq!(axes[0]["name"], "Geodetic latitude");
    assert_eq!(axes[0]["abbreviation"], "Lat");
    assert_eq!(axes[2]["name"], "Ellipsoidal height");
    assert_eq!(axes[2]["direction"], "up");
    assert_eq!(axes[2]["unit"], "metre");
}

#[test]
fn datum_shifted_entries_are_written_as_their_source_crs() {
    // The table stores EPSG:3823 as a BOUNDCRS (TWD97 to WGS 84, zero shift); the EPSG
    // definition, and what pyproj names by this code, is the 3D geographic CRS itself.
    let crs = epsg(3823);
    assert_eq!(crs["type"], "GeographicCRS");
    assert_eq!(crs["id"]["code"], 3823);
    assert_eq!(crs["name"], "TWD97");
    assert_eq!(crs["datum"]["name"], "Taiwan Datum 1997");
    assert_eq!(
        crs["coordinate_system"]["axis"].as_array().unwrap().len(),
        3
    );
    assert!(crs.get("transformation").is_none());
}

#[test]
fn compound_crs_list_horizontal_and_vertical_components() {
    // Geographic + vertical, projected + vertical, and a projection with a rectified angle.
    for (code, horizontal) in [
        (5498, "GeographicCRS"),
        (5707, "ProjectedCRS"),
        (8803, "ProjectedCRS"),
    ] {
        let crs = epsg(code);
        assert_eq!(crs["type"], "CompoundCRS", "EPSG:{code}");
        assert_eq!(crs["id"]["code"], code);
        let components = crs["components"].as_array().unwrap();
        assert_eq!(components.len(), 2, "EPSG:{code}");
        assert_eq!(components[0]["type"], horizontal, "EPSG:{code}");
        assert_eq!(components[1]["type"], "VerticalCRS", "EPSG:{code}");
        assert_eq!(components[1]["coordinate_system"]["subtype"], "vertical");
        assert_eq!(
            components[1]["coordinate_system"]["axis"][0]["direction"],
            "up"
        );
        // Only the compound is the top-level object, so only it carries the schema.
        assert!(components[0].get("$schema").is_none(), "EPSG:{code}");
    }
    let vertical = &epsg(5498)["components"][1];
    assert_eq!(vertical["datum"]["type"], "VerticalReferenceFrame");
    assert_eq!(
        vertical["datum"]["name"],
        "North American Vertical Datum 1988"
    );
    assert_eq!(vertical["id"]["code"], 5703);
}

#[test]
fn projections_with_their_own_epsg_methods_are_translated() {
    let laborde = epsg(8441);
    assert_eq!(laborde["conversion"]["method"]["id"]["code"], 9813);
    assert_eq!(parameter(&laborde, 8813), 18.9);
    let belgium = epsg(31300);
    assert_eq!(belgium["conversion"]["method"]["id"]["code"], 9803);
    assert_eq!(parameter(&belgium, 8821), 90.0);
    let tunisia = epsg(22300);
    assert_eq!(tunisia["conversion"]["method"]["id"]["code"], 9816);
    assert_eq!(parameter(&tunisia, 8826), 270.0);
    assert_eq!(
        tunisia["conversion"]["parameters"][2]["unit"]["name"],
        "kilometre"
    );
}

/// Every EPSG definition in the bundled table, bar the entries the table leaves empty, is
/// written in full (issue #50).
#[test]
fn every_nonempty_table_definition_has_a_full_projjson() {
    let mut stubs = Vec::new();
    let mut empty = 0;
    for code in 0..=65535_u64 {
        let Some(wkt) = crate::crs_wkt::epsg_definition(code as i64) else {
            continue;
        };
        if wkt.is_empty() {
            empty += 1;
        } else if known_projjson("EPSG", code).is_none() {
            stubs.push(code);
        }
    }
    assert!(stubs.is_empty(), "no PROJJSON for {stubs:?}");
    assert_eq!(empty, 8);
}
