//! The WKT2 entries of the bundled `crs-definitions` table, translated to PROJJSON: 3D
//! geographic CRS (`GEOGCRS`) and the datum-shifted `BOUNDCRS` entries, which wrap a geographic
//! source CRS with an abridged Helmert transformation to WGS 84.

use serde_json::{json, Map, Value};

use crate::wkt1_projjson::{number, reference_frame, unit_value, Arg, Node, SCHEMA};

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
    // A datum ensemble (WGS 84, ETRS89, ...) stands in for the datum: only its ellipsoid matters.
    let datum = node.child("DATUM").or_else(|| node.child("ENSEMBLE"))?;
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

// -- projected, geocentric, compound and bound CRS (any WKT2 text, issue #100) -------------------

/// The unit of a conversion parameter or an axis, whichever unit node it carries.
fn any_unit(node: &Node) -> Option<Value> {
    if let Some(u) = node.child("ANGLEUNIT") {
        return unit_value(u, true);
    }
    if let Some(u) = node.child("LENGTHUNIT") {
        return unit_value(u, false);
    }
    if let Some(u) = node
        .child("SCALEUNIT")
        .or_else(|| node.child("PARAMETRICUNIT"))
    {
        let (name, factor) = (u.text(0)?, u.number(1)?);
        return Some(if factor == 1.0 {
            Value::from("unity")
        } else {
            json!({ "type": "ScaleUnit", "name": name, "conversion_factor": factor })
        });
    }
    // The pre-2019 `UNIT[...]`: a length when it is not the degree or radian family.
    let u = node.child("UNIT")?;
    let angular = u.number(1).is_some_and(|f| {
        f < 1.0
            && f > 0.0
            && u.text(0).is_some_and(|n| {
                let n = n.to_ascii_lowercase();
                n.contains("degree") || n.contains("grad") || n.contains("radian")
            })
    });
    unit_value(u, angular)
}

fn id_or_none(node: &Node, out: &mut Map<String, Value>) {
    if let Some(id) = node.id() {
        out.insert("id".into(), id);
    }
}

/// The coordinate system of a WKT2 CRS node; an axis without its own unit takes the unit
/// stated once on the CRS (older writers).
fn cs_of(node: &Node, subtype: &str) -> Option<Value> {
    let fallback = any_unit(node);
    let mut list = Vec::new();
    for (position, axis) in node.children("AXIS").enumerate() {
        let (name, abbreviation) = name_and_abbreviation(axis.text(0)?);
        let unit = any_unit(axis).or_else(|| fallback.clone())?;
        let order = axis
            .child("ORDER")
            .and_then(|o| o.number(0))
            .unwrap_or(position as f64 + 1.0);
        let mut json = json!({
            "name": name,
            "abbreviation": abbreviation,
            "direction": axis.text(1)?.to_ascii_lowercase(),
            "unit": unit,
        });
        // A polar axis ("south along 90 deg East") names its meridian.
        if let Some(meridian) = axis.child("MERIDIAN").and_then(|m| m.number(0)) {
            json["meridian"] = json!({ "longitude": number(meridian) });
        }
        list.push((order as u64, json));
    }
    list.sort_by_key(|(order, _)| *order);
    Some(
        json!({ "subtype": subtype, "axis": list.into_iter().map(|(_, a)| a).collect::<Vec<_>>() }),
    )
}

