//! Datum shifts: the Helmert fallback, `+nadgrids` grids from a directory, and the PROJ
//! reference values in `test/reference/fixtures/crs_datum` (produced by pyproj, generator
//! `test/reference/generate_crs_datum.py`).

use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

use crate::{grid_dirs, grid_status, register_grids, set_grid_dirs, unregister_grids, GeoColumn};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/reference/fixtures/crs_datum"
);

/// The grid directories and registrations are process-wide: tests that read or change them (all
/// of these) run one at a time.
static GLOBAL_STATE: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    GLOBAL_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn point(x: f64, y: f64, crs: &str) -> GeoColumn {
    GeoColumn::from_wkt(&[Some(format!("POINT ({x:?} {y:?})"))], Some(crs)).unwrap()
}

fn xy(col: &GeoColumn) -> (f64, f64) {
    let wkt = col.to_wkt().unwrap()[0].clone().expect("row is not null");
    let inner = wkt.split_once('(').unwrap().1.trim_end_matches(')');
    let mut it = inner.split_whitespace().map(|t| t.parse::<f64>().unwrap());
    (it.next().unwrap(), it.next().unwrap())
}

/// Distance in metres between two lon/lat points on the WGS 84 sphere of mean radius (error
/// below 0.5 %, enough for tolerances of metres).
fn metres((lon1, lat1): (f64, f64), (lon2, lat2): (f64, f64)) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlon = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * a.sqrt().asin()
}

#[test]
fn nad27_and_nzgd49_without_grids_fall_back_to_a_helmert_shift() {
    let _guard = exclusive();
    // Both CRSs used to fail with "NAD grid not available" when no grid file was found.
    let nad27 = point(-100.0, 40.0, "EPSG:4267")
        .to_crs("EPSG:4326")
        .unwrap();
    let (lon, lat) = xy(&nad27);
    // PROJ's CONUS operation moves this point by about 20 m (+8/-160/-176 m geocentric).
    assert!(
        metres((lon, lat), (-100.0, 40.0)) > 10.0,
        "{lon} {lat}: the shift was dropped"
    );
    let nz = point(172.0, -42.0, "EPSG:4272")
        .to_crs("EPSG:4326")
        .unwrap();
    assert!(metres(xy(&nz), (172.0, -42.0)) > 1.0);
    // Back again, and through a projected NAD27 CRS.
    let back = point(-100.0, 40.0, "EPSG:4326")
        .to_crs("EPSG:4267")
        .unwrap();
    assert!(metres(xy(&back), (-100.0, 40.0)) > 10.0);
    assert!(point(500_000.0, 4_400_000.0, "EPSG:26714")
        .to_crs("EPSG:4326")
        .is_ok());
}

/// Largest deviation from pyproj (no grids installed) accepted per CRS, in metres. The bundled
/// table has one Helmert set per datum while PROJ picks the operation of the region: NAD27
/// differs by up to ~25 m (Alaska, Canada, the Caribbean), ED50 by ~60 m at the edge of its
/// area (Russia; 10 m in Western Europe), SAD69 by ~20 m, everything else by under 3 m.
fn tolerance(code: &str) -> f64 {
    match code {
        "4267" | "26714" | "26717" => 30.0,
        "4230" | "23032" => 60.0,
        "4618" => 20.0,
        _ => 3.0,
    }
}

#[test]
fn datum_shifts_without_grids_stay_within_the_documented_ballpark_of_pyproj() {
    let _guard = exclusive();
    let doc = fixture("datum_shifts.json");
    let mut worst = Vec::new();
    for (code, entry) in doc["crs"].as_object().unwrap() {
        let crs = format!("EPSG:{code}");
        for p in entry["pts"].as_array().unwrap() {
            let v: Vec<f64> = p
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_f64().unwrap())
                .collect();
            let moved = point(v[0], v[1], &crs).to_crs("EPSG:4326").unwrap();
            let miss = metres(xy(&moved), (v[2], v[3]));
            if miss > tolerance(code) {
                worst.push(format!(
                    "{crs} ({}) at {:?}: {miss:.2} m",
                    entry["name"],
                    &v[..2]
                ));
            }
        }
    }
    assert!(
        worst.is_empty(),
        "{} points too far: {:#?}",
        worst.len(),
        &worst[..worst.len().min(8)]
    );
}

/// Web Mercator (EPSG:3857) is defined on WGS 84 (`+nadgrids=@null` in its proj string). Going
/// straight to or from a CRS on another datum must apply that datum's shift, as going through
/// EPSG:4326 does. Expected values are pyproj 3.8 / PROJ 9.8.1 without grids (`always_xy`).
#[test]
fn web_mercator_keeps_the_datum_shift_to_other_datums() {
    let _guard = exclusive();
    let close = |got: (f64, f64), want: (f64, f64), what: &str| {
        let miss = ((got.0 - want.0).powi(2) + (got.1 - want.1).powi(2)).sqrt();
        assert!(
            miss < 0.01,
            "{what}: got {got:?}, want {want:?} ({miss:.3} m off)"
        );
    };
    // Soho, London (the Broad Street pump), to the British National Grid (OSGB36).
    let soho = (-15_222.781_732_626_117, 6_712_604.559_631_288);
    let osgb = point(soho.0, soho.1, "EPSG:3857")
        .to_crs("EPSG:27700")
        .unwrap();
    close(
        xy(&osgb),
        (529_390.946_921_815, 181_024.571_306_380_1),
        "3857 -> 27700",
    );
    let back = point(529_390.946_921_815, 181_024.571_306_380_1, "EPSG:27700")
        .to_crs("EPSG:3857")
        .unwrap();
    close(
        xy(&back),
        (-15_222.780_340_696_188, 6_712_604.558_808_459),
        "27700 -> 3857",
    );
    // Stuttgart to Gauss-Krueger zone 3 (DHDN).
    let stuttgart = point(1_021_912.925_482_251_3, 6_237_614.107_564_601, "EPSG:3857")
        .to_crs("EPSG:31467")
        .unwrap();
    close(
        xy(&stuttgart),
        (3_513_304.512_453_810_3, 5_404_737.597_294_708),
        "3857 -> 31467",
    );
    // The direct route and the route through EPSG:4326 agree.
    let via = point(soho.0, soho.1, "EPSG:3857")
        .to_crs("EPSG:4326")
        .unwrap()
        .to_crs("EPSG:27700")
        .unwrap();
    close(xy(&osgb), xy(&via), "direct vs through 4326");
}

