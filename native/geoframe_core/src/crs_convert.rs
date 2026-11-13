//! A CRS definition (PROJJSON, or WKT1/WKT2 translated to PROJJSON) to the proj string the
//! engine runs, like `pyproj.CRS.from_user_input(...).to_proj4()` followed by `Transformer`.
//!
//! The datum comes from the geographic CRS under a projected one: its ellipsoid (`+a` with `+rf`
//! or `+b`, `+R` for a sphere), prime meridian (`+pm`, in degrees) and, for a bound CRS, the
//! `+towgs84` of its abridged transformation. Datum shifts by area of use and grids are keyed by
//! the EPSG code of that geographic CRS (see [`crate::datum`]), so a definition that carries the
//! identifier of its base CRS shifts exactly like the code does; one that does not keeps its
//! coordinates on the datum shift PROJ would call a ballpark (none), like pyproj does for a
//! definition PROJ cannot match to a database CRS.
//!
//! The conversion method is found by its EPSG code (or its EPSG name when the definition has no
//! identifiers) and its parameters by their EPSG codes or names; angles, lengths and scales are
//! converted from the units the definition states. The projection axes give `+axis` (only a
//! southing or westing changes the engine's east/north convention) and the linear unit
//! `+to_meter`.
//!
//! Methods: Transverse Mercator (also south orientated), Lambert Conic Conformal (1SP, 2SP,
//! 1SP variant B, 2SP Michigan, west orientated), Albers, Lambert Azimuthal Equal Area,
//! Azimuthal Equidistant (and Guam), Oblique and Polar Stereographic (A, B), Hotine Oblique
//! Mercator (A, B; the Swiss case is `somerc`), Mercator (A, B, Pseudo-Mercator), Cassini-Soldner
//! (also Hyperbolic), Equidistant Cylindrical, Lambert Cylindrical Equal Area, Krovak (also
//! Modified), Laborde, Colombia Urban, American Polyconic, New Zealand Map Grid and Equal Earth.
//! Any other method is refused with its name.

use std::f64::consts::PI;

use serde_json::Value;

const DEGREE: f64 = PI / 180.0;

/// What the engine needs of a CRS definition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Definition {
    /// The proj string (`+proj=longlat ...` for a geographic CRS).
    pub(crate) proj: String,
    /// EPSG code of the geographic CRS the definition is based on, the key of its grids.
    pub(crate) base: Option<u32>,
    /// Radians per unit of a geographic CRS.
    pub(crate) angular: f64,
}

/// Converts a PROJJSON object.
pub(crate) fn from_projjson(value: &Value) -> Result<Definition, String> {
    if let Some(reason) = unshiftable_prime_meridian(value) {
        return Err(reason);
    }
    convert(value)
}

fn convert(value: &Value) -> Result<Definition, String> {
    match type_of(value)? {
        "GeographicCRS" | "GeodeticCRS" => geodetic(value),
        "ProjectedCRS" => projected(value),
        "BoundCRS" => bound(value),
        "CompoundCRS" => {
            let first = value
                .get("components")
                .and_then(Value::as_array)
                .and_then(|c| c.first())
                .ok_or("a compound CRS without components")?;
            convert(first)
        }
        other => Err(format!("unsupported CRS type {other:?}")),
    }
}

fn type_of(value: &Value) -> Result<&str, String> {
    value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "a PROJJSON CRS needs a `type`".to_string())
}

/// The EPSG code of a geographic CRS: its identifier, else (as PROJ identifies a CRS) the CRS of
/// that name that has datum shifts, else the one whose datum has that name. PROJ identifies the
/// datum of a WKT1 text by its name (`D_OSGB_1936` of a Shapefile `.prj`, `European_Datum_1950`
/// of GDAL's text, through the EPSG and ESRI aliases) and shifts it like the code, whatever the
/// name of the CRS.
fn geographic_code(value: &Value) -> Option<u32> {
    let id = epsg_code(value);
    geographic_code_without_datum_name(value)
        .or_else(|| {
            let datum = value.get("datum")?;
            let pm = prime_meridian_degrees(datum);
            // The CRS of the datum on that prime meridian (MGI on Ferro is MGI (Ferro), not MGI).
            // A geographic text whose answer by that code differs from pyproj is refused before
            // this point (see `unshiftable_prime_meridian`).
            datum_lookup(datum.get("name")?.as_str()?, pm).map(|(code, _)| code)
        })
        .or(id)
}

