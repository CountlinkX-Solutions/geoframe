//! The WKT2 entries of the bundled `crs-definitions` table, translated to PROJJSON: 3D
//! geographic CRS (`GEOGCRS`) and the datum-shifted `BOUNDCRS` entries, which wrap a geographic
//! source CRS with an abridged Helmert transformation to WGS 84.

use serde_json::{json, Map, Value};

use crate::wkt1_projjson::{reference_frame, unit_value, Arg, Node, SCHEMA};

/// `geodetic latitude (Lat)` as the PROJJSON name `Geodetic latitude` and abbreviation `Lat`.
fn name_and_abbreviation(label: &str) -> (String, String) {
    let (name, abbreviation) = match label.rsplit_once(" (") {
        Some((name, rest)) if rest.ends_with(')') => {
            (name, Some(rest.trim_end_matches(')').to_string()))
        }
        _ => (label, None),
    };
    let mut chars = name.chars();
    let capitalised = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    let abbreviation = abbreviation.unwrap_or_else(|| match name {
        "latitude" => "lat".into(),
        "longitude" => "lon".into(),
        other => other.chars().take(1).collect(),
    });
    (capitalised, abbreviation)
}

/// The axes of a WKT2 CRS in `ORDER`, each with the unit it states.
fn axes(node: &Node) -> Option<Vec<Value>> {
    let mut axes = Vec::new();
    for (position, axis) in node.children("AXIS").enumerate() {
        let (name, abbreviation) = name_and_abbreviation(axis.text(0)?);
        let unit = match (axis.child("ANGLEUNIT"), axis.child("LENGTHUNIT")) {
            (Some(unit), _) => unit_value(unit, true)?,
            (None, Some(unit)) => unit_value(unit, false)?,
            (None, None) => return None,
        };
        let order = axis
            .child("ORDER")
            .and_then(|o| o.number(0))
            .unwrap_or(position as f64 + 1.0);
        axes.push((
            order as u64,
            json!({
                "name": name,
                "abbreviation": abbreviation,
                "direction": axis.text(1)?.to_ascii_lowercase(),
                "unit": unit,
            }),
        ));
    }
    axes.sort_by_key(|(order, _)| *order);
    Some(axes.into_iter().map(|(_, axis)| axis).collect())
}

/// A WKT2 `GEOGCRS`. `with_schema` is set when it is the top-level object.
pub(crate) fn geographic(node: &Node, with_schema: bool) -> Option<Value> {
    let datum = node.child("DATUM")?;
    let frame = reference_frame(datum, datum.child("ELLIPSOID")?, node.child("PRIMEM"))?;
    let mut out = Map::new();
    if with_schema {
        out.insert("$schema".into(), Value::from(SCHEMA));
    }
    out.insert("type".into(), Value::from("GeographicCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("datum".into(), frame);
    out.insert(
        "coordinate_system".into(),
        json!({ "subtype": "ellipsoidal", "axis": axes(node)? }),
    );
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
    Some(Value::Object(out))
}

/// A WKT2 `BOUNDCRS`: the table wraps a geographic CRS with its abridged Helmert transformation
/// to WGS 84. The EPSG definition of the code is the source CRS alone (the shift is a separate
/// coordinate operation in the EPSG database, and pyproj names the code without it), so that is
/// what is written, like the `TOWGS84` of the WKT1 entries is left out.
pub(crate) fn bound(node: &Node) -> Option<Value> {
    let source = node.child("SOURCECRS")?.args.iter().find_map(|a| match a {
        Arg::Node(n) => Some(n),
        _ => None,
    })?;
    match source.keyword.as_str() {
        "GEOGCRS" => geographic(source, true),
        _ => None,
    }
}