/// Tokyo (EPSG:4301) is the datum the synthetic grids stand in for; no other test touches it.
const TOKYO: u32 = 4301;

#[test]
fn ntv2_and_geotiff_grids_from_a_directory_match_proj() {
    let _guard = exclusive();
    let cases = fixture("grids/cases.json");
    let grids = format!("{FIXTURES}/grids");
    let before = grid_dirs();

    // No directory configured: the grid is unavailable and the Helmert shift is used.
    register_grids(TOKYO, vec!["synthetic_tokyo.gsb".into()]);
    let status = grid_status("EPSG:4301").unwrap();
    assert_eq!(status, vec![("synthetic_tokyo.gsb".to_string(), None)]);
    let helmert = xy(&point(138.0, 35.0, "EPSG:4301").to_crs("EPSG:4326").unwrap());

    set_grid_dirs(vec![grids.clone()]);
    let status = grid_status("EPSG:4301").unwrap();
    assert_eq!(
        status[0].1.as_deref(),
        Some(format!("{grids}/synthetic_tokyo.gsb").as_str())
    );
    let gridded = xy(&point(138.0, 35.0, "EPSG:4301").to_crs("EPSG:4326").unwrap());
    assert!(
        metres(helmert, gridded) > 100.0,
        "the grid shift ({gridded:?}) is not the Helmert shift ({helmert:?})"
    );

    for (format, name) in [
        ("gsb", "synthetic_tokyo.gsb"),
        ("tif", "synthetic_tokyo.tif"),
    ] {
        unregister_grids(TOKYO);
        register_grids(TOKYO, vec![name.into()]);
        let mut worst = 0.0f64;
        for case in cases["inside"].as_array().unwrap() {
            let (x, y) = (case["x"].as_f64().unwrap(), case["y"].as_f64().unwrap());
            let want = |key: &str| {
                let v = case[key].as_array().unwrap();
                (v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
            };
            let forward = xy(&point(x, y, "EPSG:4301").to_crs("EPSG:4326").unwrap());
            worst = worst.max(metres(forward, want(format)));
            // The inverse of the same grid: PROJ's iteration against ours.
            let inverse = xy(&point(x, y, "EPSG:4326").to_crs("EPSG:4301").unwrap());
            worst = worst.max(metres(inverse, want("inv_gsb")));
        }
        assert!(worst < 0.001, "{format}: {worst} m from PROJ");

        // Outside the grid the Helmert shift takes over, as PROJ falls back to the next operation.
        for case in cases["outside"].as_array().unwrap() {
            let (x, y) = (case["x"].as_f64().unwrap(), case["y"].as_f64().unwrap());
            let want = case["fallback"].as_array().unwrap();
            let got = xy(&point(x, y, "EPSG:4301").to_crs("EPSG:4326").unwrap());
            let miss = metres(got, (want[0].as_f64().unwrap(), want[1].as_f64().unwrap()));
            assert!(miss < 0.001, "{format} outside {x} {y}: {miss} m");
            let want = case["inv_fallback"].as_array().unwrap();
            let got = xy(&point(x, y, "EPSG:4326").to_crs("EPSG:4301").unwrap());
            let miss = metres(got, (want[0].as_f64().unwrap(), want[1].as_f64().unwrap()));
            assert!(miss < 0.001, "{format} inverse outside {x} {y}: {miss} m");
        }
    }

    // A projected CRS on the same datum uses the grid too (Tokyo / Japan Plane Rectangular CS IX).
    let projected = point(0.0, 0.0, "EPSG:30169");
    assert!(projected.to_crs("EPSG:4326").is_ok());

    unregister_grids(TOKYO);
    set_grid_dirs(before);
}

#[test]
fn a_registered_grid_that_is_not_installed_is_not_an_error() {
    let _guard = exclusive();
    register_grids(4277, vec!["no_such_grid.tif".into()]);
    let moved = point(-1.5, 52.0, "EPSG:4277").to_crs("EPSG:4326");
    unregister_grids(4277);
    let (lon, lat) = xy(&moved.unwrap());
    assert!(
        metres((lon, lat), (-1.5, 52.0)) > 50.0,
        "OSGB36 shifts by ~100 m"
    );
}

#[test]
fn the_built_in_grid_table_names_the_proj_data_files() {
    let _guard = exclusive();
    let status = |crs: &str| -> Vec<String> {
        grid_status(crs)
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    };
    assert!(status("EPSG:27700").contains(&"uk_os_OSTN15_NTv2_OSGBtoETRS.tif".to_string()));
    assert!(status("EPSG:4267").contains(&"us_noaa_conus.tif".to_string()));
    assert!(status("EPSG:31467").contains(&"de_adv_BETA2007.tif".to_string()));
    assert!(status("EPSG:4272").contains(&"nz_linz_nzgd2kgrid0005.tif".to_string()));
    // CRSs without a datum grid have nothing to look up.
    assert!(status("EPSG:3857").is_empty());
    assert!(status("EPSG:4326").is_empty());
}
