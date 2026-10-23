//! Reprojection reference cases. Expected values come from closed forms or an independent
//! series implementation in this file, not from the engine under test.

use crate::{normalize_crs, GeoColumn};

const R: f64 = 6_378_137.0;

fn pt(x: f64, y: f64, crs: &str) -> GeoColumn {
    GeoColumn::from_wkt(&[Some(format!("POINT ({x} {y})"))], Some(crs)).unwrap()
}

/// First vertex of the first row, parsed from WKT.
fn first_xy(col: &GeoColumn) -> (f64, f64) {
    let wkt = col.to_wkt().unwrap()[0].clone().expect("row is not null");
    let inner = wkt.split_once('(').unwrap().1;
    let nums: Vec<f64> = inner
        .split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | 'E' | '+')))
        .filter(|t| !t.is_empty())
        .map(|t| t.parse().unwrap())
        .collect();
    (nums[0], nums[1])
}

fn web_mercator(lon: f64, lat: f64) -> (f64, f64) {
    let phi = lat.to_radians();
    (
        R * lon.to_radians(),
        R * (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln(),
    )
}

/// Independent forward UTM (WGS84) via the Krueger n-series to third order (error < 1e-4 m).
fn utm_krueger(lon: f64, lat: f64, zone: u32, north: bool) -> (f64, f64) {
    let f = 1.0 / 298.257_223_563;
    let n = f / (2.0 - f);
    let a_big = R / (1.0 + n) * (1.0 + n * n / 4.0 + n.powi(4) / 64.0);
    let alpha = [
        n / 2.0 - 2.0 * n * n / 3.0 + 5.0 * n.powi(3) / 16.0,
        13.0 * n * n / 48.0 - 3.0 * n.powi(3) / 5.0,
        61.0 * n.powi(3) / 240.0,
    ];
    let lon0 = f64::from(zone) * 6.0 - 183.0;
    let (phi, dl) = (lat.to_radians(), (lon - lon0).to_radians());
    let c = 2.0 * n.sqrt() / (1.0 + n);
    let t = (phi.sin().atanh() - c * (c * phi.sin()).atanh()).sinh();
    let xi = (t / dl.cos()).atan();
    let eta = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
    let (mut x, mut y) = (eta, xi);
    for (j, a) in alpha.iter().enumerate() {
        let k = 2.0 * (j as f64 + 1.0);
        x += a * (k * xi).cos() * (k * eta).sinh();
        y += a * (k * xi).sin() * (k * eta).cosh();
    }
    (
        500_000.0 + 0.9996 * a_big * x,
        if north { 0.0 } else { 10_000_000.0 } + 0.9996 * a_big * y,
    )
}

#[test]
fn web_mercator_matches_closed_form() {
    for (lon, lat) in [(-80.19, 25.76), (0.0, 0.0), (151.2, -33.87), (179.9, 60.0)] {
        let out = pt(lon, lat, "EPSG:4326").to_crs("EPSG:3857").unwrap();
        let (x, y) = first_xy(&out);
        let (ex, ey) = web_mercator(lon, lat);
        assert!(
            (x - ex).abs() < 1e-3 && (y - ey).abs() < 1e-3,
            "{lon} {lat}: {x} {y}"
        );
        assert_eq!(out.crs().as_deref(), Some("EPSG:3857"));
    }
}

#[test]
fn utm_17n_central_meridian_on_equator() {
    let (x, y) = first_xy(&pt(-81.0, 0.0, "EPSG:4326").to_crs("EPSG:32617").unwrap());
    assert!((x - 500_000.0).abs() < 1e-3 && y.abs() < 1e-3, "{x} {y}");
}

#[test]
fn utm_matches_independent_series() {
    let cases = [
        (-80.19, 25.76, 17, true, "EPSG:32617"),
        (-78.5, 40.0, 17, true, "EPSG:32617"),
        (151.2, -33.87, 56, false, "EPSG:32756"),
    ];
    for (lon, lat, zone, north, code) in cases {
        let (x, y) = first_xy(&pt(lon, lat, "EPSG:4326").to_crs(code).unwrap());
        let (ex, ey) = utm_krueger(lon, lat, zone, north);
        assert!(
            (x - ex).abs() < 1e-3 && (y - ey).abs() < 1e-3,
            "{code}: {x} {y} vs {ex} {ey}"
        );
    }
}

#[test]
fn florida_east_us_feet_false_easting() {
    // EPSG:2236: central meridian -81, latitude of origin 24 20' N, false easting 200000 m,
    // US survey feet. On the origin the result is (200000 / 0.3048006096, 0) ftUS.
    let (x, y) = first_xy(
        &pt(-81.0, 24.0 + 20.0 / 60.0, "EPSG:4326")
            .to_crs("EPSG:2236")
            .unwrap(),
    );
    let expected_x = 200_000.0 / 0.304_800_609_601_219_2;
    assert!((x - expected_x).abs() < 1e-3 && y.abs() < 1e-3, "{x} {y}");
}

#[test]
fn round_trips_through_projected_crs() {
    for target in ["EPSG:3857", "EPSG:32617", "EPSG:2236"] {
        for (lon, lat) in [(-80.19, 25.76), (-81.5, 27.1), (-79.9, 26.0)] {
            let back = pt(lon, lat, "EPSG:4326")
                .to_crs(target)
                .unwrap()
                .to_crs("EPSG:4326")
                .unwrap();
            let (x, y) = first_xy(&back);
            assert!(
                (x - lon).abs() < 1e-9 && (y - lat).abs() < 1e-9,
                "{target}: {x} {y}"
            );
        }
    }
}

#[test]
fn polygon_area_scales_with_metres_per_degree() {
    let square = GeoColumn::from_wkt(
        &[Some("POLYGON ((0 0, 0.01 0, 0.01 0.01, 0 0.01, 0 0))")],
        Some("EPSG:4326"),
    )
    .unwrap();
    let metres = square.to_crs("EPSG:3857").unwrap().area()[0].unwrap();
    let expected = 0.01 * 0.01 * (R * std::f64::consts::PI / 180.0).powi(2);
    assert!(
        (metres / expected - 1.0).abs() < 0.01,
        "{metres} vs {expected}"
    );
}

#[test]
fn all_geometry_types_keep_their_shape() {
    let wkts = [
        "POINT (-80 25)",
        "MULTIPOINT ((-80 25), (-81 26))",
        "LINESTRING (-80 25, -81 26)",
        "MULTILINESTRING ((-80 25, -81 26), (-82 27, -83 28))",
        "POLYGON ((-80 25, -81 25, -81 26, -80 25), (-80.6 25.2, -80.8 25.2, -80.8 25.4, -80.6 25.2))",
        "MULTIPOLYGON (((-80 25, -81 25, -81 26, -80 25)))",
        "GEOMETRYCOLLECTION (POINT (-80 25), LINESTRING (-80 25, -81 26))",
    ];
    let rows: Vec<_> = wkts.iter().map(|w| Some(*w)).collect();
    let col = GeoColumn::from_wkt(&rows, Some("EPSG:4326")).unwrap();
    let out = col.to_crs("EPSG:3857").unwrap();
    assert_eq!(out.geom_type(), col.geom_type());
    assert_eq!(out.count_coordinates(), col.count_coordinates());
    let (x, _) = first_xy(&out);
    assert!((x - web_mercator(-80.0, 25.0).0).abs() < 1e-3);
}

#[test]
fn nulls_stay_null_and_failures_become_null() {
    let col = GeoColumn::from_wkt(
        &[
            Some("POINT (-80 25)"),
            None,
            Some("POINT (0 90)"),
            Some("POINT (0 95)"),
        ],
        Some("EPSG:4326"),
    )
    .unwrap();
    let wkts = col.to_crs("EPSG:3857").unwrap().to_wkt().unwrap();
    assert!(wkts[0].is_some());
    assert!(wkts[1].is_none());
    // Web Mercator is undefined at the pole and beyond it.
    assert!(wkts[2].is_none(), "{:?}", wkts[2]);
    assert!(wkts[3].is_none(), "{:?}", wkts[3]);
}

#[test]
fn output_is_two_dimensional() {
    let col = GeoColumn::from_wkt(&[Some("POINT Z (-80 25 7)")], Some("EPSG:4326")).unwrap();
    let wkt = col.to_crs("EPSG:3857").unwrap().to_wkt().unwrap()[0]
        .clone()
        .unwrap();
    assert!(!wkt.contains('Z'), "{wkt}");
    let coords = wkt.split_once('(').unwrap().1.trim_end_matches(')');
    assert_eq!(coords.split_whitespace().count(), 2, "{wkt}");
}

#[test]
fn same_crs_is_unchanged_and_crs84_is_lon_lat() {
    let col = pt(-80.19, 25.76, "EPSG:4326");
    let same = col.to_crs("epsg:4326").unwrap();
    assert_eq!(same.to_wkt().unwrap(), col.to_wkt().unwrap());
    let crs84 = col.to_crs("OGC:CRS84").unwrap();
    assert_eq!(crs84.crs().as_deref(), Some("OGC:CRS84"));
    assert_eq!(first_xy(&crs84), (-80.19, 25.76));
    let merc = crs84.to_crs("EPSG:3857").unwrap();
    assert!((first_xy(&merc).0 - web_mercator(-80.19, 25.76).0).abs() < 1e-3);
}

#[test]
fn source_given_as_projjson_with_id() {
    let projjson =
        r#"{"type":"GeographicCRS","name":"WGS 84","id":{"authority":"EPSG","code":4326}}"#;
    let col = pt(-80.19, 25.76, projjson);
    let out = col.to_crs("EPSG:3857").unwrap();
    assert!((first_xy(&out).0 - web_mercator(-80.19, 25.76).0).abs() < 1e-3);
    assert_eq!(normalize_crs(projjson).unwrap(), "EPSG:4326");
}

#[test]
fn errors_are_clear() {
    let naive = GeoColumn::from_wkt(&[Some("POINT (1 1)")], None).unwrap();
    let err = naive.to_crs("EPSG:3857").unwrap_err();
    assert!(err.contains("naive") && err.contains("set_crs"), "{err}");

    let col = pt(1.0, 1.0, "EPSG:4326");
    let err = col.to_crs("EPSG:999999").unwrap_err();
    assert!(err.contains("999999"), "{err}");
    assert!(col.to_crs("EPSG:1").unwrap_err().contains("unknown"));
    assert!(col.to_crs("banana").unwrap_err().contains("banana"));
    assert!(col.to_crs("FOO:3857").unwrap_err().contains("FOO"));

    let no_id = pt(1.0, 1.0, r#"{"type":"GeographicCRS","name":"custom"}"#);
    assert!(no_id.to_crs("EPSG:3857").unwrap_err().contains("PROJJSON"));
}

#[test]
fn normalize_crs_canonicalises_labels() {
    assert_eq!(normalize_crs(" epsg:3857 ").unwrap(), "EPSG:3857");
    assert_eq!(normalize_crs("ogc:crs84").unwrap(), "OGC:CRS84");
    assert!(normalize_crs("EPSG:abc").is_err());
}

#[test]
fn estimate_utm_crs_picks_zone_and_hemisphere() {
    let miami = GeoColumn::from_wkt(
        &[Some("POINT (-80.3 25.6)"), Some("POINT (-80.1 25.9)")],
        Some("EPSG:4326"),
    )
    .unwrap();
    assert_eq!(miami.estimate_utm_crs().unwrap(), "EPSG:32617");
    assert_eq!(
        pt(151.2, -33.87, "EPSG:4326").estimate_utm_crs().unwrap(),
        "EPSG:32756"
    );
    // A projected source is converted to lon/lat first.
    let merc = miami.to_crs("EPSG:3857").unwrap();
    assert_eq!(merc.estimate_utm_crs().unwrap(), "EPSG:32617");
    // Zones stay in 1..=60; longitude 180 wraps to -180, which is zone 1.
    assert_eq!(
        pt(179.9, 10.0, "EPSG:4326").estimate_utm_crs().unwrap(),
        "EPSG:32660"
    );
    assert_eq!(
        pt(180.0, 10.0, "EPSG:4326").estimate_utm_crs().unwrap(),
        "EPSG:32601"
    );
    assert!(pt(0.0, 85.0, "EPSG:4326").estimate_utm_crs().is_err());
    assert!(GeoColumn::from_wkt(&[Some("POINT (1 1)")], None)
        .unwrap()
        .estimate_utm_crs()
        .is_err());
    assert!(GeoColumn::from_wkt(&[None::<&str>], Some("EPSG:4326"))
        .unwrap()
        .estimate_utm_crs()
        .is_err());
}
