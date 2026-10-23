//! Thin Rustler wrapper around `geoframe_core`. All logic and its tests live in the core
//! crate; this layer only converts between BEAM terms and core types.

use geoframe_core::{read_geoparquet, write_geoparquet, GeoColumn};
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