/// The longitude of the prime meridian of a datum in degrees (0 for Greenwich or none).
fn prime_meridian_degrees(datum: &Value) -> f64 {
    let Some(longitude) = datum
        .get("prime_meridian")
        .and_then(|pm| pm.get("longitude"))
    else {
        return 0.0;
    };
    match longitude {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        other => quantity(other).map_or(0.0, |q| q / DEGREE),
    }
}

/// The CRS (and whether its by-code answer agrees with pyproj) of a datum name on a prime meridian.
fn datum_lookup(name: &str, pm: f64) -> Option<(u32, bool)> {
    let name = name.to_ascii_lowercase().replace('_', " ");
    let table = crate::crs_datum_names::DATUM_NAMES;
    let start = table.partition_point(|(n, ..)| *n < name.as_str());
    table[start..]
        .iter()
        .take_while(|(n, ..)| *n == name)
        .find(|(_, p, ..)| (p - pm).abs() < 5e-6)
        .map(|(_, _, code, agrees)| (*code, *agrees))
}

/// A CRS with that datum name on any prime meridian.
fn datum_known(name: &str) -> Option<u32> {
    let name = name.to_ascii_lowercase().replace('_', " ");
    let table = crate::crs_datum_names::DATUM_NAMES;
    let start = table.partition_point(|(n, ..)| *n < name.as_str());
    table.get(start).filter(|(n, ..)| *n == name).map(|r| r.2)
}

/// The code from the identifier or the name of the CRS, before the datum name is tried.
fn geographic_code_without_datum_name(value: &Value) -> Option<u32> {
    let id = epsg_code(value);
    // A 3D geographic CRS (LUREF 3D) has no operations of its own: its 2D namesake has them.
    id.filter(|code| crate::datum::has_operations(*code))
        .or_else(|| {
            let name = value.get("name")?.as_str()?;
            lookup(crate::crs_names::CRS_NAMES, name)
        })
}

/// Why a definition with a prime meridian other than Greenwich and no identifier is refused, when
/// PROJ shifts its datum to WGS 84 and GeoFrame would not: a name finds the datum shifts only for a
/// Greenwich-based definition (the table is Greenwich-based and the shifts are chosen by the
/// longitude of the point), so an answer would silently leave the datum unshifted, by tens to
/// hundreds of metres (Ferro, Paris, Jakarta, Bern, Oslo, Lisbon). A datum PROJ does not shift
/// (Brussels, Stockholm, Voirol 1879) keeps its answer, and so does a definition with `TOWGS84`
/// (a bound CRS) or one that is looked up by its EPSG code or its name.
fn unshiftable_prime_meridian(value: &Value) -> Option<String> {
    let geographic_top = matches!(
        value.get("type").and_then(Value::as_str),
        Some("GeographicCRS" | "GeodeticCRS")
    );
    let crs = match value.get("type").and_then(Value::as_str)? {
        "ProjectedCRS" => value.get("base_crs")?,
        "CompoundCRS" => value.get("components")?.as_array()?.first()?,
        "GeographicCRS" | "GeodeticCRS" => value,
        _ => return None,
    };
    if matches!(
        crs.get("type").and_then(Value::as_str),
        Some("BoundCRS" | "CompoundCRS" | "ProjectedCRS")
    ) {
        return unshiftable_prime_meridian(crs);
    }
    let datum = crs.get("datum")?;
    datum.get("prime_meridian")?;
    let degrees = prime_meridian_degrees(datum);
    if degrees == 0.0 || geographic_code_without_datum_name(crs).is_some() {
        return None;
    }
    let name = datum.get("name")?.as_str()?;
    let (code, why) = match datum_lookup(name, degrees) {
        // answered through the CRS of the datum on that meridian
        Some((_, true)) => return None,
        // a geographic text whose own answer through that CRS differs from pyproj's (MGI (Ferro)
        // by code, Monte Mario (Rome) by text): refused where PROJ shifts the datum; a projected
        // CRS on it agrees with pyproj and is answered
        Some((code, false)) if geographic_top => {
            (code, "GeoFrame's answer for that CRS differs from PROJ's")
        }
        Some(_) => return None,
        // the name is known, but not on this meridian
        None => (
            datum_known(name)?,
            "no CRS has that datum on that prime meridian",
        ),
    };
    crate::datum::shifts_to_wgs84(code).then(|| {
        format!(
            "the datum {name:?} has a prime meridian at {degrees} degrees and no identifier, and \
             PROJ shifts it to WGS 84; {why}, so the CRS is refused rather than answered off by \
             the datum shift (give it as its EPSG code)"
        )
    })
}

/// The code a sorted name table gives `name` (lower case, `_` as a space).
fn lookup(table: &[(&str, u32)], name: &str) -> Option<u32> {
    let name = name.to_ascii_lowercase().replace('_', " ");
    table
        .binary_search_by(|(n, _)| (*n).cmp(name.as_str()))
        .ok()
        .map(|i| table[i].1)
}

