//! Complete PROJJSON definitions for the coordinate reference systems GeoFrame can name
//! without a PROJ database.
//!
//! GeoPandas and pyproj refuse a PROJJSON stub that only carries an identifier, so files
//! written with one are unreadable there. The CRS below are written in full instead:
//! `EPSG:4326` and `EPSG:4269` (geographic), `EPSG:3857` (Pseudo-Mercator) and the WGS 84 UTM
//! zones `EPSG:32601`..`32660` (north) and `EPSG:32701`..`32760` (south). Every other EPSG
//! code in the bundled `crs-definitions` table is translated from its WKT definition
//! (`wkt1_projjson.rs`, `wkt2_projjson.rs`): geographic (2D and 3D), geocentric, projected and
//! compound CRS. An identifier outside that table (the pure-Rust build has no PROJ database),
//! another authority, or one of the few entries the table leaves empty keeps the
//! name-and-identifier stub, which GeoFrame reads back but pyproj cannot interpret; a caller
//! that needs such a CRS in a GeoParquet file passes its PROJJSON as the CRS.

use serde_json::{json, Value};

const SCHEMA: &str = "https://proj.org/schemas/v0.7/projjson.schema.json";

/// The full PROJJSON of `authority:code`, if it is one of the built-in definitions.
pub(crate) fn known_projjson(authority: &str, code: u64) -> Option<Value> {
    if !authority.eq_ignore_ascii_case("EPSG") {
        return None;
    }
    match code {
        4326 => Some(wgs84()),
        4269 => Some(nad83()),
        3857 => Some(pseudo_mercator()),
        32601..=32660 => Some(utm(code - 32600, true)),
        32701..=32760 => Some(utm(code - 32700, false)),
        _ => from_table(code),
    }
}

/// The PROJJSON translated from the bundled WKT1 table. The requested code names the `id`: a
/// few table entries carry another code in their own `AUTHORITY`.
fn from_table(code: u64) -> Option<Value> {
    let wkt = crate::crs_wkt::epsg_definition(i64::try_from(code).ok()?)?;
    let mut value = crate::wkt1_projjson::wkt1_to_projjson(wkt)?;
    value["id"] = json!({ "authority": "EPSG", "code": code });
    Some(value)
}

fn geographic_axes() -> Value {
    json!({
        "subtype": "ellipsoidal",
        "axis": [
            { "name": "Geodetic latitude", "abbreviation": "Lat", "direction": "north", "unit": "degree" },
            { "name": "Geodetic longitude", "abbreviation": "Lon", "direction": "east", "unit": "degree" }
        ]
    })
}

fn projected_axes() -> Value {
    json!({
        "subtype": "Cartesian",
        "axis": [
            { "name": "Easting", "abbreviation": "E", "direction": "east", "unit": "metre" },
            { "name": "Northing", "abbreviation": "N", "direction": "north", "unit": "metre" }
        ]
    })
}

fn wgs84_base() -> Value {
    json!({
        "type": "GeographicCRS",
        "name": "WGS 84",
        "datum": {
            "type": "GeodeticReferenceFrame",
            "name": "World Geodetic System 1984",
            "ellipsoid": {
                "name": "WGS 84",
                "semi_major_axis": 6378137,
                "inverse_flattening": 298.257223563
            }
        },
        "coordinate_system": geographic_axes(),
        "id": { "authority": "EPSG", "code": 4326 }
    })
}

fn wgs84() -> Value {
    let mut value = wgs84_base();
    value["$schema"] = Value::from(SCHEMA);
    value
}

fn nad83() -> Value {
    json!({
        "$schema": SCHEMA,
        "type": "GeographicCRS",
        "name": "NAD83",
        "datum": {
            "type": "GeodeticReferenceFrame",
            "name": "North American Datum 1983",
            "ellipsoid": {
                "name": "GRS 1980",
                "semi_major_axis": 6378137,
                "inverse_flattening": 298.257222101
            }
        },
        "coordinate_system": geographic_axes(),
        "id": { "authority": "EPSG", "code": 4269 }
    })
}

fn parameter(name: &str, value: Value, unit: &str, code: u32) -> Value {
    json!({
        "name": name,
        "value": value,
        "unit": unit,
        "id": { "authority": "EPSG", "code": code }
    })
}

fn pseudo_mercator() -> Value {
    json!({
        "$schema": SCHEMA,
        "type": "ProjectedCRS",
        "name": "WGS 84 / Pseudo-Mercator",
        "base_crs": wgs84_base(),
        "conversion": {
            "name": "Popular Visualisation Pseudo-Mercator",
            "method": {
                "name": "Popular Visualisation Pseudo Mercator",
                "id": { "authority": "EPSG", "code": 1024 }
            },
            "parameters": [
                parameter("Latitude of natural origin", json!(0), "degree", 8801),
                parameter("Longitude of natural origin", json!(0), "degree", 8802),
                parameter("False easting", json!(0), "metre", 8806),
                parameter("False northing", json!(0), "metre", 8807)
            ]
        },
        "coordinate_system": projected_axes(),
        "id": { "authority": "EPSG", "code": 3857 }
    })
}

fn utm(zone: u64, north: bool) -> Value {
    let hemisphere = if north { 'N' } else { 'S' };
    let central_meridian = zone as i64 * 6 - 183;
    let code = if north { 32600 + zone } else { 32700 + zone };
    json!({
        "$schema": SCHEMA,
        "type": "ProjectedCRS",
        "name": format!("WGS 84 / UTM zone {zone}{hemisphere}"),
        "base_crs": wgs84_base(),
        "conversion": {
            "name": format!("UTM zone {zone}{hemisphere}"),
            "method": {
                "name": "Transverse Mercator",
                "id": { "authority": "EPSG", "code": 9807 }
            },
            "parameters": [
                parameter("Latitude of natural origin", json!(0), "degree", 8801),
                parameter("Longitude of natural origin", json!(central_meridian), "degree", 8802),
                parameter("Scale factor at natural origin", json!(0.9996), "unity", 8805),
                parameter("False easting", json!(500000), "metre", 8806),
                parameter("False northing", json!(if north { 0 } else { 10_000_000 }), "metre", 8807)
            ]
        },
        "coordinate_system": projected_axes(),
        "id": { "authority": "EPSG", "code": code }
    })
}
