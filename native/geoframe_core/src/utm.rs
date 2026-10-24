//! The EPSG UTM CRS lookup behind `estimate_utm_crs`.
//!
//! GeoPandas asks `pyproj.database.query_utm_crs_info(datum_name, area_of_interest)` with the
//! centre of the data as a degenerate area of interest and takes the first answer. The
//! database lists the EPSG projected CRSs named "<datum> / UTM zone ..." in ascending code
//! order, keeps those whose area of use intersects the area of interest (bounds inclusive) and
//! compares the datum with the CRS name up to the slash, ignoring spaces. This module is the
//! same query over the table in `utm_crs.rs`, which is generated from that database.

use crate::utm_crs::UTM_CRS;

fn without_spaces(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().filter(|c| *c != ' ')
}

/// The EPSG code of the first UTM CRS of `datum` (spaces ignored, otherwise an exact match)
/// whose area of use contains the lon/lat point.
pub(crate) fn utm_crs_code(datum: &str, lon: f64, lat: f64) -> Option<u16> {
    UTM_CRS
        .iter()
        .find(|(_, name, west, south, east, north)| {
            let in_lon = if west <= east {
                (*west..=*east).contains(&lon)
            } else {
                // The area of use crosses the antimeridian.
                lon >= *west || lon <= *east
            };
            in_lon
                && (*south..=*north).contains(&lat)
                && without_spaces(name).eq(without_spaces(datum))
        })
        .map(|row| row.0)
}