fn epsg_code(value: &Value) -> Option<u32> {
    let id = value.get("id")?;
    if !id.get("authority")?.as_str()?.eq_ignore_ascii_case("EPSG") {
        return None;
    }
    match id.get("code")? {
        Value::Number(n) => u32::try_from(n.as_u64()?).ok(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

// -- units ----------------------------------------------------------------------------------

/// Multiplier of a PROJJSON unit: radians for an angular unit, metres for a linear one, unity.
fn unit_factor(unit: Option<&Value>) -> Result<f64, String> {
    match unit {
        None => Ok(1.0),
        Some(Value::String(name)) => match name.as_str() {
            "metre" | "meter" | "unity" | "radian" => Ok(1.0),
            "degree" => Ok(DEGREE),
            "grad" | "gon" => Ok(PI / 200.0),
            "arc-second" => Ok(DEGREE / 3600.0),
            "ppm" | "parts per million" => Ok(1e-6),
            other => Err(format!("unknown unit {other:?}")),
        },
        Some(obj) => obj
            .get("conversion_factor")
            .and_then(Value::as_f64)
            .ok_or_else(|| "a unit without a conversion_factor".to_string()),
    }
}

/// A number that may be written `{"value": v, "unit": u}`, in the base unit (radians for
/// angles, metres for lengths).
fn quantity(value: &Value) -> Result<f64, String> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| "number out of range".to_string()),
        Value::Object(o) => {
            let v = o
                .get("value")
                .and_then(Value::as_f64)
                .ok_or("a quantity without a value")?;
            Ok(v * unit_factor(o.get("unit"))?)
        }
        _ => Err(format!("expected a number, found {value}")),
    }
}

// -- datum ----------------------------------------------------------------------------------

/// `+a/+rf`, `+a/+b` or `+R` of a PROJJSON ellipsoid.
fn ellipsoid(e: &Value) -> Result<String, String> {
    let num = |key: &str| e.get(key).map(quantity).transpose();
    if let Some(r) = num("radius")? {
        return Ok(format!("+R={r}"));
    }
    let a = num("semi_major_axis")?.ok_or("an ellipsoid without semi_major_axis")?;
    if let Some(rf) = num("inverse_flattening")? {
        return Ok(if rf == 0.0 {
            format!("+R={a}")
        } else {
            format!("+a={a} +rf={rf}")
        });
    }
    let b = num("semi_minor_axis")?.ok_or("an ellipsoid without flattening or semi_minor_axis")?;
    Ok(format!("+a={a} +b={b}"))
}

/// The ellipsoid and prime meridian tokens of a geographic or geodetic CRS.
fn datum_tokens(crs: &Value) -> Result<String, String> {
    let datum = crs
        .get("datum")
        .or_else(|| crs.get("datum_ensemble"))
        .ok_or("a CRS without a datum")?;
    let mut out = ellipsoid(
        datum
            .get("ellipsoid")
            .ok_or("a datum without an ellipsoid")?,
    )?;
    if let Some(pm) = datum.get("prime_meridian") {
        let longitude = pm
            .get("longitude")
            .ok_or("a prime meridian without longitude")?;
        // A bare number is in degrees, a quantity states its unit.
        let degrees = match longitude {
            Value::Number(n) => n.as_f64().unwrap_or(0.0),
            other => quantity(other)? / DEGREE,
        };
        if degrees != 0.0 {
            out.push_str(&format!(" +pm={degrees}"));
        }
    }
    Ok(out)
}

fn geodetic(crs: &Value) -> Result<Definition, String> {
    let datum = datum_tokens(crs)?;
    let cs = crs.get("coordinate_system");
    let cartesian = cs
        .and_then(|c| c.get("subtype"))
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("cartesian"));
    if cartesian {
        let to_meter = axes_of(cs)?.map_or(1.0, |a| a.unit);
        return Ok(Definition {
            proj: format!("+proj=geocent {datum}{}", to_meter_token(to_meter)),
            base: geographic_code(crs),
            angular: DEGREE,
        });
    }
    let angular = axes_of(cs)?.map_or(DEGREE, |a| a.unit);
    // The degree is written 0.0174532925199433 in some sources: keep the exact one.
    let angular = if (angular - DEGREE).abs() < 1e-12 {
        DEGREE
    } else {
        angular
    };
    Ok(Definition {
        proj: format!("+proj=longlat {datum} +no_defs"),
        base: geographic_code(crs),
        angular,
    })
}

// -- coordinate system ------------------------------------------------------------------------

