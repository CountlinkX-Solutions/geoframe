//! Thin Rustler wrapper around `geoframe_core`. All logic and its tests live in the core
//! crate; this layer only converts between BEAM terms and core types.

use geoframe_core::{
    read_geoparquet, write_geoparquet, BufferOptions, GeoColumn, Origin, Predicate, QueryPredicate,
    SetOp, UnaryOp,
};
use rustler::{Binary, Env, OwnedBinary, ResourceArc};

/// Resource handle for a GeoArrow geometry column living in Rust memory.
pub struct GeoColumnResource(GeoColumn);

#[rustler::resource_impl]
impl rustler::Resource for GeoColumnResource {}

type Handle = ResourceArc<GeoColumnResource>;

fn wrap(column: GeoColumn) -> Handle {
    ResourceArc::new(GeoColumnResource(column))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn from_wkt(rows: Vec<Option<String>>, crs: Option<String>) -> Result<Handle, String> {
    GeoColumn::from_wkt(&rows, crs.as_deref()).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn from_wkb(rows: Vec<Option<Binary>>, crs: Option<String>) -> Result<Handle, String> {
    let rows: Vec<Option<&[u8]>> = rows
        .iter()
        .map(|r| r.as_ref().map(|b| b.as_slice()))
        .collect();
    GeoColumn::from_wkb(&rows, crs.as_deref()).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn to_wkt(column: Handle) -> Result<Vec<Option<String>>, String> {
    column.0.to_wkt()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn to_wkb<'a>(env: Env<'a>, column: Handle) -> Result<Vec<Option<Binary<'a>>>, String> {
    let rows = column.0.to_wkb()?;
    Ok(rows
        .into_iter()
        .map(|row| {
            row.map(|bytes| {
                let mut owned = OwnedBinary::new(bytes.len());
                owned.as_mut_slice().copy_from_slice(&bytes);
                Binary::from_owned(owned, env)
            })
        })
        .collect())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn to_geojson(column: Handle) -> Vec<Option<String>> {
    column.0.to_geojson()
}

#[rustler::nif]
fn len(column: Handle) -> usize {
    column.0.len()
}

#[rustler::nif]
fn crs(column: Handle) -> Option<String> {
    column.0.crs()
}

#[rustler::nif]
fn set_crs(column: Handle, crs: Option<String>) -> Handle {
    wrap(column.0.set_crs(crs.as_deref()))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn take(column: Handle, indices: Vec<usize>) -> Result<Handle, String> {
    column.0.take(&indices).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn area(column: Handle) -> Vec<Option<f64>> {
    column.0.area()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn length(column: Handle) -> Vec<Option<f64>> {
    column.0.length()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn validity(column: Handle) -> Vec<Option<bool>> {
    column.0.is_valid()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn bounds(column: Handle) -> Vec<Option<(f64, f64, f64, f64)>> {
    column
        .0
        .bounds()
        .into_iter()
        .map(|b| b.map(|[a, b, c, d]| (a, b, c, d)))
        .collect()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn total_bounds(column: Handle) -> Option<(f64, f64, f64, f64)> {
    column.0.total_bounds().map(|[a, b, c, d]| (a, b, c, d))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn centroid(column: Handle) -> Result<Handle, String> {
    column.0.centroid().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn predicate(left: Handle, right: Handle, name: String) -> Result<Vec<Option<bool>>, String> {
    let predicate =
        Predicate::from_name(&name).ok_or_else(|| format!("unknown predicate: {name}"))?;
    left.0.predicate(&right.0, predicate)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn distance(left: Handle, right: Handle) -> Result<Vec<Option<f64>>, String> {
    left.0.distance(&right.0)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn dwithin(left: Handle, right: Handle, limit: f64) -> Result<Vec<Option<bool>>, String> {
    left.0.dwithin(&right.0, limit)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn sindex_query(
    left: Handle,
    right: Handle,
    name: String,
    distance: Option<f64>,
) -> Result<(Vec<u32>, Vec<u32>), String> {
    let predicate = QueryPredicate::from_name(&name, distance)?;
    left.0.query_pairs(&right.0, predicate)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn relate(left: Handle, right: Handle) -> Result<Vec<Option<String>>, String> {
    left.0.relate(&right.0)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn set_op(left: Handle, right: Handle, name: String) -> Result<Handle, String> {
    let op = SetOp::from_name(&name).ok_or_else(|| format!("unknown set operation: {name}"))?;
    left.0.set_op(&right.0, op).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn union_all(column: Handle) -> Result<Handle, String> {
    column.0.union_all().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn unary(column: Handle, name: String) -> Result<Handle, String> {
    let op = UnaryOp::from_name(&name).ok_or_else(|| format!("unknown operation: {name}"))?;
    column.0.unary(op).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn buffer(
    column: Handle,
    distance: f64,
    cap: String,
    join: String,
    quad_segs: u32,
    mitre_limit: f64,
) -> Result<Handle, String> {
    let options = BufferOptions {
        cap,
        join,
        quad_segs,
        mitre_limit,
    };
    column.0.buffer(distance, &options).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn simplify(column: Handle, tolerance: f64, preserve_topology: bool) -> Result<Handle, String> {
    column.0.simplify(tolerance, preserve_topology).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn translate(column: Handle, xoff: f64, yoff: f64) -> Result<Handle, String> {
    column.0.translate(xoff, yoff).map(wrap)
}

/// `origin` is `"center"`, `"centroid"` or `"coord"` (then `ox`/`oy` apply).
fn parse_origin(origin: &str, ox: f64, oy: f64) -> Result<Origin, String> {
    match origin {
        "center" => Ok(Origin::Center),
        "centroid" => Ok(Origin::Centroid),
        "coord" => Ok(Origin::Coord(ox, oy)),
        other => Err(format!("unknown origin: {other}")),
    }
}

#[rustler::nif(schedule = "DirtyCpu")]
fn scale(
    column: Handle,
    xfact: f64,
    yfact: f64,
    origin: String,
    ox: f64,
    oy: f64,
) -> Result<Handle, String> {
    let origin = parse_origin(&origin, ox, oy)?;
    column.0.scale(xfact, yfact, &origin).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn rotate(
    column: Handle,
    degrees: f64,
    origin: String,
    ox: f64,
    oy: f64,
) -> Result<Handle, String> {
    let origin = parse_origin(&origin, ox, oy)?;
    column.0.rotate(degrees, &origin).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn x(column: Handle) -> Vec<Option<f64>> {
    column.0.x()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn y(column: Handle) -> Vec<Option<f64>> {
    column.0.y()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn geom_type(column: Handle) -> Vec<Option<String>> {
    column.0.geom_type()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn empty_flags(column: Handle) -> Vec<Option<bool>> {
    column.0.is_empty_rows()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn count_coordinates(column: Handle) -> Vec<Option<usize>> {
    column.0.count_coordinates()
}

/// Reads a GeoParquet file. Returns `{geometry, geometry_column, attributes_ipc}` where the
/// attributes are an Arrow IPC stream (one copy; Explorer has no zero-copy NIF access).
#[rustler::nif(schedule = "DirtyIo")]
fn read_parquet<'a>(
    env: Env<'a>,
    path: String,
    columns: Option<Vec<String>>,
) -> Result<(Handle, String, Binary<'a>), String> {
    let read = read_geoparquet(&path, columns.as_deref())?;
    let mut ipc = OwnedBinary::new(read.attributes_ipc.len());
    ipc.as_mut_slice().copy_from_slice(&read.attributes_ipc);
    Ok((
        wrap(read.geometry),
        read.geometry_column,
        Binary::from_owned(ipc, env),
    ))
}

#[rustler::nif(schedule = "DirtyIo")]
fn write_parquet(
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    path: String,
) -> Result<(), String> {
    write_geoparquet(
        &path,
        &column.0,
        &geometry_column,
        attributes_ipc.as_slice(),
    )
}

rustler::init!("Elixir.GeoFrame.Native");
