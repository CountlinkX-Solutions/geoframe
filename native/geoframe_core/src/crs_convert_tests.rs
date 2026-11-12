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

/// The CRS of the fixture that the bundled table lacks, by code: they come from the supplementary
/// table generated from the PROJ database.
#[test]
fn codes_outside_the_bundled_table_resolve_like_pyproj() {
    let data = fixture();
    let mut checked = 0;
    for (code, entry) in data["crs"].as_object().unwrap() {
        let number: u32 = code.parse().unwrap();
        if u16::try_from(number)
            .ok()
            .and_then(crs_definitions::from_code)
            .is_some()
        {
            continue;
        }
        checked += 1;
        let label = format!("EPSG:{code}");
        check(code, entry, &label, "its code").unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(
        checked >= 4,
        "the fixture should hold CRSs outside the table"
    );
}

#[test]
fn supplementary_codes_have_full_projjson_for_geoparquet() {
    let json = crate::projjson::known_projjson("EPSG", 5515).expect("EPSG:5515 is supplementary");
    assert_eq!(json["type"], "ProjectedCRS");
    assert_eq!(json["id"]["code"], 5515);
    assert_eq!(json["conversion"]["method"]["name"], "Krovak Modified");
}

#[test]
fn supplementary_table_complements_the_bundled_one() {
    let codes = crate::crs_extra::codes();
    assert!(
        codes.len() > 1000,
        "{} supplementary definitions",
        codes.len()
    );
    assert!(codes.windows(2).all(|w| w[0] < w[1]), "sorted, no repeats");
    for code in codes {
        let bundled = u16::try_from(code)
            .ok()
            .and_then(crs_definitions::from_code)
            .filter(|d| !d.proj4.is_empty());
        assert!(bundled.is_none(), "EPSG:{code} is in both tables");
        // every definition converts (or names the method it cannot)
        let json = crate::crs_extra::projjson(code).unwrap();
        if let Err(e) = crate::crs_convert::from_projjson(&json) {
            assert!(
                e.contains("unsupported projection method"),
                "EPSG:{code}: {e}"
            );
        }
    }
}

// -- ESRI WKT1 StatePlane definitions in feet, issue #110 --------------------------------------

/// ESRI WKT1 writes parameter names in mixed case (`False_Easting`, `Standard_Parallel_1`), the
/// projection `Lambert_Conformal_Conic` for the 2SP method and the unit `Foot_US`/`Foot`.
/// `(name, wkt, [(lon, lat, x, y, back_lon, back_lat)])`; the TM definition is the `.prj` of #110
/// (ESRI:102649), the LCC one is ESRI:102739 (Texas Central), each also with the unit changed to
/// the international `Foot`. Values from pyproj 3.8 / PROJ 9.8, `always_xy`:
///
/// ```python
/// t = pyproj.Transformer.from_crs("EPSG:4326", wkt, always_xy=True)
/// b = pyproj.Transformer.from_crs(wkt, "EPSG:4326", always_xy=True)
/// x, y = t.transform(lon, lat); back = b.transform(x, y)
/// ```
type EsriCase = (&'static str, String, [(f64, f64, f64, f64, f64, f64); 3]);

fn esri_cases() -> Vec<EsriCase> {
    const TM: &str = r#"PROJCS["NAD_1983_StatePlane_Arizona_Central_FIPS_0202_Feet",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",699998.6],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",-111.9166666666667],PARAMETER["Scale_Factor",0.9999],PARAMETER["Latitude_Of_Origin",31.0],UNIT["Foot_US",0.3048006096012192]]"#;
    const LCC: &str = r#"PROJCS["NAD_1983_StatePlane_Texas_Central_FIPS_4203_Feet",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",2296583.333333333],PARAMETER["False_Northing",9842500.0],PARAMETER["Central_Meridian",-100.3333333333333],PARAMETER["Standard_Parallel_1",30.11666666666667],PARAMETER["Standard_Parallel_2",31.88333333333333],PARAMETER["Latitude_Of_Origin",29.66666666666667],UNIT["Foot_US",0.3048006096012192]]"#;
    let foot = |w: &str| {
        w.replace(
            r#"UNIT["Foot_US",0.3048006096012192]"#,
            r#"UNIT["Foot",0.3048]"#,
        )
    };
    vec![
        (
            "tm_foot_us",
            TM.to_string(),
            [
                (
                    -112.0,
                    33.5,
                    674595.8554291083,
                    909458.6895101842,
                    -112.0,
                    33.5,
                ),
                (
                    -111.5,
                    34.2,
                    825980.9492779631,
                    1164416.8040223015,
                    -111.5,
                    34.2,
                ),
                (
                    -112.6,
                    32.8,
                    490033.0867108226,
                    655444.7621810915,
                    -112.6,
                    32.8,
                ),
            ],
        ),
        (
            "tm_foot",
            foot(TM),
            [
                (
                    -112.0,
                    33.5,
                    674595.8046235174,
                    909460.5084312009,
                    -112.0,
                    33.5,
                ),
                (
                    -111.5,
                    34.2,
                    825981.2012431655,
                    1164419.132860567,
                    -111.5,
                    34.2,
                ),
                (
                    -112.6,
                    32.8,
                    490032.6667789561,
                    655446.0730732374,
                    -112.6,
                    32.8,
                ),
            ],
        ),
        (
            "lcc_foot_us",
            LCC.to_string(),
            [
                (
                    -100.0,
                    31.0,
                    2401015.348007461,
                    10327585.26945426,
                    -100.0,
                    31.0,
                ),
                (
                    -99.5,
                    30.5,
                    2559023.55561745,
                    10146565.91195174,
                    -99.5,
                    30.5,
                ),
                (
                    -101.2,
                    31.5,
                    2026479.268677063,
                    10510340.943435503,
                    -101.2,
                    31.5,
                ),
            ],
        ),
        (
            "lcc_foot",
            foot(LCC),
            [
                (
                    -100.0,
                    31.0,
                    2401015.5572052407,
                    10327586.239626735,
                    -100.0,
                    31.0,
                ),
                (
                    -99.5,
                    30.5,
                    2559024.0808322765,
                    10146566.520084778,
                    -99.5,
                    30.5,
                ),
                (
                    -101.2,
                    31.5,
                    2026478.728801186,
                    10510342.279120063,
                    -101.2,
                    31.5,
                ),
            ],
        ),
    ]
}

#[test]
fn esri_wkt1_stateplane_in_feet_converts_to_projjson() {
    for (name, wkt, _) in esri_cases() {
        let json =
            crate::crs_convert::wkt_to_projjson(&wkt).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(json["type"], "ProjectedCRS", "{name}");
        crate::crs_convert::from_projjson(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn esri_wkt1_stateplane_in_feet_reprojects_like_pyproj() {
    // forward to 1e-3 ft (far below the 1e-2 m of the other PROJ comparisons), inverse to 1e-8
    // degrees (about a millimetre; proj4rs differs from PROJ by 1e-9 degrees on the LCC inverse)
    for (name, wkt, points) in esri_cases() {
        for (lon, lat, x, y, back_lon, back_lat) in points {
            let (gx, gy) = transform_xy("EPSG:4326", &wkt, lon, lat)
                .unwrap_or_else(|e| panic!("{name} forward at {lon} {lat}: {e}"));
            assert!(
                (gx - x).abs() < 1e-3 && (gy - y).abs() < 1e-3,
                "{name} forward: {gx} {gy} vs {x} {y}"
            );
            let (bx, by) = transform_xy(&wkt, "EPSG:4326", x, y)
                .unwrap_or_else(|e| panic!("{name} inverse at {x} {y}: {e}"));
            assert!(
                (bx - back_lon).abs() < 1e-8 && (by - back_lat).abs() < 1e-8,
                "{name} inverse: {bx} {by}"
            );
        }
    }
}