struct Axes {
    /// Unit of the first axis (radians or metres).
    unit: f64,
    /// Directions of the first two axes as `+axis` letters.
    directions: Vec<char>,
}

fn axes_of(cs: Option<&Value>) -> Result<Option<Axes>, String> {
    let Some(list) = cs.and_then(|c| c.get("axis")).and_then(Value::as_array) else {
        return Ok(None);
    };
    let Some(first) = list.first() else {
        return Ok(None);
    };
    // Polar axes ("south along 90 deg East") name a meridian: they are the usual easting and
    // northing, which PROJ writes without `+axis`.
    let polar = list.iter().take(2).any(|a| a.get("meridian").is_some());
    let directions = if polar {
        vec!['e', 'n']
    } else {
        list.iter()
            .take(2)
            .filter_map(|a| match a.get("direction")?.as_str()? {
                "east" => Some('e'),
                "west" => Some('w'),
                "north" => Some('n'),
                "south" => Some('s'),
                _ => None,
            })
            .collect()
    };
    Ok(Some(Axes {
        unit: unit_factor(first.get("unit"))?,
        directions,
    }))
}

fn to_meter_token(factor: f64) -> String {
    if factor == 1.0 {
        String::new()
    } else {
        format!(" +to_meter={factor}")
    }
}

/// `+axis` when an axis points south or west (a swapped north/east pair is what `always_xy`
/// undoes, and `+axis=enu` is the default).
fn axis_token(axes: &Axes) -> String {
    if axes.directions.len() == 2 && axes.directions.iter().any(|d| matches!(d, 'w' | 's')) {
        format!(" +axis={}{}u", axes.directions[0], axes.directions[1])
    } else {
        String::new()
    }
}

// -- projections ------------------------------------------------------------------------------

/// EPSG parameter codes by their lower-case EPSG names, for definitions without identifiers.
const PARAMETER_NAMES: &[(&str, u32)] = &[
    ("latitude of natural origin", 8801),
    ("longitude of natural origin", 8802),
    ("scale factor at natural origin", 8805),
    ("false easting", 8806),
    ("false northing", 8807),
    ("latitude of projection centre", 8811),
    ("longitude of projection centre", 8812),
    ("azimuth at projection centre", 8813),
    ("azimuth of initial line", 8813),
    ("angle from rectified to skew grid", 8814),
    ("scale factor on initial line", 8815),
    ("scale factor at projection centre", 8815),
    ("easting at projection centre", 8816),
    ("northing at projection centre", 8817),
    ("latitude of pseudo standard parallel", 8818),
    ("scale factor on pseudo standard parallel", 8819),
    ("latitude of false origin", 8821),
    ("longitude of false origin", 8822),
    ("latitude of 1st standard parallel", 8823),
    ("latitude of 2nd standard parallel", 8824),
    ("easting at false origin", 8826),
    ("northing at false origin", 8827),
    ("latitude of standard parallel", 8832),
    ("longitude of origin", 8833),
    ("co-latitude of cone axis", 1036),
    ("ellipsoid scaling factor", 1038),
    ("projection plane origin height", 1039),
];

/// EPSG method codes by their lower-case EPSG names.
const METHOD_NAMES: &[(&str, u32)] = &[
    ("transverse mercator", 9807),
    ("transverse mercator (south orientated)", 9808),
    ("transverse mercator 3d", 1111),
    ("lambert conic conformal (1sp)", 9801),
    ("lambert conic conformal (2sp)", 9802),
    ("lambert conic conformal (2sp belgium)", 9803),
    ("lambert conic conformal (2sp michigan)", 1051),
    ("lambert conic conformal (1sp variant b)", 1102),
    ("lambert conic conformal (west orientated)", 9826),
    ("albers equal area", 9822),
    ("lambert azimuthal equal area", 9820),
    ("lambert azimuthal equal area (spherical)", 1027),
    ("azimuthal equidistant", 1125),
    ("modified azimuthal equidistant", 9832),
    ("guam projection", 9831),
    ("oblique stereographic", 9809),
    ("polar stereographic (variant a)", 9810),
    ("polar stereographic (variant b)", 9829),
    ("hotine oblique mercator (variant a)", 9812),
    ("hotine oblique mercator (variant b)", 9815),
    ("mercator (variant a)", 9804),
    ("mercator (variant b)", 9805),
    ("mercator (spherical)", 1026),
    ("popular visualisation pseudo mercator", 1024),
    ("cassini-soldner", 9806),
    ("hyperbolic cassini-soldner", 9833),
    ("equidistant cylindrical", 1028),
    ("equidistant cylindrical (spherical)", 1029),
    ("lambert cylindrical equal area", 9835),
    ("lambert cylindrical equal area (spherical)", 9834),
    ("krovak", 9819),
    ("krovak (north orientated)", 1041),
    ("krovak modified", 1042),
    ("krovak modified (north orientated)", 1043),
    ("laborde oblique mercator", 9813),
    ("colombia urban", 1052),
    ("american polyconic", 9818),
    ("new zealand map grid", 9811),
    ("equal earth", 1078),
];

