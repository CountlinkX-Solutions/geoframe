//! CRS text conversions for the formats that store a WKT definition (Shapefile `.prj`,
//! GeoPackage `gpkg_spatial_ref_sys`).
//!
//! # Reading
//!
//! A WKT definition is mapped to an `EPSG:<code>` string when it names one: either through a
//! top-level `AUTHORITY["EPSG","<code>"]` (OGC WKT1) or `ID["EPSG",<code>]` (WKT2), or, for the
//! ESRI flavour that GDAL writes into `.prj` files (which carries no authority), through a
//! small table of well-known names (`GCS_WGS_1984`, `WGS_1984_Web_Mercator_Auxiliary_Sphere`,
//! `WGS_1984_UTM_Zone_17N`, `NAD_1983_UTM_Zone_17N`, ...). Any other definition is kept as the
//! WKT text itself, so it still round-trips.
//!
//! # Writing
//!
//! `EPSG:<code>` (and `OGC:CRS84`, and PROJJSON carrying an EPSG identifier) is written as the
//! OGC WKT1 definition from the `crs-definitions` table, which covers the EPSG registry GeoFrame
//! can reproject. GDAL writes the ESRI flavour instead; both are read by GDAL, GeoPandas and
//! pyproj. WKT text is written verbatim. Anything else has no definition here and is `None`.

use serde_json::Value;

const WKT_ROOTS: [&str; 10] = [
    "GEOGCS[",
    "PROJCS[",
    "GEOGCRS[",
    "PROJCRS[",
    "GEODCRS[",
    "BOUNDCRS[",
    "COMPOUNDCRS[",
    "COMPD_CS[",
    "GEOCCS[",
    "GEODETICCRS[",
];

/// Whether `text` starts like a WKT coordinate system definition.
pub(crate) fn looks_like_wkt(text: &str) -> bool {
    let upper = text.trim_start().to_ascii_uppercase();
    WKT_ROOTS.iter().any(|root| upper.starts_with(root))
}

/// The OGC WKT1 definition of an EPSG code, when the bundled table has it.
pub(crate) fn epsg_definition(code: i64) -> Option<&'static str> {
    let code = u16::try_from(code).ok()?;
    crs_definitions::from_code(code).map(|def| def.wkt)
}

/// The authority and numeric code a CRS string names: `EPSG:4326`, `OGC:CRS84` (as EPSG 4326),
/// PROJJSON with an `id`, or WKT that maps to an EPSG code. `None` for anything else.
pub(crate) fn crs_to_authority(crs: &str) -> Option<(String, i64)> {
    let text = crs.trim();
    if text.starts_with('{') {
        let value: Value = serde_json::from_str(text).ok()?;
        let id = value.get("id")?;
        let authority = id.get("authority")?.as_str()?.to_string();
        let code = match id.get("code")? {
            Value::Number(n) => n.as_i64()?,
            Value::String(s) => s.parse().ok()?,
            _ => return None,
        };
        return Some((authority.to_ascii_uppercase(), code));
    }
    if text.eq_ignore_ascii_case("OGC:CRS84")
        || text.eq_ignore_ascii_case("urn:ogc:def:crs:OGC:1.3:CRS84")
    {
        return Some(("EPSG".to_string(), 4326));
    }
    if looks_like_wkt(text) {
        let mapped = prj_wkt_to_crs(text);
        return mapped
            .strip_prefix("EPSG:")
            .and_then(|code| code.parse().ok())
            .map(|code| ("EPSG".to_string(), code));
    }
    let (authority, code) = text.split_once(':')?;
    let authority = authority.trim();
    if authority.is_empty() || authority.contains(char::is_whitespace) {
        return None;
    }
    let code: i64 = code.trim().parse().ok()?;
    (code > 0).then(|| (authority.to_ascii_uppercase(), code))
}

/// The `.prj` text for a CRS string, or `None` when no definition is available.
pub(crate) fn crs_to_prj_wkt(crs: &str) -> Option<String> {
    let text = crs.trim();
    if looks_like_wkt(text) {
        return Some(text.to_string());
    }
    let (authority, code) = crs_to_authority(text)?;
    if authority != "EPSG" {
        return None;
    }
    epsg_definition(code).map(str::to_string)
}

