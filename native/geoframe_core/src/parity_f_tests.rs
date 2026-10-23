//! Reference cases for batch F: geodesic measures on WGS 84.

use crate::GeoColumn;

fn col(rows: &[Option<&str>], crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(rows, crs).expect("valid WKT")
}

fn one(text: &str, crs: Option<&str>) -> GeoColumn {
    col(&[Some(text)], crs)
}

fn first(values: Vec<Option<f64>>) -> f64 {
    values[0].expect("a value")
}

/// Karney's published example: `Geodesic.WGS84.Inverse(-41.32, 174.81, 40.96, -5.50)`.
#[test]
fn distance_matches_karney_published_value() {
    let a = one("POINT (174.81 -41.32)", Some("EPSG:4326"));
    let b = one("POINT (-5.50 40.96)", Some("EPSG:4326"));
    let d = first(a.distance_geodesic(&b, false).unwrap());
    assert!((d - 19_959_679.267).abs() < 1e-3, "{d}");
}

#[test]
fn haversine_uses_the_mean_earth_radius() {
    let a = one("POINT (0 0)", Some("EPSG:4326"));
    let b = one("POINT (90 0)", Some("EPSG:4326"));
    let d = first(a.distance_geodesic(&b, true).unwrap());
    let quarter = std::f64::consts::FRAC_PI_2 * 6_371_008.8;
    assert!((d - quarter).abs() < 1e-3, "{d} vs {quarter}");
}

#[test]
fn distance_requires_a_crs() {
    let a = one("POINT (0 0)", None);
    let err = a.distance_geodesic(&a, false).unwrap_err();
    assert!(err.contains("set_crs"), "{err}");
}

#[test]
fn distance_rejects_differing_crs() {
    let a = one("POINT (0 0)", Some("EPSG:4326"));
    let b = one("POINT (0 0)", Some("EPSG:3857"));
    assert!(a.distance_geodesic(&b, false).is_err());
}

#[test]
fn distance_broadcasts_and_keeps_nulls() {
    let a = col(
        &[Some("POINT (0 0)"), None, Some("POINT (0 1)")],
        Some("EPSG:4326"),
    );
    let b = one("POINT (0 0)", Some("EPSG:4326"));
    let d = a.distance_geodesic(&b, false).unwrap();
    assert_eq!(d[0], Some(0.0));
    assert_eq!(d[1], None);
    assert!((d[2].unwrap() - 110_574.388).abs() < 1.0, "{:?}", d[2]);
}

#[test]
fn distance_between_non_points_uses_planar_closest_points() {
    // A point 1 degree east of a north-south segment on the equator side.
    let line = one("LINESTRING (0 -1, 0 1)", Some("EPSG:4326"));
    let point = one("POINT (1 0)", Some("EPSG:4326"));
    let d = first(line.distance_geodesic(&point, false).unwrap());
    assert!((d - 111_319.49).abs() < 1.0, "{d}");
    // Intersecting geometries are at distance zero.
    let crossing = one("LINESTRING (-1 0, 1 0)", Some("EPSG:4326"));
    assert_eq!(
        first(line.distance_geodesic(&crossing, false).unwrap()),
        0.0
    );
}

#[test]
fn projected_inputs_are_reprojected_first() {
    let lonlat = one("POINT (0 0)", Some("EPSG:4326"));
    let other = one("POINT (1 0)", Some("EPSG:4326"));
    let want = first(lonlat.distance_geodesic(&other, false).unwrap());
    let a = lonlat.to_crs("EPSG:3857").unwrap();
    let b = other.to_crs("EPSG:3857").unwrap();
    let got = first(a.distance_geodesic(&b, false).unwrap());
    assert!((got - want).abs() < 1e-3, "{got} vs {want}");
}

/// Area of the zone between two parallels over `dlon` degrees on the WGS 84 ellipsoid.
fn zone_area(lat1: f64, lat2: f64, dlon: f64) -> f64 {
    let a: f64 = 6_378_137.0;
    let f: f64 = 1.0 / 298.257_223_563;
    let e2 = f * (2.0 - f);
    let e = e2.sqrt();
    let b2 = a * a * (1.0 - e2);
    let q = |phi: f64| {
        let s = phi.to_radians().sin();
        s / (1.0 - e2 * s * s) + ((1.0 + e * s) / (1.0 - e * s)).ln() / (2.0 * e)
    };
    0.5 * b2 * dlon.to_radians() * (q(lat2) - q(lat1))
}

