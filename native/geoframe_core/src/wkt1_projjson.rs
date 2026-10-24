//! OGC WKT to PROJJSON, for the EPSG definitions in the bundled `crs-definitions` table.
//!
//! The table holds WKT1 (`GEOGCS`, `PROJCS`, `GEOCCS`, `COMPD_CS`) and, for 3D geographic and
//! datum-shifted entries, WKT2 (`GEOGCRS`, `BOUNDCRS`, translated in `wkt2_projjson.rs`).
//!
//! pyproj (and so GeoPandas) cannot parse a PROJJSON object that lacks a top-level `type`, a
//! projected CRS also needs its base CRS, conversion and coordinate system. There is no PROJ
//! database in the pure-Rust build, so the table's WKT1 is translated: the datum and ellipsoid
//! carry over, the WKT1 projection and its parameters are renamed to the EPSG method and
//! parameter names (with their EPSG codes, which is how PROJ matches them), and the axes and
//! units come from the WKT. A projection or parameter that has no entry in the tables below
//! makes the conversion return `None` (the caller then keeps an identifier-only stub).

use serde_json::{json, Map, Value};

pub(crate) const SCHEMA: &str = "https://proj.org/schemas/v0.7/projjson.schema.json";

// -- a tiny WKT parser ----------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum Arg {
    Text(String),
    Number(f64),
    Word(String),
    Node(Node),
}

#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) keyword: String,
    pub(crate) args: Vec<Arg>,
}

impl Node {
    pub(crate) fn child<'a>(&'a self, keyword: &'a str) -> Option<&'a Node> {
        self.children(keyword).next()
    }

    pub(crate) fn children<'a>(&'a self, keyword: &'a str) -> impl Iterator<Item = &'a Node> {
        self.args.iter().filter_map(move |a| match a {
            Arg::Node(n) if n.keyword == keyword => Some(n),
            _ => None,
        })
    }

