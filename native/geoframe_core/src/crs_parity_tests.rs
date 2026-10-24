//! `to_crs` against pyproj for the CRSs of issue #97: axis directions (Krovak southing and
//! westing), grad units and prime meridians of geographic CRSs, ellipsoidal equidistant
//! cylindrical, projections added on top of `proj4rs`, and Helmert operations chosen by area of
//! use. Fixture `test/reference/fixtures/crs_datum/crs_parity.json` is produced by
//! `generate_crs_datum.py parity` (pyproj 3.8 / PROJ 9.8, no grids, `always_xy`).

use serde_json::Value;

use crate::transform_xy;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/reference/fixtures/crs_datum/crs_parity.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Metres between two lon/lat points (spherical, plenty for centimetre tolerances).
fn metres((lon1, lat1): (f64, f64), (lon2, lat2): (f64, f64)) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlon = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * a.sqrt().asin()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

/// Every CRS of the fixture, both directions, against pyproj, within `tol_m` metres (at least a
/// millimetre).
fn check(codes: &[u32], tol_m: f64) {
    let data = fixture();
    for code in codes {
        let entry = &data["crs"][code.to_string()];
        assert!(entry.is_object(), "EPSG:{code} is not in the fixture");
        let label = format!("EPSG:{code}");
        for p in entry["pts"].as_array().unwrap() {
            let [lon, lat, x, y, ex, ey] = [0, 1, 2, 3, 4, 5].map(|i| num(&p[i]));
            let (gx, gy) = transform_xy("EPSG:4326", &label, lon, lat)
                .unwrap_or_else(|e| panic!("{label} forward at {lon} {lat}: {e}"));
            // Projected coordinates are metres; a geographic CRS is in degrees or grads.
            let unit_m = if entry["geographic"].as_bool().unwrap() {
                111_000.0
            } else {
                1.0
            };
            assert!(
                (gx - x).abs() * unit_m <= tol_m.max(1e-3)
                    && (gy - y).abs() * unit_m <= tol_m.max(1e-3),
                "{label} forward at {lon} {lat}: got {gx} {gy}, pyproj {x} {y}"
            );
            let (bx, by) = transform_xy(&label, "EPSG:4326", x, y)
                .unwrap_or_else(|e| panic!("{label} inverse at {x} {y}: {e}"));
            let d = metres((bx, by), (ex, ey));
            assert!(
                d <= tol_m,
                "{label} inverse at {x} {y}: got {bx} {by}, pyproj {ex} {ey} ({d} m)"
            );
        }
    }
}

#[test]
fn krovak_southing_westing_axes() {
    // EPSG:5513 and 2065 are southing/westing; the East North twin 5514 is the control.
    check(&[5513, 2065, 5514], 0.01);
}

#[test]
fn paris_meridian_grad_geographic_crs() {
    check(&[4807, 4901], 0.01);
    check(&[4810, 4816, 4821, 4811], 0.01);
}

#[test]
fn ellipsoidal_equidistant_cylindrical() {
    check(&[4087], 0.01);
}

#[test]
fn hotine_oblique_mercator_variants_a_and_b() {
    // 2057 Rassadiran (variant B, with u offset), 3078 Michigan (variant A, +no_uoff), 3375
    // and 3468 (Malaysia / Alaska, +no_uoff with gamma), 6394 Michigan-Ohio.
    check(&[2057, 3078, 3375, 3468, 6394], 0.05);
}

#[test]
fn cassini_soldner() {
    check(&[2066, 3377], 0.05);
    check(&[3068], 3.0);
}

#[test]
fn polyconic() {
    check(&[5472, 5880], 0.05);
}

#[test]
fn equal_earth() {
    check(&[8857, 8858], 0.01);
}

#[test]
fn new_zealand_map_grid() {
    check(&[27200], 0.05);
}

#[test]
fn azimuthal_equidistant_guam() {
    check(&[3295, 3993], 0.05);
}

#[test]
fn helmert_operation_by_area_of_use() {
    // Fatu Iva 72: the table's +towgs84 is the wrong one (rotation signs), PROJ has one
    // operation for the island. S-JTSK: four operations by area.
    check(&[4688, 3303, 4156], 0.01);
}