/// The CRS string for a `.prj` / `srs` WKT definition; see the module documentation.
pub(crate) fn prj_wkt_to_crs(wkt: &str) -> String {
    let text = wkt.trim().trim_end_matches('\0').trim();
    if let Some(code) = root_authority_code(text) {
        return format!("EPSG:{code}");
    }
    if let Some(code) = root_name(text).and_then(code_for_name) {
        return format!("EPSG:{code}");
    }
    text.to_string()
}

/// The `EPSG` code of a root-level `AUTHORITY[...]` or `ID[...]` node.
fn root_authority_code(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut found = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            quoted = !quoted;
        } else if !quoted {
            match b {
                b'[' | b'(' => {
                    if depth == 1 {
                        let head = &text[..i];
                        let keyword = head
                            .rsplit(|c: char| !c.is_ascii_alphabetic())
                            .next()
                            .unwrap_or("");
                        if keyword.eq_ignore_ascii_case("AUTHORITY")
                            || keyword.eq_ignore_ascii_case("ID")
                        {
                            if let Some(code) = parse_authority_node(&text[i + 1..]) {
                                found = Some(code);
                            }
                        }
                    }
                    depth += 1;
                }
                b']' | b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        i += 1;
    }
    found
}

/// Parses `"EPSG","4326"]` or `"EPSG",4326]` (the text after the opening bracket).
fn parse_authority_node(rest: &str) -> Option<i64> {
    let end = rest.find(']').or_else(|| rest.find(')'))?;
    let mut parts = rest[..end].split(',').map(|p| p.trim().trim_matches('"'));
    let authority = parts.next()?;
    let code = parts.next()?;
    authority
        .eq_ignore_ascii_case("EPSG")
        .then(|| code.parse().ok())
        .flatten()
}

/// The name of the root node: `WGS_1984_UTM_Zone_17N` in `PROJCS["WGS_1984_UTM_Zone_17N", ...]`.
fn root_name(text: &str) -> Option<&str> {
    let open = text.find('[')?;
    let rest = text[open + 1..].trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn code_for_name(name: &str) -> Option<i64> {
    let normalized: String = name
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect::<String>()
        .to_ascii_uppercase();
    match normalized.as_str() {
        "GCS_WGS_1984" | "WGS_84" | "WGS84" => return Some(4326),
        "GCS_NORTH_AMERICAN_1983" | "NAD83" | "NAD_83" => return Some(4269),
        "GCS_NORTH_AMERICAN_1927" | "NAD27" | "NAD_27" => return Some(4267),
        "GCS_ETRS_1989" | "ETRS89" => return Some(4258),
        "WGS_1984_WEB_MERCATOR_AUXILIARY_SPHERE" | "WGS_84_/_PSEUDO_MERCATOR" => return Some(3857),
        "BRITISH_NATIONAL_GRID" | "OSGB_1936_/_BRITISH_NATIONAL_GRID" => return Some(27700),
        _ => {}
    }
    utm_code(&normalized, "WGS_1984_UTM_ZONE_", 32600, 32700, 60)
        .or_else(|| utm_code(&normalized, "WGS_84_/_UTM_ZONE_", 32600, 32700, 60))
        .or_else(|| utm_code(&normalized, "NAD_1983_UTM_ZONE_", 26900, 0, 23))
        .or_else(|| utm_code(&normalized, "NAD83_/_UTM_ZONE_", 26900, 0, 23))
}

/// `PREFIX<zone><N|S>` to its EPSG code; `south_base` of 0 means the datum has no southern zones.
fn utm_code(
    name: &str,
    prefix: &str,
    north_base: i64,
    south_base: i64,
    max_zone: i64,
) -> Option<i64> {
    let rest = name.strip_prefix(prefix)?;
    let hemisphere = rest.chars().last()?;
    let zone: i64 = rest[..rest.len() - 1].parse().ok()?;
    if !(1..=max_zone).contains(&zone) {
        return None;
    }
    match hemisphere {
        'N' => Some(north_base + zone),
        'S' if south_base != 0 => Some(south_base + zone),
        _ => None,
    }
}
