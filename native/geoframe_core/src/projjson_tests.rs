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
        assert!(crs["name"].is_string(), "EPSG:{code}: no name");
        assert_eq!(crs["id"]["code"], code, "EPSG:{code}: id");
        assert!(
            crs["coordinate_system"]["axis"].is_array(),
            "EPSG:{code}: axes"
        );
        if kind == "ProjectedCRS" {
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
        } else {
            assert_eq!(kind, "GeographicCRS", "EPSG:{code}");
            assert!(
                crs["datum"]["ellipsoid"].is_object(),
                "EPSG:{code}: ellipsoid"
            );
        }
    }
    assert!(converted > 4000, "only {converted} definitions converted");
}
