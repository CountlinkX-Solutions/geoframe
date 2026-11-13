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

/// A definition with a prime meridian other than Greenwich finds the CRS of its datum on that
/// meridian (NTF (Paris) 4807) and answers like pyproj; MGI (Ferro), whose answer by code is 305 m
/// off pyproj, and Monte Mario (Rome), whose text PROJ shifts as the Greenwich datum but its code
/// not (72 m), are refused as geographic text; ATF (Paris), whose datum PROJ does not shift,
/// keeps its previous answer (fixture `crs_esri_prime_meridian.json`: GeoFrame on origin/main, and
/// pyproj).
#[test]
fn esri_wkt_with_another_prime_meridian_is_answered_through_its_crs() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/reference/fixtures/crs_datum/crs_esri_prime_meridian.json"
    ))
    .unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    for (code, e) in data["crs"].as_object().unwrap() {
        let result = transform_xy(
            e["wkt"].as_str().unwrap(),
            "EPSG:4326",
            num(&e["x"]),
            num(&e["y"]),
        );
        // the expected answer: pyproj's, except for ATF (Paris) (grads: unchanged)
        let key = if code == "4901" { "before" } else { "pyproj" };
        let [ex, ey] = [0, 1].map(|i| num(&e[key][i]));
        if code == "4805" || code == "4806" {
            let err = result.expect_err("geographic text must be refused");
            assert!(
                err.starts_with("unsupported WKT CRS") && err.contains("prime meridian"),
                "{code}: {err}"
            );
        } else {
            let (gx, gy) = result.unwrap_or_else(|e| panic!("{code}: {e}"));
            assert!(
                (gx - ex).abs() < 1e-6 && (gy - ey).abs() < 1e-6,
                "{code}: got {gx} {gy}, expected {ex} {ey}"
            );
        }
    }
}