/// The parameters of a conversion, in degrees, metres and unity, by EPSG code.
struct Parameters(Vec<(u32, f64)>);

impl Parameters {
    fn read(conversion: &Value) -> Result<Self, String> {
        let mut out = Vec::new();
        for p in conversion
            .get("parameters")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            let code = epsg_code(p).or_else(|| {
                let lower = name.to_ascii_lowercase();
                PARAMETER_NAMES
                    .iter()
                    .find(|(n, _)| *n == lower)
                    .map(|(_, c)| *c)
            });
            let Some(code) = code else { continue };
            let value = p
                .get("value")
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("parameter {name:?} has no value"))?;
            // Angles are kept in degrees, lengths in metres.
            let unit = p.get("unit");
            let factor = unit_factor(unit)?;
            let angular = unit.is_some_and(|u| match u {
                Value::String(s) => matches!(
                    s.as_str(),
                    "degree" | "grad" | "gon" | "radian" | "arc-second"
                ),
                o => o.get("type").and_then(Value::as_str) == Some("AngularUnit"),
            });
            out.push((
                code,
                if angular {
                    value * factor / DEGREE
                } else {
                    value * factor
                },
            ));
        }
        Ok(Self(out))
    }

    fn get(&self, codes: &[u32]) -> Option<f64> {
        codes
            .iter()
            .find_map(|c| self.0.iter().find(|(k, _)| k == c).map(|(_, v)| *v))
    }

    /// The first of `codes` that is present, else 0.
    fn or_zero(&self, codes: &[u32]) -> f64 {
        self.get(codes).unwrap_or(0.0)
    }

    fn need(&self, codes: &[u32], what: &str) -> Result<f64, String> {
        self.get(codes)
            .ok_or_else(|| format!("the conversion lacks the {what}"))
    }

    fn scale(&self, codes: &[u32]) -> f64 {
        self.get(codes).unwrap_or(1.0)
    }
}