/// `BASEGEOGCRS` / `BASEGEODCRS`: a geographic CRS with no coordinate system of its own.
fn base_geographic(node: &Node) -> Option<Value> {
    let datum = node.child("DATUM").or_else(|| node.child("ENSEMBLE"))?;
    let frame = reference_frame(datum, datum.child("ELLIPSOID")?, node.child("PRIMEM"))?;
    let mut out = Map::new();
    out.insert("type".into(), Value::from("GeographicCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("datum".into(), frame);
    out.insert(
        "coordinate_system".into(),
        json!({ "subtype": "ellipsoidal", "axis": [
            { "name": "Geodetic latitude", "abbreviation": "Lat", "direction": "north", "unit": "degree" },
            { "name": "Geodetic longitude", "abbreviation": "Lon", "direction": "east", "unit": "degree" }
        ]}),
    );
    id_or_none(node, &mut out);
    Some(Value::Object(out))
}

fn parameters(node: &Node) -> Option<Vec<Value>> {
    node.children("PARAMETER")
        .map(|p| {
            let mut out = Map::new();
            out.insert("name".into(), Value::from(p.text(0)?));
            out.insert("value".into(), number(p.number(1)?));
            out.insert(
                "unit".into(),
                any_unit(p).unwrap_or_else(|| Value::from("unity")),
            );
            id_or_none(p, &mut out);
            Some(Value::Object(out))
        })
        .collect()
}

fn conversion(node: &Node) -> Option<Value> {
    let method = node.child("METHOD")?;
    let mut m = Map::new();
    m.insert("name".into(), Value::from(method.text(0)?));
    id_or_none(method, &mut m);
    Some(json!({
        "name": node.text(0)?,
        "method": Value::Object(m),
        "parameters": parameters(node)?,
    }))
}

/// A WKT2 `PROJCRS`.
pub(crate) fn projected(node: &Node, with_schema: bool) -> Option<Value> {
    let base = node
        .child("BASEGEOGCRS")
        .or_else(|| node.child("BASEGEODCRS"))?;
    let mut out = Map::new();
    if with_schema {
        out.insert("$schema".into(), Value::from(SCHEMA));
    }
    out.insert("type".into(), Value::from("ProjectedCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("base_crs".into(), base_geographic(base)?);
    out.insert("conversion".into(), conversion(node.child("CONVERSION")?)?);
    out.insert("coordinate_system".into(), cs_of(node, "Cartesian")?);
    id_or_none(node, &mut out);
    Some(Value::Object(out))
}

/// A WKT2 geocentric `GEODCRS` (`CS[Cartesian,3]`).
pub(crate) fn geocentric(node: &Node, with_schema: bool) -> Option<Value> {
    let datum = node.child("DATUM").or_else(|| node.child("ENSEMBLE"))?;
    let frame = reference_frame(datum, datum.child("ELLIPSOID")?, node.child("PRIMEM"))?;
    let mut out = Map::new();
    if with_schema {
        out.insert("$schema".into(), Value::from(SCHEMA));
    }
    out.insert("type".into(), Value::from("GeodeticCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("datum".into(), frame);
    out.insert("coordinate_system".into(), cs_of(node, "Cartesian")?);
    id_or_none(node, &mut out);
    Some(Value::Object(out))
}

fn compound(node: &Node) -> Option<Value> {
    // The horizontal component is what a transformation uses; a vertical CRS is skipped.
    let components: Vec<Value> = node
        .args
        .iter()
        .filter_map(|a| match a {
            Arg::Node(n) => from_node(n, false),
            _ => None,
        })
        .collect();
    if components.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("$schema".into(), Value::from(SCHEMA));
    out.insert("type".into(), Value::from("CompoundCRS"));
    out.insert("name".into(), Value::from(node.text(0)?));
    out.insert("components".into(), Value::Array(components));
    id_or_none(node, &mut out);
    Some(Value::Object(out))
}

/// A WKT2 `BOUNDCRS`: the source CRS with its abridged transformation to the target.
fn bound_full(node: &Node) -> Option<Value> {
    let source = node.child("SOURCECRS")?.args.iter().find_map(|a| match a {
        Arg::Node(n) => Some(n),
        _ => None,
    })?;
    let transformation = node.child("ABRIDGEDTRANSFORMATION")?;
    let mut method = Map::new();
    let m = transformation.child("METHOD")?;
    method.insert("name".into(), Value::from(m.text(0)?));
    id_or_none(m, &mut method);
    Some(json!({
        "$schema": SCHEMA,
        "type": "BoundCRS",
        "source_crs": from_node(source, false)?,
        "transformation": {
            "name": transformation.text(0)?,
            "method": Value::Object(method),
            "parameters": parameters(transformation)?,
        },
    }))
}

/// Any WKT2 CRS root as PROJJSON.
pub(crate) fn from_node(node: &Node, with_schema: bool) -> Option<Value> {
    match node.keyword.as_str() {
        "GEOGCRS" | "GEOGRAPHICCRS" => geographic(node, with_schema),
        "GEODCRS" | "GEODETICCRS" => {
            let is_cartesian = node.args.iter().any(|a| {
                matches!(a, Arg::Node(n) if n.keyword == "CS" && n.text(0) == Some("Cartesian"))
            });
            if is_cartesian {
                geocentric(node, with_schema)
            } else {
                geographic(node, with_schema)
            }
        }
        "PROJCRS" | "PROJECTEDCRS" => projected(node, with_schema),
        "BOUNDCRS" => bound_full(node),
        "COMPOUNDCRS" => compound(node),
        _ => None,
    }
}
