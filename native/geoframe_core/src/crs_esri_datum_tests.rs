//! `to_crs` with a CRS given as WKT1_ESRI text (a `.prj`: `GCS_...`, `D_...`, no identifier)
//! against pyproj. PROJ identifies the datum by its name and shifts it to WGS 84 like the code
//! would; the fixture `test/reference/fixtures/crs_datum/crs_esri_datum.json` is produced by
//! `generate_crs_datum.py esri_wkt` (pyproj 3.8 / PROJ 9.8, no grids, `always_xy`).

use serde_json::Value;

use crate::transform_xy;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/reference/fixtures/crs_datum/crs_esri_datum.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

/// The result is lon/lat degrees: 1e-7 of a degree is about a centimetre.
const TOLERANCE: f64 = 1e-7;

#[test]
fn esri_wkt_shifts_its_datum_like_pyproj() {
    let data = fixture();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (code, entry) in data["crs"].as_object().unwrap() {
        let wkt = entry["wkt"].as_str().unwrap();
        for p in entry["pts"].as_array().unwrap() {
            let [x, y, ex, ey] = [0, 1, 2, 3].map(|i| num(&p[i]));
            checked += 1;
            match transform_xy(wkt, "EPSG:4326", x, y) {
                Ok((gx, gy)) => {
                    if (gx - ex).abs() > TOLERANCE || (gy - ey).abs() > TOLERANCE {
                        failures.push(format!(
                            "{code} ({}): {x} {y} -> got {gx} {gy}, pyproj {ex} {ey}",
                            entry["name"]
                        ));
                    }
                }
                Err(e) => failures.push(format!("{code}: {e}")),
            }
        }
    }
    assert!(checked > 0);
    assert!(
        failures.is_empty(),
        "{} of {checked} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A definition with a prime meridian other than Greenwich (Ferro, Paris, Rome) is not moved by
/// the datum-name lookup: its result stays what it was before (fixture
/// `crs_esri_prime_meridian.json`: GeoFrame without the lookup).
#[test]
fn esri_wkt_with_another_prime_meridian_keeps_its_result() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/reference/fixtures/crs_datum/crs_esri_prime_meridian.json"
    ))
    .unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    for (code, e) in data["crs"].as_object().unwrap() {
        let (gx, gy) = transform_xy(
            e["wkt"].as_str().unwrap(),
            "EPSG:4326",
            num(&e["x"]),
            num(&e["y"]),
        )
        .unwrap();
        let [bx, by] = [0, 1].map(|i| num(&e["before"][i]));
        assert!(
            (gx - bx).abs() < 1e-9 && (gy - by).abs() < 1e-9,
            "{code}: got {gx} {gy}, before {bx} {by}"
        );
    }
}