/// The projection tokens of a conversion.
fn projection(conversion: &Value, axes: &Axes) -> Result<String, String> {
    let method = conversion
        .get("method")
        .ok_or("a conversion without method")?;
    let method_name = method.get("name").and_then(Value::as_str).unwrap_or("");
    let code = epsg_code(method)
        .or_else(|| {
            let lower = method_name.to_ascii_lowercase();
            METHOD_NAMES
                .iter()
                .find(|(n, _)| *n == lower)
                .map(|(_, c)| *c)
        })
        .ok_or_else(|| format!("unsupported projection method {method_name:?}"))?;
    let p = Parameters::read(conversion)?;
    let natural = |p: &Parameters| {
        Ok::<_, String>((
            p.or_zero(&[8801, 8811]),
            p.or_zero(&[8802, 8833, 8812]),
            p.or_zero(&[8806]),
            p.or_zero(&[8807]),
        ))
    };
    let s = match code {
        // Transverse Mercator (also 3D and south orientated: the axes carry the orientation)
        9807 | 9808 | 1111 => {
            let (lat, lon, x, y) = natural(&p)?;
            format!(
                "+proj=tmerc +lat_0={lat} +lon_0={lon} +k={} +x_0={x} +y_0={y}",
                p.scale(&[8805])
            )
        }
        9801 | 9826 => {
            let (lat, lon, x, y) = natural(&p)?;
            format!(
                "+proj=lcc +lat_1={lat} +lat_0={lat} +lon_0={lon} +k_0={} +x_0={x} +y_0={y}",
                p.scale(&[8805])
            )
        }
        // Belgium 2SP differs from 2SP only in the (given) false origin on the pole.
        9802 | 9803 | 1051 => format!(
            "+proj=lcc +lat_0={} +lon_0={} +lat_1={} +lat_2={} +x_0={} +y_0={} +k_0={}",
            p.or_zero(&[8821]),
            p.or_zero(&[8822]),
            p.need(&[8823], "latitude of the 1st standard parallel")?,
            p.need(&[8824], "latitude of the 2nd standard parallel")?,
            p.or_zero(&[8826]),
            p.or_zero(&[8827]),
            p.scale(&[1038])
        ),
        1102 => format!(
            "+proj=lcc +lat_1={} +k_0={} +lat_0={} +lon_0={} +x_0={} +y_0={}",
            p.or_zero(&[8801]),
            p.scale(&[8805]),
            p.or_zero(&[8821]),
            p.or_zero(&[8822]),
            p.or_zero(&[8826]),
            p.or_zero(&[8827])
        ),
        9822 => format!(
            "+proj=aea +lat_0={} +lon_0={} +lat_1={} +lat_2={} +x_0={} +y_0={}",
            p.or_zero(&[8821]),
            p.or_zero(&[8822]),
            p.need(&[8823], "latitude of the 1st standard parallel")?,
            p.need(&[8824], "latitude of the 2nd standard parallel")?,
            p.or_zero(&[8826]),
            p.or_zero(&[8827])
        ),
        9820 | 1027 => {
            let (lat, lon, x, y) = natural(&p)?;
            // The spherical variant projects on the sphere of the authalic radius (`+R_A`).
            let sphere = if code == 1027 { " +R_A" } else { "" };
            format!("+proj=laea{sphere} +lat_0={lat} +lon_0={lon} +x_0={x} +y_0={y}")
        }
        1125 | 9832 | 9831 => {
            let (lat, lon, x, y) = natural(&p)?;
            let guam = if code == 9831 { " +guam" } else { "" };
            format!("+proj=aeqd{guam} +lat_0={lat} +lon_0={lon} +x_0={x} +y_0={y}")
        }
        9809 => {
            let (lat, lon, x, y) = natural(&p)?;
            format!(
                "+proj=sterea +lat_0={lat} +lon_0={lon} +k={} +x_0={x} +y_0={y}",
                p.scale(&[8805])
            )
        }
        9810 => {
            let (lat, lon, x, y) = natural(&p)?;
            format!(
                "+proj=stere +lat_0={lat} +lon_0={lon} +k={} +x_0={x} +y_0={y}",
                p.scale(&[8805])
            )
        }
        9829 => {
            let ts = p.need(&[8832], "latitude of standard parallel")?;
            format!(
                "+proj=stere +lat_0={} +lat_ts={ts} +lon_0={} +x_0={} +y_0={}",
                if ts < 0.0 { -90 } else { 90 },
                p.or_zero(&[8833, 8802]),
                p.or_zero(&[8806]),
                p.or_zero(&[8807])
            )
        }
        9812 | 9815 => {
            let (alpha, gamma) = (p.or_zero(&[8813]), p.or_zero(&[8814]));
            let (lat, lonc) = (p.or_zero(&[8811]), p.or_zero(&[8812]));
            let k = p.scale(&[8815]);
            if code == 9815 && alpha == 90.0 && gamma == 90.0 {
                // The Swiss oblique cylindrical projection.
                format!(
                    "+proj=somerc +lat_0={lat} +lon_0={lonc} +k_0={k} +x_0={} +y_0={}",
                    p.or_zero(&[8816]),
                    p.or_zero(&[8817])
                )
            } else if code == 9812 {
                format!(
                    "+proj=omerc +no_uoff +lat_0={lat} +lonc={lonc} +alpha={alpha} +gamma={gamma} +k={k} +x_0={} +y_0={}",
                    p.or_zero(&[8806]),
                    p.or_zero(&[8807])
                )
            } else {
                format!(
                    "+proj=omerc +lat_0={lat} +lonc={lonc} +alpha={alpha} +gamma={gamma} +k={k} +x_0={} +y_0={}",
                    p.or_zero(&[8816]),
                    p.or_zero(&[8817])
                )
            }
        }
        9804 | 1026 => {
            let (_, lon, x, y) = natural(&p)?;
            format!(
                "+proj=merc +lon_0={lon} +k={} +x_0={x} +y_0={y}",
                p.scale(&[8805])
            )
        }
        9805 => format!(
            "+proj=merc +lat_ts={} +lon_0={} +x_0={} +y_0={}",
            p.need(&[8823, 8832], "latitude of the standard parallel")?,
            p.or_zero(&[8802, 8822, 8833]),
            p.or_zero(&[8806, 8826]),
            p.or_zero(&[8807, 8827])
        ),
        // The sphere of the base ellipsoid's semi-major axis with no shift between the sphere and
        // the ellipsoid (what PROJ does); `+nadgrids=@null` keeps the datum WGS 84, so shifts to
        // other datums still apply (see `datum::without_nadgrids`).
        1024 => format!(
            "+proj=merc +lat_ts=0 +lon_0={} +x_0={} +y_0={} +k=1 +nadgrids=@null +wktext",
            p.or_zero(&[8802]),
            p.or_zero(&[8806]),
            p.or_zero(&[8807])
        ),
        9806 | 9833 => {
            let (lat, lon, x, y) = natural(&p)?;
            let hyperbolic = if code == 9833 { " +hyperbolic" } else { "" };
            format!("+proj=cass{hyperbolic} +lat_0={lat} +lon_0={lon} +x_0={x} +y_0={y}")
        }
        1028 | 1029 => format!(
            "+proj=eqc +lat_ts={} +lat_0={} +lon_0={} +x_0={} +y_0={}",
            p.or_zero(&[8823]),
            p.or_zero(&[8801]),
            p.or_zero(&[8802]),
            p.or_zero(&[8806]),
            p.or_zero(&[8807])
        ),
        9835 | 9834 => format!(
            "+proj=cea +lat_ts={} +lon_0={} +x_0={} +y_0={}",
            p.or_zero(&[8823]),
            p.or_zero(&[8802]),
            p.or_zero(&[8806]),
            p.or_zero(&[8807])
        ),
        9819 | 1041 | 1042 | 1043 => {
            let name = if matches!(code, 1042 | 1043) {
                "mod_krovak"
            } else {
                "krovak"
            };
            format!(
                "+proj={name} +lat_0={} +lon_0={} +alpha={} +k={} +x_0={} +y_0={}",
                p.or_zero(&[8811]),
                p.or_zero(&[8833, 8802]),
                p.need(&[1036], "co-latitude of the cone axis")?,
                p.scale(&[8819]),
                p.or_zero(&[8806]),
                p.or_zero(&[8807])
            )
        }
        9813 => format!(
            "+proj=labrd +lat_0={} +lon_0={} +azi={} +k={} +x_0={} +y_0={}",
            p.or_zero(&[8811]),
            p.or_zero(&[8812]),
            p.need(&[8813], "azimuth at the projection centre")?,
            p.scale(&[8815]),
            p.or_zero(&[8806]),
            p.or_zero(&[8807])
        ),
        1052 => {
            let (lat, lon, x, y) = natural(&p)?;
            format!(
                "+proj=col_urban +lat_0={lat} +lon_0={lon} +x_0={x} +y_0={y} +h_0={}",
                p.or_zero(&[1039])
            )
        }
        9818 | 9811 | 1078 => {
            let (lat, lon, x, y) = natural(&p)?;
            match code {
                9818 => format!("+proj=poly +lat_0={lat} +lon_0={lon} +x_0={x} +y_0={y}"),
                9811 => format!("+proj=nzmg +lat_0={lat} +lon_0={lon} +x_0={x} +y_0={y}"),
                _ => format!("+proj=eqearth +lon_0={lon} +x_0={x} +y_0={y}"),
            }
        }
        _ => return Err(format!("unsupported projection method {method_name:?}")),
    };
    Ok(format!("{s}{}", axis_token(axes)))
}