#[test]
fn area_of_equatorial_cell_matches_the_closed_form() {
    let cell = one("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", Some("EPSG:4326"));
    let area = first(cell.area_geodesic().unwrap());
    let want = zone_area(0.0, 1.0, 1.0);
    assert!((want - 12_308_778_361.0).abs() / want < 1e-4, "{want}");
    assert!((area - want).abs() / want < 1e-4, "{area} vs {want}");
}

#[test]
fn area_is_unsigned_and_subtracts_holes() {
    let cw = one("POLYGON ((0 0, 0 1, 1 1, 1 0, 0 0))", Some("EPSG:4326"));
    let ccw = one("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", Some("EPSG:4326"));
    let (a, b) = (
        first(cw.area_geodesic().unwrap()),
        first(ccw.area_geodesic().unwrap()),
    );
    assert!(a > 0.0 && (a - b).abs() < 1e-3, "{a} vs {b}");
    let holed = one(
        "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0), (0.25 0.25, 0.75 0.25, 0.75 0.75, 0.25 0.75, 0.25 0.25))",
        Some("EPSG:4326"),
    );
    let h = first(holed.area_geodesic().unwrap());
    assert!((h - 0.75 * b).abs() / b < 1e-3, "{h}");
}

#[test]
fn area_is_zero_for_points_and_lines_and_null_for_nulls() {
    let c = col(
        &[Some("POINT (0 0)"), Some("LINESTRING (0 0, 1 1)"), None],
        Some("EPSG:4326"),
    );
    assert_eq!(c.area_geodesic().unwrap(), vec![Some(0.0), Some(0.0), None]);
}

#[test]
fn area_requires_a_crs_and_handles_projected_input() {
    let naive = one("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", None);
    assert!(naive.area_geodesic().unwrap_err().contains("set_crs"));
    let lonlat = one("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", Some("EPSG:4326"));
    let want = first(lonlat.area_geodesic().unwrap());
    let got = first(lonlat.to_crs("EPSG:3857").unwrap().area_geodesic().unwrap());
    assert!((got - want).abs() / want < 1e-6, "{got} vs {want}");
}

#[test]
fn length_of_a_line_is_the_sum_of_segment_distances() {
    let line = one("LINESTRING (0 0, 10 0, 10 10)", Some("EPSG:4326"));
    let got = first(line.length_geodesic().unwrap());
    let seg = |a: &str, b: &str| {
        first(
            one(a, Some("EPSG:4326"))
                .distance_geodesic(&one(b, Some("EPSG:4326")), false)
                .unwrap(),
        )
    };
    let want = seg("POINT (0 0)", "POINT (10 0)") + seg("POINT (10 0)", "POINT (10 10)");
    assert!((got - want).abs() < 1e-6, "{got} vs {want}");
}

#[test]
fn length_covers_every_geometry_family() {
    let c = col(
        &[
            Some("POINT (0 0)"),
            Some("MULTILINESTRING ((0 0, 1 0), (0 0, 0 1))"),
            Some("POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"),
            Some("GEOMETRYCOLLECTION (LINESTRING (0 0, 1 0), POINT (3 3))"),
            None,
        ],
        Some("EPSG:4326"),
    );
    let l = c.length_geodesic().unwrap();
    assert_eq!(l[0], Some(0.0));
    assert!((l[1].unwrap() - (111_319.491 + 110_574.389)).abs() < 1.0);
    // Perimeter of the 1 degree cell: 2 parallels (~111.3 km, ~111.3 km) and 2 meridians.
    assert!((l[2].unwrap() - 443_770.0).abs() < 100.0, "{:?}", l[2]);
    assert!((l[3].unwrap() - 111_319.491).abs() < 1.0);
    assert_eq!(l[4], None);
}

#[test]
fn length_requires_a_crs() {
    let naive = one("LINESTRING (0 0, 1 1)", None);
    assert!(naive.length_geodesic().unwrap_err().contains("set_crs"));
}