    pub(crate) fn text(&self, index: usize) -> Option<&str> {
        match self.args.get(index)? {
            Arg::Text(s) | Arg::Word(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn number(&self, index: usize) -> Option<f64> {
        match self.args.get(index)? {
            Arg::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The `AUTHORITY["EPSG", "code"]` (WKT1) or `ID["EPSG", code]` (WKT2) of this node, as a
    /// PROJJSON `id`.
    pub(crate) fn id(&self) -> Option<Value> {
        let authority = self.child("AUTHORITY").or_else(|| self.child("ID"))?;
        let code = authority
            .text(1)
            .and_then(|c| c.parse::<u64>().ok())
            .or_else(|| authority.number(1).map(|c| c as u64))?;
        Some(json!({ "authority": authority.text(0)?, "code": code }))
    }
}

pub(crate) fn parse(text: &str) -> Option<Node> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let node = parse_node(&chars, &mut at)?;
    (at == chars.len()).then_some(node)
}

fn parse_node(c: &[char], at: &mut usize) -> Option<Node> {
    let start = *at;
    while c
        .get(*at)
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
    {
        *at += 1;
    }
    let keyword: String = c[start..*at].iter().collect();
    if keyword.is_empty() || c.get(*at) != Some(&'[') {
        return None;
    }
    *at += 1;
    let mut args = Vec::new();
    loop {
        match c.get(*at)? {
            ']' => {
                *at += 1;
                return Some(Node { keyword, args });
            }
            ',' => *at += 1,
            '"' => {
                *at += 1;
                let begin = *at;
                while *c.get(*at)? != '"' {
                    *at += 1;
                }
                args.push(Arg::Text(c[begin..*at].iter().collect()));
                *at += 1;
            }
            ch if ch.is_ascii_digit() || *ch == '-' || *ch == '+' || *ch == '.' => {
                let begin = *at;
                while c
                    .get(*at)
                    .is_some_and(|d| d.is_ascii_digit() || "+-.eE".contains(*d))
                {
                    *at += 1;
                }
                let number: String = c[begin..*at].iter().collect();
                args.push(Arg::Number(number.parse().ok()?));
            }
            _ => {
                let begin = *at;
                while c
                    .get(*at)
                    .is_some_and(|d| d.is_ascii_alphanumeric() || *d == '_')
                {
                    *at += 1;
                }
                if c.get(*at) == Some(&'[') {
                    *at = begin;
                    args.push(Arg::Node(parse_node(c, at)?));
                } else if *at == begin {
                    return None;
                } else {
                    args.push(Arg::Word(c[begin..*at].iter().collect()));
                }
            }
        }
    }
}

// -- units and numbers ---------------------------------------------------------------------

pub(crate) fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// A PROJJSON unit: the bare name for `degree`, `metre` and `unity`, an object otherwise.
pub(crate) fn unit_value(unit: &Node, angular: bool) -> Option<Value> {
    let name = unit.text(0)?;
    let factor = unit.number(1)?;
    let plain = if angular {
        name.eq_ignore_ascii_case("degree") && (factor - 0.017_453_292_519_943_3).abs() < 1e-12
    } else {
        (name.eq_ignore_ascii_case("metre") || name.eq_ignore_ascii_case("meter")) && factor == 1.0
    };
    if plain {
        return Some(Value::from(if angular { "degree" } else { "metre" }));
    }
    let mut out = Map::new();
    out.insert(
        "type".into(),
        Value::from(if angular { "AngularUnit" } else { "LinearUnit" }),
    );
    out.insert("name".into(), Value::from(name));
    out.insert("conversion_factor".into(), number(factor));
    if let Some(id) = unit.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

fn underscores_to_spaces(name: &str) -> String {
    name.replace('_', " ")
}

// -- coordinate systems --------------------------------------------------------------------

fn abbreviation(name: &str) -> String {
    match name {
        "Easting" => "E".into(),
        "Northing" => "N".into(),
        "Westing" => "W".into(),
        "Southing" => "S".into(),
        "Latitude" | "Geodetic latitude" => "Lat".into(),
        "Longitude" | "Geodetic longitude" => "Lon".into(),
        other => other.chars().take(1).collect::<String>().to_uppercase(),
    }
}

fn axes(node: &Node, unit: &Value, default: [(&str, &str); 2]) -> Vec<Value> {
    let from_wkt: Vec<Value> = node
        .children("AXIS")
        .filter_map(|axis| {
            let name = axis.text(0)?;
            let direction = axis.text(1)?.to_ascii_lowercase();
            Some(json!({
                "name": name,
                "abbreviation": abbreviation(name),
                "direction": direction,
                "unit": unit,
            }))
        })
        .collect();
    if from_wkt.len() == 2 {
        return from_wkt;
    }
    default
        .iter()
        .map(|(name, direction)| {
            json!({
                "name": name,
                "abbreviation": abbreviation(name),
                "direction": direction,
                "unit": unit,
            })
        })
        .collect()
}

// -- geographic CRS ------------------------------------------------------------------------

/// The `GeodeticReferenceFrame` of a `DATUM`, its `SPHEROID`/`ELLIPSOID` and the optional
/// `PRIMEM` of the CRS that holds them.
pub(crate) fn reference_frame(
    datum: &Node,
    spheroid: &Node,
    meridian: Option<&Node>,
) -> Option<Value> {
    let mut ellipsoid = Map::new();
    ellipsoid.insert("name".into(), Value::from(spheroid.text(0)?));
    let inverse_flattening = spheroid.number(2)?;
    if inverse_flattening == 0.0 {
        ellipsoid.insert("radius".into(), number(spheroid.number(1)?));
    } else {
        ellipsoid.insert("semi_major_axis".into(), number(spheroid.number(1)?));
        ellipsoid.insert("inverse_flattening".into(), number(inverse_flattening));
    }
    if let Some(id) = spheroid.id() {
        ellipsoid.insert("id".into(), id);
    }
    let mut frame = Map::new();
    frame.insert("type".into(), Value::from("GeodeticReferenceFrame"));
    frame.insert(
        "name".into(),
        Value::from(underscores_to_spaces(datum.text(0)?)),
    );
    frame.insert("ellipsoid".into(), Value::Object(ellipsoid));
    if let Some(meridian) = meridian {
        // WKT1 gives the prime meridian in degrees whatever the unit of the CRS (Paris is
        // 2.33722917, not 2.5969213 grads); WKT2 states the unit on the node.
        let longitude = meridian.number(1)?;
        let unit = match meridian.child("ANGLEUNIT") {
            Some(unit) => unit_value(unit, true)?,
            None => Value::from("degree"),
        };
        if longitude != 0.0 {
            frame.insert(
                "prime_meridian".into(),
                json!({
                    "name": meridian.text(0)?,
                    "longitude": { "value": number(longitude), "unit": unit },
                }),
            );
        }
    }
    if let Some(id) = datum.id() {
        frame.insert("id".into(), id);
    }
    Some(Value::Object(frame))
}

fn geographic(node: &Node, with_schema: bool) -> Option<Value> {
    let datum = node.child("DATUM")?;
    let frame = reference_frame(datum, datum.child("SPHEROID")?, node.child("PRIMEM"))?;
    let angular = unit_value(node.child("UNIT")?, true)?;
    let mut out = Map::new();
    if with_schema {
        out.insert("$schema".into(), Value::from(SCHEMA));
    }
    out.insert("type".into(), Value::from("GeographicCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("datum".into(), frame);
    out.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "ellipsoidal",
            "axis": axes(node, &angular, [("Geodetic latitude", "north"), ("Geodetic longitude", "east")]),
        }),
    );
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

/// A geocentric CRS (`GEOCCS`): Cartesian X, Y, Z axes on the datum's ellipsoid.
fn geocentric(node: &Node) -> Option<Value> {
    let datum = node.child("DATUM")?;
    let frame = reference_frame(datum, datum.child("SPHEROID")?, node.child("PRIMEM"))?;
    let linear = unit_value(node.child("UNIT")?, false)?;
    let axis = |name: &str, abbreviation: &str, direction: &str| {
        json!({
            "name": name,
            "abbreviation": abbreviation,
            "direction": direction,
            "unit": linear,
        })
    };
    let mut out = Map::new();
    out.insert("$schema".into(), Value::from(SCHEMA));
    out.insert("type".into(), Value::from("GeodeticCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("datum".into(), frame);
    out.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "Cartesian",
            "axis": [
                axis("Geocentric X", "X", "geocentricX"),
                axis("Geocentric Y", "Y", "geocentricY"),
                axis("Geocentric Z", "Z", "geocentricZ"),
            ],
        }),
    );
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

/// A vertical CRS (`VERT_CS`). The `EXTENSION` grids some entries carry are PROJ.4 hints, not
/// part of the EPSG definition, and are left out.
fn vertical(node: &Node) -> Option<Value> {
    let datum = node.child("VERT_DATUM")?;
    let linear = unit_value(node.child("UNIT")?, false)?;
    let down = node
        .child("AXIS")
        .and_then(|a| a.text(1))
        .is_some_and(|d| d.eq_ignore_ascii_case("down"));
    let (name, abbreviation, direction) = if down {
        ("Depth", "D", "down")
    } else {
        ("Gravity-related height", "H", "up")
    };
    let mut frame = Map::new();
    frame.insert("type".into(), Value::from("VerticalReferenceFrame"));
    frame.insert("name".into(), Value::from(datum.text(0)?));
    if let Some(id) = datum.id() {
        frame.insert("id".into(), id);
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::from("VerticalCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("datum".into(), Value::Object(frame));
    out.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "vertical",
            "axis": [{
                "name": name,
                "abbreviation": abbreviation,
                "direction": direction,
                "unit": linear,
            }],
        }),
    );
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

/// A compound CRS (`COMPD_CS`): a geographic or projected CRS plus a vertical one. Datum
/// shifts (`TOWGS84`) are dropped from the components, as they are everywhere else here.
fn compound(node: &Node) -> Option<Value> {
    let mut components = Vec::new();
    for arg in &node.args {
        let Arg::Node(part) = arg else { continue };
        components.push(match part.keyword.as_str() {
            "GEOGCS" => geographic(part, false)?,
            "PROJCS" => projected(part, false)?,
            "VERT_CS" => vertical(part)?,
            "AUTHORITY" => continue,
            _ => return None,
        });
    }
    if components.len() != 2 {
        return None;
    }
    let mut out = Map::new();
    out.insert("$schema".into(), Value::from(SCHEMA));
    out.insert("type".into(), Value::from("CompoundCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("components".into(), Value::Array(components));
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

// -- projections ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Kind {
    Angle,
    Linear,
    Scale,
}

/// One conversion parameter: the WKT1 names it may have, its EPSG name, code and kind.
type Param = (&'static [&'static str], &'static str, u32, Kind);

const LAT_ORIGIN: Param = (
    &["latitude_of_origin", "latitude_of_center"],
    "Latitude of natural origin",
    8801,
    Kind::Angle,
);
const LON_ORIGIN: Param = (
    &["central_meridian", "longitude_of_center"],
    "Longitude of natural origin",
    8802,
    Kind::Angle,
);
const SCALE: Param = (
    &["scale_factor"],
    "Scale factor at natural origin",
    8805,
    Kind::Scale,
);
const FALSE_EASTING: Param = (&["false_easting"], "False easting", 8806, Kind::Linear);
const FALSE_NORTHING: Param = (&["false_northing"], "False northing", 8807, Kind::Linear);

const NATURAL_ORIGIN: &[Param] = &[LAT_ORIGIN, LON_ORIGIN, SCALE, FALSE_EASTING, FALSE_NORTHING];
const NATURAL_ORIGIN_NO_SCALE: &[Param] = &[LAT_ORIGIN, LON_ORIGIN, FALSE_EASTING, FALSE_NORTHING];

const FALSE_ORIGIN: &[Param] = &[
    (
        &["latitude_of_origin", "latitude_of_center"],
        "Latitude of false origin",
        8821,
        Kind::Angle,
    ),
    (
        &["central_meridian", "longitude_of_center"],
        "Longitude of false origin",
        8822,
        Kind::Angle,
    ),
    (
        &["standard_parallel_1"],
        "Latitude of 1st standard parallel",
        8823,
        Kind::Angle,
    ),
    (
        &["standard_parallel_2"],
        "Latitude of 2nd standard parallel",
        8824,
        Kind::Angle,
    ),
    (
        &["false_easting"],
        "Easting at false origin",
        8826,
        Kind::Linear,
    ),
    (
        &["false_northing"],
        "Northing at false origin",
        8827,
        Kind::Linear,
    ),
];

const POLAR_B: &[Param] = &[
    (
        &["latitude_of_origin"],
        "Latitude of standard parallel",
        8832,
        Kind::Angle,
    ),
    (
        &["central_meridian"],
        "Longitude of origin",
        8833,
        Kind::Angle,
    ),
    FALSE_EASTING,
    FALSE_NORTHING,
];

const MERCATOR_B: &[Param] = &[
    (
        &["standard_parallel_1"],
        "Latitude of 1st standard parallel",
        8823,
        Kind::Angle,
    ),
    LON_ORIGIN,
    FALSE_EASTING,
    FALSE_NORTHING,
];

const EQUIDISTANT_CYLINDRICAL: &[Param] = &[
    (
        &["latitude_of_origin"],
        "Latitude of 1st standard parallel",
        8823,
        Kind::Angle,
    ),
    LON_ORIGIN,
    FALSE_EASTING,
    FALSE_NORTHING,
];

const CYLINDRICAL_EQUAL_AREA: &[Param] = &[
    (
        &["standard_parallel_1"],
        "Latitude of 1st standard parallel",
        8823,
        Kind::Angle,
    ),
    LON_ORIGIN,
    FALSE_EASTING,
    FALSE_NORTHING,
];

const HOTINE_CENTRE: [Param; 5] = [
    (
        &["latitude_of_center"],
        "Latitude of projection centre",
        8811,
        Kind::Angle,
    ),
    (
        &["longitude_of_center"],
        "Longitude of projection centre",
        8812,
        Kind::Angle,
    ),
    (&["azimuth"], "Azimuth of initial line", 8813, Kind::Angle),
    (
        &["rectified_grid_angle"],
        "Angle from Rectified to Skew Grid",
        8814,
        Kind::Angle,
    ),
    (
        &["scale_factor"],
        "Scale factor on initial line",
        8815,
        Kind::Scale,
    ),
];
const HOTINE_A: &[Param] = &[
    HOTINE_CENTRE[0],
    HOTINE_CENTRE[1],
    HOTINE_CENTRE[2],
    HOTINE_CENTRE[3],
    HOTINE_CENTRE[4],
    FALSE_EASTING,
    FALSE_NORTHING,
];
const HOTINE_B: &[Param] = &[
    HOTINE_CENTRE[0],
    HOTINE_CENTRE[1],
    HOTINE_CENTRE[2],
    HOTINE_CENTRE[3],
    HOTINE_CENTRE[4],
    (
        &["false_easting"],
        "Easting at projection centre",
        8816,
        Kind::Linear,
    ),
    (
        &["false_northing"],
        "Northing at projection centre",
        8817,
        Kind::Linear,
    ),
];

const LABORDE: &[Param] = &[
    (
        &["latitude_of_center"],
        "Latitude of projection centre",
        8811,
        Kind::Angle,
    ),
    (
        &["longitude_of_center"],
        "Longitude of projection centre",
        8812,
        Kind::Angle,
    ),
    (
        &["azimuth"],
        "Azimuth at projection centre",
        8813,
        Kind::Angle,
    ),
    (
        &["scale_factor"],
        "Scale factor at projection centre",
        8815,
        Kind::Scale,
    ),
    FALSE_EASTING,
    FALSE_NORTHING,
];

const TUNISIA_MINING_GRID: &[Param] = &[
    (
        &["latitude_of_origin"],
        "Latitude of false origin",
        8821,
        Kind::Angle,
    ),
    (
        &["central_meridian"],
        "Longitude of false origin",
        8822,
        Kind::Angle,
    ),
    (
        &["false_easting"],
        "Easting at false origin",
        8826,
        Kind::Linear,
    ),
    (
        &["false_northing"],
        "Northing at false origin",
        8827,
        Kind::Linear,
    ),
];

const KROVAK: &[Param] = &[
    (
        &["latitude_of_center"],
        "Latitude of projection centre",
        8811,
        Kind::Angle,
    ),
    (
        &["longitude_of_center"],
        "Longitude of origin",
        8833,
        Kind::Angle,
    ),
    (&["azimuth"], "Co-latitude of cone axis", 1036, Kind::Angle),
    (
        &["pseudo_standard_parallel_1"],
        "Latitude of pseudo standard parallel",
        8818,
        Kind::Angle,
    ),
    (
        &["scale_factor"],
        "Scale factor on pseudo standard parallel",
        8819,
        Kind::Scale,
    ),
    FALSE_EASTING,
    FALSE_NORTHING,
];

/// The EPSG method (name, code) and parameter table of a WKT1 projection.
fn method(
    projection: &str,
    has_scale: bool,
    east_first: bool,
) -> Option<(&'static str, u32, &'static [Param])> {
    Some(match projection {
        "Transverse_Mercator" => ("Transverse Mercator", 9807, NATURAL_ORIGIN),
        "Transverse_Mercator_South_Orientated" => (
            "Transverse Mercator (South Orientated)",
            9808,
            NATURAL_ORIGIN,
        ),
        "Lambert_Conformal_Conic_2SP" => ("Lambert Conic Conformal (2SP)", 9802, FALSE_ORIGIN),
        "Lambert_Conformal_Conic_1SP" => ("Lambert Conic Conformal (1SP)", 9801, NATURAL_ORIGIN),
        "Albers_Conic_Equal_Area" => ("Albers Equal Area", 9822, FALSE_ORIGIN),
        "Polar_Stereographic" if has_scale => {
            ("Polar Stereographic (variant A)", 9810, NATURAL_ORIGIN)
        }
        "Polar_Stereographic" => ("Polar Stereographic (variant B)", 9829, POLAR_B),
        "Cassini_Soldner" => ("Cassini-Soldner", 9806, NATURAL_ORIGIN_NO_SCALE),
        "Hotine_Oblique_Mercator" => ("Hotine Oblique Mercator (variant A)", 9812, HOTINE_A),
        "Hotine_Oblique_Mercator_Azimuth_Center" => {
            ("Hotine Oblique Mercator (variant B)", 9815, HOTINE_B)
        }
        "Oblique_Stereographic" => ("Oblique Stereographic", 9809, NATURAL_ORIGIN),
        "Lambert_Conformal_Conic_2SP_Belgium" => {
            ("Lambert Conic Conformal (2SP Belgium)", 9803, FALSE_ORIGIN)
        }
        "Laborde_Oblique_Mercator" => ("Laborde Oblique Mercator", 9813, LABORDE),
        "Tunisia_Mining_Grid" => ("Tunisia Mining Grid", 9816, TUNISIA_MINING_GRID),
        "Lambert_Azimuthal_Equal_Area" => (
            "Lambert Azimuthal Equal Area",
            9820,
            NATURAL_ORIGIN_NO_SCALE,
        ),
        "Mercator_1SP" => ("Mercator (variant A)", 9804, NATURAL_ORIGIN),
        "Mercator_2SP" => ("Mercator (variant B)", 9805, MERCATOR_B),
        "Krovak" if east_first => ("Krovak (North Orientated)", 1041, KROVAK),
        "Krovak" => ("Krovak", 9819, KROVAK),
        "Polyconic" => ("American Polyconic", 9818, NATURAL_ORIGIN_NO_SCALE),
        "Equirectangular" => ("Equidistant Cylindrical", 1028, EQUIDISTANT_CYLINDRICAL),
        "Cylindrical_Equal_Area" => (
            "Lambert Cylindrical Equal Area",
            9835,
            CYLINDRICAL_EQUAL_AREA,
        ),
        "New_Zealand_Map_Grid" => ("New Zealand Map Grid", 9811, NATURAL_ORIGIN_NO_SCALE),
        "Azimuthal_Equidistant" => (
            "Modified Azimuthal Equidistant",
            9832,
            NATURAL_ORIGIN_NO_SCALE,
        ),
        _ => return None,
    })
}

fn conversion(node: &Node, base: &Node, linear: &Value) -> Option<Value> {
    let projection = node.child("PROJECTION")?.text(0)?;
    let parameters: Vec<&Node> = node.children("PARAMETER").collect();
    // Polar Stereographic variant A is the one on a pole (UPS); variant B puts the origin of
    // scale on a standard parallel and has no scale factor (WKT1 may still list `1`).
    let on_a_pole = parameters
        .iter()
        .find(|p| p.text(0) == Some("latitude_of_origin"))
        .and_then(|p| p.number(1))
        .is_some_and(|lat| lat.abs() == 90.0);
    let has_scale = on_a_pole && parameters.iter().any(|p| p.text(0) == Some("scale_factor"));
    let east_first = node
        .child("AXIS")
        .and_then(|a| a.text(1))
        .is_some_and(|d| d.eq_ignore_ascii_case("east"));
    let (method_name, method_code, table) = method(projection, has_scale, east_first)?;
    let angular = unit_value(base.child("UNIT")?, true)?;

    let mut out = Vec::new();
    for parameter in &parameters {
        let name = parameter.text(0)?;
        let value = parameter.number(1)?;
        let entry = table.iter().find(|(names, ..)| names.contains(&name));
        let Some((_, epsg_name, code, kind)) = entry else {
            if name == "scale_factor" && value == 1.0 {
                continue;
            }
            // The Laborde grid angle is not an EPSG parameter of that method (PROJ drops it).
            if projection == "Laborde_Oblique_Mercator" && name == "rectified_grid_angle" {
                continue;
            }
            return None;
        };
        let unit = match kind {
            Kind::Angle => angular.clone(),
            Kind::Linear => linear.clone(),
            Kind::Scale => Value::from("unity"),
        };
        out.push(json!({
            "name": epsg_name,
            "value": number(value),
            "unit": unit,
            "id": { "authority": "EPSG", "code": code },
        }));
    }
    // GDAL leaves a parameter out of the WKT1 when it is zero (or a scale of one): PROJ would
    // default it, but a complete definition lists it.
    for (names, epsg_name, code, kind) in table {
        let given = parameters
            .iter()
            .any(|p| p.text(0).is_some_and(|n| names.contains(&n)));
        if given {
            continue;
        }
        let (value, unit) = match kind {
            Kind::Angle => (0, angular.clone()),
            Kind::Linear => (0, linear.clone()),
            Kind::Scale => (1, Value::from("unity")),
        };
        out.push(json!({
            "name": epsg_name,
            "value": value,
            "unit": unit,
            "id": { "authority": "EPSG", "code": code },
        }));
    }
    if out.is_empty() {
        return None;
    }
    Some(json!({
        "name": underscores_to_spaces(projection),
        "method": { "name": method_name, "id": { "authority": "EPSG", "code": method_code } },
        "parameters": out,
    }))
}

fn projected(node: &Node, with_schema: bool) -> Option<Value> {
    let base_node = node.child("GEOGCS")?;
    let base = geographic(base_node, false)?;
    let linear = unit_value(node.child("UNIT")?, false)?;
    let mut out = Map::new();
    if with_schema {
        out.insert("$schema".into(), Value::from(SCHEMA));
    }
    out.insert("type".into(), Value::from("ProjectedCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("base_crs".into(), base);
    out.insert("conversion".into(), conversion(node, base_node, &linear)?);
    out.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "Cartesian",
            "axis": axes(node, &linear, [("Easting", "east"), ("Northing", "north")]),
        }),
    );
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

/// Translates an OGC WKT definition from the table (`GEOGCS`, `PROJCS`, `GEOCCS`, `COMPD_CS`,
/// `GEOGCRS`, `BOUNDCRS`) into a complete PROJJSON object, or `None` when it uses a projection
/// or parameter outside the supported set.
pub(crate) fn wkt1_to_projjson(wkt: &str) -> Option<Value> {
    let node = parse(wkt.trim())?;
    match node.keyword.as_str() {
        "GEOGCS" => geographic(&node, true),
        "PROJCS" => projected(&node, true),
        "GEOCCS" => geocentric(&node),
        "COMPD_CS" => compound(&node),
        "GEOGCRS" => crate::wkt2_projjson::geographic(&node, true),
        "BOUNDCRS" => crate::wkt2_projjson::bound(&node),
        _ => None,
    }
}