fn projected(crs: &Value) -> Result<Definition, String> {
    let base = crs
        .get("base_crs")
        .ok_or("a projected CRS without base_crs")?;
    let geographic = convert(base)?;
    let datum = datum_tokens_of(base)?;
    let axes = axes_of(crs.get("coordinate_system"))?.unwrap_or(Axes {
        unit: 1.0,
        directions: Vec::new(),
    });
    let conversion = crs
        .get("conversion")
        .ok_or("a projected CRS without conversion")?;
    let proj = projection(conversion, &axes)?;
    // Pseudo-Mercator runs the spherical formulas on a sphere of the ellipsoid's semi-major axis.
    let datum = if proj.contains("+wktext") {
        sphere_of_semi_major(&datum)
    } else {
        datum
    };
    let mut out = format!("{proj} {datum}{}", to_meter_token(axes.unit));
    if let Some(towgs84) = geographic
        .proj
        .split_whitespace()
        .find(|t| t.starts_with("+towgs84="))
    {
        out.push(' ');
        out.push_str(towgs84);
    }
    out.push_str(" +no_defs");
    Ok(Definition {
        proj: out,
        base: geographic.base,
        angular: DEGREE,
    })
}

/// The datum tokens with the ellipsoid replaced by the sphere of its semi-major axis.
fn sphere_of_semi_major(datum: &str) -> String {
    let mut out = Vec::new();
    for token in datum.split_whitespace() {
        match token.split_once('=') {
            Some(("+a" | "+R", a)) => {
                out.push(format!("+a={a}"));
                out.push(format!("+b={a}"));
            }
            Some(("+rf" | "+b", _)) => {}
            _ => out.push(token.to_string()),
        }
    }
    out.join(" ")
}

