//! `to_crs` / `set_crs` with a CRS given as a definition (PROJJSON, WKT1 or WKT2 text with no
//! identifier) against pyproj, issue #100. Fixture
//! `test/reference/fixtures/crs_datum/crs_convert.json` is produced by
//! `generate_crs_datum.py convert` (pyproj 3.8 / PROJ 9.8, no grids, `always_xy`); every entry
//! carries the definition without its `id`, so nothing can be looked up by code.

use serde_json::Value;

use crate::transform_xy;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/reference/fixtures/crs_datum/crs_convert.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn metres((lon1, lat1): (f64, f64), (lon2, lat2): (f64, f64)) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlon = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * a.sqrt().asin()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

/// Tolerance in metres: a centimetre, except where PROJ's own formulas are series that differ
/// from the exact ones (Laborde 8441: 3 cm over Madagascar; the Hyperbolic Cassini-Soldner inverse
/// of 3139: 7 cm).
fn tolerance(code: &str) -> f64 {
    match code {
        "8441" => 0.1,
        "3139" => 0.1,
        _ => 0.01,
    }
}

/// `definition` as the CRS, both directions, against pyproj.
fn check(code: &str, entry: &Value, definition: &str, flavour: &str) -> Result<(), String> {
    let unit_m = if entry["geographic"].as_bool().unwrap() {
        111_000.0
    } else {
        1.0
    };
    for p in entry["pts"].as_array().unwrap() {
        let [lon, lat, x, y, ex, ey] = [0, 1, 2, 3, 4, 5].map(|i| num(&p[i]));
        let (gx, gy) = transform_xy("EPSG:4326", definition, lon, lat)
            .map_err(|e| format!("EPSG:{code} as {flavour}, forward at {lon} {lat}: {e}"))?;
        let tol = tolerance(code);
        if (gx - x).abs() * unit_m > tol || (gy - y).abs() * unit_m > tol {
            return Err(format!(
                "EPSG:{code} as {flavour}, forward at {lon} {lat}: got {gx} {gy}, pyproj {x} {y}"
            ));
        }
        let (bx, by) = transform_xy(definition, "EPSG:4326", x, y)
            .map_err(|e| format!("EPSG:{code} as {flavour}, inverse at {x} {y}: {e}"))?;
        let d = metres((bx, by), (ex, ey));
        if d > tol {
            return Err(format!(
                "EPSG:{code} as {flavour}, inverse at {x} {y}: got {bx} {by}, pyproj {ex} {ey} ({d} m)"
            ));
        }
    }
    Ok(())
}

/// Runs `check` over the fixture entries that have `field` and reports every failure at once.
fn check_all(field: &str) {
    let data = fixture();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (code, entry) in data["crs"].as_object().unwrap() {
        let definition = match &entry[field] {
            Value::String(text) => text.clone(),
            Value::Null => continue,
            other => serde_json::to_string(other).unwrap(),
        };
        checked += 1;
        if let Err(e) = check(code, entry, &definition, field) {
            failures.push(e);
        }
    }
    assert!(checked > 0, "no {field} in the fixture");
    assert!(
        failures.is_empty(),
        "{} of {checked} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn projjson_without_an_identifier_converts_like_pyproj() {
    check_all("projjson");
}

#[test]
fn wkt1_text_converts_like_pyproj() {
    check_all("wkt1");
}

#[test]
fn wkt2_text_converts_like_pyproj() {
    check_all("wkt2");
}