/// Ellipsoid and prime meridian of a base CRS (a bound CRS holds them in its source).
fn datum_tokens_of(base: &Value) -> Result<String, String> {
    match type_of(base)? {
        "BoundCRS" => datum_tokens_of(
            base.get("source_crs")
                .ok_or("a bound CRS without source_crs")?,
        ),
        _ => datum_tokens(base),
    }
}

// -- bound CRS ----------------------------------------------------------------------------------

/// A CRS with an abridged transformation to WGS 84: the source CRS plus `+towgs84`.
fn bound(crs: &Value) -> Result<Definition, String> {
    let source = crs
        .get("source_crs")
        .ok_or("a bound CRS without source_crs")?;
    let mut definition = convert(source)?;
    if let Some(set) = towgs84(crs.get("transformation"))? {
        definition.proj = format!(
            "{} +towgs84={}",
            definition.proj.replace(" +no_defs", ""),
            set
        );
        definition.proj.push_str(" +no_defs");
    }
    Ok(definition)
}

/// The `+towgs84` set of a Helmert transformation to WGS 84, or `None` for any other method.
fn towgs84(transformation: Option<&Value>) -> Result<Option<String>, String> {
    let Some(t) = transformation else {
        return Ok(None);
    };
    let name = t
        .get("method")
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let coordinate_frame = name.contains("coordinate frame");
    if !(coordinate_frame
        || name.contains("position vector")
        || name.contains("geocentric translations"))
    {
        return Ok(None);
    }
    let mut v = [0.0_f64; 7];
    for p in t
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let n = p
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let slot = match n.as_str() {
            "x-axis translation" => 0,
            "y-axis translation" => 1,
            "z-axis translation" => 2,
            "x-axis rotation" => 3,
            "y-axis rotation" => 4,
            "z-axis rotation" => 5,
            "scale difference" => 6,
            _ => continue,
        };
        let value = p
            .get("value")
            .and_then(Value::as_f64)
            .ok_or("a transformation parameter without value")?;
        let factor = unit_factor(p.get("unit"))?;
        v[slot] = match slot {
            // rotations in arc-seconds, the scale in ppm
            3..=5 => value * factor / (DEGREE / 3600.0),
            6 => value * factor / 1e-6,
            _ => value * factor,
        };
    }
    if coordinate_frame {
        for r in &mut v[3..6] {
            *r = -*r;
        }
    }
    let list: Vec<String> = v.iter().map(f64::to_string).collect();
    Ok(Some(list.join(",")))
}

// -- WKT ----------------------------------------------------------------------------------------

/// WKT1 or WKT2 text as PROJJSON. A WKT1 `TOWGS84` becomes a bound CRS, like PROJ reads it.
pub(crate) fn wkt_to_projjson(text: &str) -> Result<Value, String> {
    use crate::wkt1_projjson::{parse, wkt1_to_projjson};
    let node = parse(text.trim()).ok_or("invalid WKT text")?;
    let converted = match node.keyword.as_str() {
        "GEOGCS" | "PROJCS" | "GEOCCS" | "COMPD_CS" => wkt1_to_projjson(text).map(|v| {
            let geog = if node.keyword == "GEOGCS" {
                Some(&node)
            } else {
                node.child("GEOGCS")
            };
            match geog
                .and_then(|g| g.child("DATUM"))
                .and_then(|d| d.child("TOWGS84"))
            {
                Some(t) if t.args.len() == 7 => wrap_towgs84(v, t),
                _ => v,
            }
        }),
        _ => crate::wkt2_projjson::from_node(&node, true),
    };
    converted.ok_or_else(|| {
        format!(
            "unsupported WKT definition: {} with a projection, parameter or CRS type that is not recognised",
            node.keyword
        )
    })
}

fn wrap_towgs84(source: Value, towgs84: &crate::wkt1_projjson::Node) -> Value {
    let names = [
        ("X-axis translation", "metre"),
        ("Y-axis translation", "metre"),
        ("Z-axis translation", "metre"),
        ("X-axis rotation", "arc-second"),
        ("Y-axis rotation", "arc-second"),
        ("Z-axis rotation", "arc-second"),
        ("Scale difference", "parts per million"),
    ];
    let parameters: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(i, (name, unit))| {
            serde_json::json!({
                "name": name,
                "value": towgs84.number(i).unwrap_or(0.0),
                "unit": unit,
            })
        })
        .collect();
    serde_json::json!({
        "type": "BoundCRS",
        "source_crs": source,
        "transformation": {
            "name": "TOWGS84",
            "method": { "name": "Position Vector transformation (geog2D domain)" },
            "parameters": parameters,
        },
    })
}
