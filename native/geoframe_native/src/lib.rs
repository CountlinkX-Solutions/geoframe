//! Thin Rustler wrapper around `geoframe_core`. All logic and its tests live in the core
//! crate; this layer only converts between BEAM terms and core types.

use geoframe_core::{
    deck_buffers, from_arrow_ipc, geoparquet_metadata, gpkg_info, gpkg_layers, open_batch_readers,
    read_feather, read_flatgeobuf, read_geojson, read_geojson_str, read_geoparquet_filtered,
    read_gpkg, read_shapefile, shapefile_info, to_arrow_ipc_multi_batched,
    write_feather_multi_batched, write_flatgeobuf, write_geojson, write_geoparquet_multi,
    write_gpkg, write_shapefile, BatchReader, BufferOptions, FeatherCompression, GeoColumn,
    GeometryEncoding, GpkgWriteOptions, GridFeature, GridType, GroupAgg, JoinHow, LayerInfo,
    NearestPairs, OffsetJoin, OffsetParams, Origin, ParquetOptions, PrecisionMode, Predicate,
    QueryPredicate, SetOp, TableRead, UnaryOp, VectorRead, VertexKind, VertexParts,
};
use rustler::{Binary, Encoder, Env, OwnedBinary, ResourceArc, Term};
use std::sync::Mutex;

rustler::atoms! {
    ok,
    nan,
    infinity,
    neg_infinity,
}

/// A float result that may be non-finite. `enif_make_double` cannot encode NaN or infinity
/// (the call raises `badarg`), so those cross as the atoms Explorer's `f64` series accepts
/// and returns: `:nan`, `:infinity` and `:neg_infinity`.
pub struct Real(f64);

impl Encoder for Real {
    fn encode<'a>(&self, env: Env<'a>) -> Term<'a> {
        if self.0.is_nan() {
            nan().encode(env)
        } else if self.0 == f64::INFINITY {
            infinity().encode(env)
        } else if self.0 == f64::NEG_INFINITY {
            neg_infinity().encode(env)
        } else {
            self.0.encode(env)
        }
    }
}

fn reals(values: Vec<Option<f64>>) -> Vec<Option<Real>> {
    values.into_iter().map(|v| v.map(Real)).collect()
}

fn reals_result(values: Result<Vec<Option<f64>>, String>) -> Result<Vec<Option<Real>>, String> {
    values.map(reals)
}

/// Resource handle for a GeoArrow geometry column living in Rust memory.
pub struct GeoColumnResource(GeoColumn);

#[rustler::resource_impl]
impl rustler::Resource for GeoColumnResource {}

type Handle = ResourceArc<GeoColumnResource>;

/// Secondary geometry columns of a table: `{name, column}`.
type Secondary = Vec<(String, Handle)>;

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
fn to_ewkb<'a>(env: Env<'a>, column: Handle, srid: i32) -> Result<Vec<Option<Binary<'a>>>, String> {
    let rows = column.0.to_ewkb(srid)?;
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

/// `take` with the indices as a native-endian `u32` binary (no list of a million integers).
#[rustler::nif(schedule = "DirtyCpu")]
fn take_u32(column: Handle, indices: Binary) -> Result<Handle, String> {
    let indices: Vec<usize> = u32_values(indices.as_slice())?
        .into_iter()
        .map(|i| i as usize)
        .collect();
    column.0.take(&indices).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn area(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.area())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn length(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.length())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn area_geodesic(column: Handle) -> Result<Vec<Option<Real>>, String> {
    reals_result(column.0.area_geodesic())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn length_geodesic(column: Handle) -> Result<Vec<Option<Real>>, String> {
    reals_result(column.0.length_geodesic())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn distance_geodesic(
    left: Handle,
    right: Handle,
    haversine: bool,
) -> Result<Vec<Option<Real>>, String> {
    reals_result(left.0.distance_geodesic(&right.0, haversine))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn validity(column: Handle) -> Vec<Option<bool>> {
    column.0.is_valid()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn bounds(column: Handle) -> Vec<Option<(Real, Real, Real, Real)>> {
    column
        .0
        .bounds()
        .into_iter()
        .map(|b| b.map(|[a, b, c, d]| (Real(a), Real(b), Real(c), Real(d))))
        .collect()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn total_bounds(column: Handle) -> Option<(Real, Real, Real, Real)> {
    column
        .0
        .total_bounds()
        .map(|[a, b, c, d]| (Real(a), Real(b), Real(c), Real(d)))
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
fn distance(left: Handle, right: Handle) -> Result<Vec<Option<Real>>, String> {
    reals_result(left.0.distance(&right.0))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn dwithin(left: Handle, right: Handle, limit: f64) -> Result<Vec<Option<bool>>, String> {
    left.0.dwithin(&right.0, limit)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn sindex_query<'a>(
    env: Env<'a>,
    left: Handle,
    right: Handle,
    name: String,
    distance: Option<f64>,
) -> Result<(Binary<'a>, Binary<'a>), String> {
    let predicate = QueryPredicate::from_name(&name, distance)?;
    let (l, r) = left.0.query_pairs(&right.0, predicate)?;
    Ok((u32_binary(env, &l), u32_binary(env, &r)))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn sindex_nearest<'a>(
    env: Env<'a>,
    left: Handle,
    right: Handle,
    max_distance: Option<f64>,
    exclusive: bool,
) -> Result<(Binary<'a>, Binary<'a>, Binary<'a>), String> {
    let (l, r, d): NearestPairs = left.0.nearest_pairs(&right.0, max_distance, exclusive)?;
    Ok((
        u32_binary(env, &l),
        u32_binary(env, &r),
        f64_binary(env, &d),
    ))
}

/// Lays out the rows of a spatial join from matched pairs (native-endian `u32` binaries, and
/// optionally `f64` distances): the sort order of `sjoin` plus the unmatched rows of the
/// preserved side. Missing partners are `u32::MAX`; their distance is NaN.
#[rustler::nif(schedule = "DirtyCpu")]
fn join_rows<'a>(
    env: Env<'a>,
    left: Binary,
    right: Binary,
    distances: Option<Binary>,
    how: String,
    n_left: usize,
    n_right: usize,
) -> Result<(Binary<'a>, Binary<'a>, Binary<'a>), String> {
    let how = JoinHow::from_name(&how).ok_or_else(|| format!("unknown join kind: {how}"))?;
    let left = u32_values(left.as_slice())?;
    let right = u32_values(right.as_slice())?;
    let distances = distances.map(|d| f64_values(d.as_slice())).transpose()?;
    let rows = geoframe_core::join_rows(&left, &right, distances.as_deref(), how, n_left, n_right)?;
    Ok((
        u32_binary(env, &rows.left),
        u32_binary(env, &rows.right),
        f64_binary(env, &rows.distances),
    ))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn relate(left: Handle, right: Handle) -> Result<Vec<Option<String>>, String> {
    left.0.relate(&right.0)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn relate_pattern(
    left: Handle,
    right: Handle,
    pattern: String,
) -> Result<Vec<Option<bool>>, String> {
    left.0.relate_pattern(&right.0, &pattern)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn equals_exact(left: Handle, right: Handle, tolerance: f64) -> Result<Vec<Option<bool>>, String> {
    left.0.equals_exact(&right.0, tolerance)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn equals_identical(left: Handle, right: Handle) -> Result<Vec<Option<bool>>, String> {
    left.0.equals_identical(&right.0)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn bbox_intersection(column: Handle, bbox: (f64, f64, f64, f64)) -> Result<Vec<u32>, String> {
    column.0.bbox_intersection([bbox.0, bbox.1, bbox.2, bbox.3])
}

#[rustler::nif(schedule = "DirtyCpu")]
fn concat(columns: Vec<Handle>) -> Result<Handle, String> {
    let refs: Vec<&GeoColumn> = columns.iter().map(|c| &c.0).collect();
    GeoColumn::concat(&refs).map(wrap)
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
fn intersection_all(column: Handle) -> Result<Handle, String> {
    column.0.intersection_all().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn explode(column: Handle) -> Result<(Handle, Vec<u32>), String> {
    column
        .0
        .explode()
        .map(|(parts, source)| (wrap(parts), source))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn clip_by_rect(
    column: Handle,
    xmin: f64,
    ymin: f64,
    xmax: f64,
    ymax: f64,
) -> Result<Handle, String> {
    column.0.clip_by_rect(xmin, ymin, xmax, ymax).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn union_groups(column: Handle, group_ids: Vec<u32>, n_groups: usize) -> Result<Handle, String> {
    column.0.union_groups(&group_ids, n_groups).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn aggregate_groups(
    column: Handle,
    group_ids: Vec<u32>,
    n_groups: usize,
    agg: String,
) -> Result<Handle, String> {
    let agg = GroupAgg::from_name(&agg).ok_or_else(|| format!("unknown aggregation: {agg}"))?;
    column
        .0
        .aggregate_groups(&group_ids, n_groups, agg)
        .map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn unary(column: Handle, name: String) -> Result<Handle, String> {
    let op = UnaryOp::from_name(&name).ok_or_else(|| format!("unknown operation: {name}"))?;
    column.0.unary(op).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn buffer(
    column: Handle,
    distances: Vec<Option<f64>>,
    cap: String,
    join: String,
    quad_segs: u32,
    mitre_limit: f64,
    single_sided: bool,
) -> Result<Handle, String> {
    let options = BufferOptions {
        cap,
        join,
        quad_segs,
        mitre_limit,
        single_sided,
    };
    column.0.buffer_rows(&distances, &options).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn simplify(column: Handle, tolerance: f64, preserve_topology: bool) -> Result<Handle, String> {
    column.0.simplify(tolerance, preserve_topology).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn to_crs(column: Handle, target: String) -> Result<Handle, String> {
    column.0.to_crs(&target).map(wrap)
}

/// `[{file_name, path | nil}]`: the grid files that may shift the datum of `crs`.
#[rustler::nif]
fn grid_status(crs: String) -> Result<Vec<(String, Option<String>)>, String> {
    geoframe_core::grid_status(&crs)
}

#[rustler::nif]
fn set_grid_dirs(dirs: Vec<String>) {
    geoframe_core::set_grid_dirs(dirs)
}

#[rustler::nif]
fn grid_dirs() -> Vec<String> {
    geoframe_core::grid_dirs()
}

#[rustler::nif]
fn register_grids(crs: String, names: Vec<String>) -> Result<u32, String> {
    geoframe_core::register_grids_for(&crs, names)
}

#[rustler::nif]
fn unregister_grids(crs: String) -> Result<u32, String> {
    geoframe_core::unregister_grids_for(&crs)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn estimate_utm_crs(column: Handle, datum_name: Option<String>) -> Result<String, String> {
    column.0.estimate_utm_crs(datum_name.as_deref())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn translate(column: Handle, xoff: f64, yoff: f64, zoff: f64) -> Result<Handle, String> {
    column.0.translate3(xoff, yoff, zoff).map(wrap)
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
    zfact: f64,
    origin: String,
    ox: f64,
    oy: f64,
) -> Result<Handle, String> {
    let origin = parse_origin(&origin, ox, oy)?;
    column.0.scale3(xfact, yfact, zfact, &origin).map(wrap)
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
fn x(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.x())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn y(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.y())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn has_z(column: Handle) -> Vec<bool> {
    column.0.has_z()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn has_m(column: Handle) -> Vec<bool> {
    column.0.has_m()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn get_z(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.z())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn get_m(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.m())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn force_2d(column: Handle) -> Result<Handle, String> {
    column.0.force_2d().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn force_3d(column: Handle, z: Vec<Option<f64>>) -> Result<Handle, String> {
    column.0.force_3d(&z).map(wrap)
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

#[rustler::nif(schedule = "DirtyCpu")]
fn from_xy(
    x: Vec<Option<f64>>,
    y: Vec<Option<f64>>,
    crs: Option<String>,
) -> Result<Handle, String> {
    GeoColumn::from_xy(&x, &y, crs.as_deref()).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn get_coordinates(column: Handle) -> (Vec<u32>, Vec<f64>, Vec<f64>) {
    column.0.get_coordinates()
}

/// `(row, x, y, z, m)`; an absent ordinate is `None`.
type CoordinateTable = (
    Vec<u32>,
    Vec<f64>,
    Vec<f64>,
    Vec<Option<f64>>,
    Vec<Option<f64>>,
);

#[rustler::nif(schedule = "DirtyCpu")]
fn get_coordinates_zm(column: Handle) -> CoordinateTable {
    // NaN cannot cross the NIF boundary as a float: an absent ordinate is `nil`.
    let (rows, xs, ys, zs, ms) = column.0.get_coordinates_zm();
    let present = |values: Vec<f64>| -> Vec<Option<f64>> {
        values
            .into_iter()
            .map(|v| (!v.is_nan()).then_some(v))
            .collect()
    };
    (rows, xs, ys, present(zs), present(ms))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn count_geometries(column: Handle) -> Vec<Option<usize>> {
    column.0.count_geometries()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn count_interior_rings(column: Handle) -> Vec<Option<usize>> {
    column.0.count_interior_rings()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn get_geometry(column: Handle, n: i64) -> Result<Handle, String> {
    column.0.get_geometry(n).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn get_geometry_rows(column: Handle, indices: Vec<Option<i64>>) -> Result<Handle, String> {
    column.0.get_geometry_rows(&indices).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn interiors(column: Handle) -> Result<Handle, String> {
    column.0.interiors().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn extract_unique_points(column: Handle) -> Result<Handle, String> {
    column.0.extract_unique_points().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mean_coordinate(column: Handle) -> Result<Handle, String> {
    column.0.mean_coordinate().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn collect(column: Handle, multi: bool) -> Result<Handle, String> {
    column.0.collect(multi).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn null_rows(column: Handle) -> Vec<bool> {
    column.0.null_rows()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn fill_null(column: Handle, fill: Handle) -> Result<Handle, String> {
    column.0.fill_null(&fill.0).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn closed_flags(column: Handle) -> Vec<Option<bool>> {
    column.0.is_closed()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn ring_flags(column: Handle) -> Vec<Option<bool>> {
    column.0.is_ring()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn valid_reasons(column: Handle) -> Vec<Option<String>> {
    column.0.is_valid_reason()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn null_out_invalid(column: Handle) -> Result<Handle, String> {
    column.0.null_out_invalid().map(wrap)
}

/// Parses GeoJSON text. Returns `{geometry, attributes_ipc}`.
#[rustler::nif(schedule = "DirtyCpu")]
fn read_geojson_text<'a>(
    env: Env<'a>,
    text: String,
    crs: Option<String>,
) -> Result<(Handle, Binary<'a>), String> {
    read_geojson_str(&text, crs.as_deref()).map(|read| vector_read(env, read))
}

/// Reads a GeoParquet file. Returns `{geometry, geometry_column, attributes_ipc, secondary}`
/// where the attributes are an Arrow IPC stream (one copy; Explorer has no zero-copy NIF
/// access) and `secondary` lists `{name, geometry}` for the other geometry columns.
/// `bbox` keeps only the rows whose bounding box intersects it.
#[rustler::nif(schedule = "DirtyIo")]
fn read_parquet<'a>(
    env: Env<'a>,
    path: String,
    columns: Option<Vec<String>>,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Result<(Handle, String, Binary<'a>, Secondary), String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    let read = read_geoparquet_filtered(&path, columns.as_deref(), bbox)?;
    let ipc = binary(env, &read.attributes_ipc);
    let secondary = read
        .secondary_geometries
        .into_iter()
        .map(|(name, column)| (name, wrap(column)))
        .collect();
    Ok((wrap(read.geometry), read.geometry_column, ipc, secondary))
}

/// `options` is `{compression, compression_level, row_group_size, geometry_encoding,
/// covering_bbox, schema_version}`; `secondary` lists `{name, geometry}` of the other geometry
/// columns, written after the primary one.
#[rustler::nif(schedule = "DirtyIo")]
fn write_parquet(
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    path: String,
    options: (String, Option<i32>, Option<usize>, String, bool, String),
    secondary: Secondary,
) -> Result<(), String> {
    let (
        compression,
        compression_level,
        row_group_size,
        geometry_encoding,
        covering_bbox,
        schema_version,
    ) = options;
    let options = ParquetOptions {
        compression,
        compression_level,
        row_group_size,
        geometry_encoding: GeometryEncoding::parse(&geometry_encoding)?,
        covering_bbox,
        schema_version,
    };
    let mut geometries: Vec<(&str, &GeoColumn)> = vec![(geometry_column.as_str(), &column.0)];
    geometries.extend(
        secondary
            .iter()
            .map(|(name, handle)| (name.as_str(), &handle.0)),
    );
    write_geoparquet_multi(&path, &geometries, attributes_ipc.as_slice(), &options)
}

fn table_read<'a>(env: Env<'a>, read: TableRead) -> (Handle, String, Binary<'a>, Secondary) {
    let ipc = binary(env, &read.attributes_ipc);
    let secondary = read
        .secondary_geometries
        .into_iter()
        .map(|(name, column)| (name, wrap(column)))
        .collect();
    (wrap(read.geometry), read.geometry_column, ipc, secondary)
}

/// The primary geometry column followed by the secondary ones, as `(name, column)` pairs.
fn geometry_columns<'a>(
    column: &'a Handle,
    geometry_column: &'a str,
    secondary: &'a Secondary,
) -> Vec<(&'a str, &'a GeoColumn)> {
    let mut geometries = vec![(geometry_column, &column.0)];
    geometries.extend(
        secondary
            .iter()
            .map(|(name, handle)| (name.as_str(), &handle.0)),
    );
    geometries
}

/// Reads a Feather v2 file. Returns `{geometry, geometry_column, attributes_ipc, secondary}`
/// where `secondary` lists `{name, geometry}` for the other geometry columns.
#[rustler::nif(schedule = "DirtyIo")]
fn read_feather_file<'a>(
    env: Env<'a>,
    path: String,
    columns: Option<Vec<String>>,
    geometry_column: Option<String>,
) -> Result<(Handle, String, Binary<'a>, Secondary), String> {
    read_feather(&path, columns.as_deref(), geometry_column.as_deref())
        .map(|read| table_read(env, read))
}

/// `options` is `{compression, record_batch_size}`; `secondary` lists `{name, geometry}` of the
/// other geometry columns, written after the primary.
#[rustler::nif(schedule = "DirtyIo")]
fn write_feather_file(
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    path: String,
    geometry_encoding: String,
    options: (String, usize),
    secondary: Secondary,
) -> Result<(), String> {
    let (compression, record_batch_size) = options;
    write_feather_multi_batched(
        &path,
        &geometry_columns(&column, &geometry_column, &secondary),
        attributes_ipc.as_slice(),
        GeometryEncoding::parse(&geometry_encoding)?,
        FeatherCompression::parse(&compression)?,
        record_batch_size,
    )
}

/// Encodes geometry plus attributes (empty bytes: none) as an Arrow IPC stream; `secondary`
/// lists `{name, geometry}` of the other geometry columns.
#[rustler::nif(schedule = "DirtyCpu")]
fn to_arrow_stream<'a>(
    env: Env<'a>,
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    geometry_encoding: String,
    record_batch_size: usize,
    secondary: Secondary,
) -> Result<Binary<'a>, String> {
    let bytes = to_arrow_ipc_multi_batched(
        &geometry_columns(&column, &geometry_column, &secondary),
        attributes_ipc.as_slice(),
        GeometryEncoding::parse(&geometry_encoding)?,
        record_batch_size,
    )?;
    Ok(binary(env, &bytes))
}

/// Decodes an Arrow IPC stream with a geometry column (WKB, WKT or native GeoArrow).
#[rustler::nif(schedule = "DirtyCpu")]
fn from_arrow_stream<'a>(
    env: Env<'a>,
    bytes: Binary,
    columns: Option<Vec<String>>,
    geometry_column: Option<String>,
) -> Result<(Handle, String, Binary<'a>, Secondary), String> {
    from_arrow_ipc(
        bytes.as_slice(),
        columns.as_deref(),
        geometry_column.as_deref(),
    )
    .map(|read| table_read(env, read))
}

/// Flat binary buffers for the deck.gl widget: named buffers plus the count of skipped parts.
/// `colors` (RGBA per row) and `sizes` (f32 per row) are optional (empty binary = none).
#[rustler::nif(schedule = "DirtyCpu")]
fn deck_buffers_nif<'a>(
    env: Env<'a>,
    column: Handle,
    colors: Binary,
    sizes: Binary,
) -> (Vec<(String, Binary<'a>)>, usize) {
    let (list, skipped) = deck_buffers(&column.0, colors.as_slice(), sizes.as_slice());
    let list = list
        .into_iter()
        .map(|(name, bytes)| (name.to_string(), binary(env, &bytes)))
        .collect();
    (list, skipped)
}

/// Native-endian `u32` values as a binary (what `Explorer.Series.from_binary(_, {:u, 32})` reads).
fn u32_binary<'a>(env: Env<'a>, values: &[u32]) -> Binary<'a> {
    let mut owned = OwnedBinary::new(values.len() * 4);
    let (slots, _) = owned.as_mut_slice().as_chunks_mut::<4>();
    for (slot, value) in slots.iter_mut().zip(values) {
        *slot = value.to_ne_bytes();
    }
    Binary::from_owned(owned, env)
}

fn f64_binary<'a>(env: Env<'a>, values: &[f64]) -> Binary<'a> {
    let mut owned = OwnedBinary::new(values.len() * 8);
    let (slots, _) = owned.as_mut_slice().as_chunks_mut::<8>();
    for (slot, value) in slots.iter_mut().zip(values) {
        *slot = value.to_ne_bytes();
    }
    Binary::from_owned(owned, env)
}

fn u32_values(bytes: &[u8]) -> Result<Vec<u32>, String> {
    match bytes.as_chunks::<4>() {
        (chunks, []) => Ok(chunks.iter().map(|c| u32::from_ne_bytes(*c)).collect()),
        _ => Err("a u32 binary must be a multiple of 4 bytes".to_string()),
    }
}

fn f64_values(bytes: &[u8]) -> Result<Vec<f64>, String> {
    match bytes.as_chunks::<8>() {
        (chunks, []) => Ok(chunks.iter().map(|c| f64::from_ne_bytes(*c)).collect()),
        _ => Err("an f64 binary must be a multiple of 8 bytes".to_string()),
    }
}

fn binary<'a>(env: Env<'a>, bytes: &[u8]) -> Binary<'a> {
    let mut owned = OwnedBinary::new(bytes.len());
    owned.as_mut_slice().copy_from_slice(bytes);
    Binary::from_owned(owned, env)
}

fn vector_read<'a>(env: Env<'a>, read: VectorRead) -> (Handle, Binary<'a>) {
    let ipc = binary(env, &read.attributes_ipc);
    (wrap(read.geometry), ipc)
}

/// Reads a GeoJSON file. Returns `{geometry, attributes_ipc}`.
#[rustler::nif(schedule = "DirtyIo")]
fn read_geojson_file<'a>(env: Env<'a>, path: String) -> Result<(Handle, Binary<'a>), String> {
    read_geojson(&path).map(|read| vector_read(env, read))
}

#[rustler::nif(schedule = "DirtyIo")]
fn write_geojson_file(
    column: Handle,
    attributes_ipc: Binary,
    path: String,
    force: bool,
) -> Result<(), String> {
    write_geojson(&path, &column.0, attributes_ipc.as_slice(), force)
}

/// Reads a FlatGeobuf file, optionally restricted to a bounding box via the file's index.
#[rustler::nif(schedule = "DirtyIo")]
fn read_flatgeobuf_file<'a>(
    env: Env<'a>,
    path: String,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Result<(Handle, Binary<'a>), String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    read_flatgeobuf(&path, bbox).map(|read| vector_read(env, read))
}

#[rustler::nif(schedule = "DirtyIo")]
fn write_flatgeobuf_file(
    column: Handle,
    attributes_ipc: Binary,
    path: String,
    layer_name: String,
    index: bool,
) -> Result<(), String> {
    write_flatgeobuf(
        &path,
        &column.0,
        attributes_ipc.as_slice(),
        &layer_name,
        index,
    )
}

/// Layer facts for `read_file_info/2`.
#[derive(rustler::NifMap)]
struct LayerInfoTerm {
    name: String,
    driver: String,
    crs: Option<String>,
    encoding: String,
    geometry_type: Option<String>,
    features: u64,
    bounds: Option<(f64, f64, f64, f64)>,
    fields: Vec<(String, String)>,
}

fn info_term(info: LayerInfo) -> LayerInfoTerm {
    LayerInfoTerm {
        name: info.name,
        driver: info.driver,
        crs: info.crs,
        encoding: info.encoding,
        geometry_type: info.geometry_type,
        features: info.features,
        bounds: info.bounds.map(|b| (b[0], b[1], b[2], b[3])),
        fields: info.fields,
    }
}

/// Reads a shapefile (`.shp` or a zip holding one). Returns `{geometry, attributes_ipc}`.
#[rustler::nif(schedule = "DirtyIo")]
fn read_shapefile_file<'a>(
    env: Env<'a>,
    path: String,
    encoding: Option<String>,
) -> Result<(Handle, Binary<'a>), String> {
    read_shapefile(&path, encoding.as_deref()).map(|read| vector_read(env, read))
}

/// Writes a shapefile; returns the warnings.
#[rustler::nif(schedule = "DirtyIo")]
fn write_shapefile_file(
    column: Handle,
    attributes_ipc: Binary,
    path: String,
    encoding: Option<String>,
) -> Result<Vec<String>, String> {
    write_shapefile(
        &path,
        &column.0,
        attributes_ipc.as_slice(),
        encoding.as_deref(),
    )
}

#[rustler::nif(schedule = "DirtyIo")]
fn shapefile_file_info(path: String, encoding: Option<String>) -> Result<LayerInfoTerm, String> {
    shapefile_info(&path, encoding.as_deref()).map(info_term)
}

/// Reads one GeoPackage layer. Returns `{geometry, attributes_ipc, layer_name}`.
#[rustler::nif(schedule = "DirtyIo")]
fn read_gpkg_file<'a>(
    env: Env<'a>,
    path: String,
    layer: Option<String>,
) -> Result<(Handle, Binary<'a>, String), String> {
    read_gpkg(&path, layer.as_deref()).map(|read| {
        let (handle, ipc) = vector_read(env, read.vector);
        (handle, ipc, read.layer)
    })
}

/// Writes a GeoPackage layer; returns the warnings.
#[rustler::nif(schedule = "DirtyIo")]
fn write_gpkg_file(
    column: Handle,
    attributes_ipc: Binary,
    path: String,
    layer: String,
    append: bool,
    spatial_index: bool,
) -> Result<Vec<String>, String> {
    write_gpkg(
        &path,
        &column.0,
        attributes_ipc.as_slice(),
        &layer,
        GpkgWriteOptions {
            append,
            spatial_index,
        },
    )
}

/// The layers of a GeoPackage as `{name, geometry_type}`.
#[rustler::nif(schedule = "DirtyIo")]
fn gpkg_layer_list(path: String) -> Result<Vec<(String, Option<String>)>, String> {
    gpkg_layers(&path).map(|layers| {
        layers
            .into_iter()
            .map(|l| (l.name, l.geometry_type))
            .collect()
    })
}

#[rustler::nif(schedule = "DirtyIo")]
fn gpkg_layer_info(path: String, layer: Option<String>) -> Result<LayerInfoTerm, String> {
    gpkg_info(&path, layer.as_deref()).map(info_term)
}

/// Per-row origins: `"coord"` reads `ox`/`oy` (a `None` in either nulls the row); the other
/// modes ignore them.
fn row_origins(
    origin: &str,
    ox: &[Option<f64>],
    oy: &[Option<f64>],
    rows: usize,
) -> Result<Vec<Option<Origin>>, String> {
    match origin {
        "coord" => {
            if ox.len() != rows || oy.len() != rows {
                return Err(format!(
                    "origin has {} entries but the series has {rows} rows",
                    ox.len()
                ));
            }
            Ok(ox
                .iter()
                .zip(oy)
                .map(|(x, y)| Some(Origin::Coord((*x)?, (*y)?)))
                .collect())
        }
        other => {
            let fixed = parse_origin(other, 0.0, 0.0)?;
            Ok(vec![Some(fixed); rows])
        }
    }
}

#[rustler::nif(schedule = "DirtyCpu")]
fn translate_rows(
    column: Handle,
    xoff: Vec<Option<f64>>,
    yoff: Vec<Option<f64>>,
) -> Result<Handle, String> {
    column.0.translate_rows(&xoff, &yoff).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn scale_rows(
    column: Handle,
    xfact: Vec<Option<f64>>,
    yfact: Vec<Option<f64>>,
    origin: String,
    ox: Vec<Option<f64>>,
    oy: Vec<Option<f64>>,
) -> Result<Handle, String> {
    let origins = row_origins(&origin, &ox, &oy, column.0.len())?;
    column.0.scale_rows(&xfact, &yfact, &origins).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn rotate_rows(
    column: Handle,
    degrees: Vec<Option<f64>>,
    origin: String,
    ox: Vec<Option<f64>>,
    oy: Vec<Option<f64>>,
) -> Result<Handle, String> {
    let origins = row_origins(&origin, &ox, &oy, column.0.len())?;
    column.0.rotate_rows(&degrees, &origins).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
#[allow(clippy::too_many_arguments)]
fn skew_rows(
    column: Handle,
    xs: Vec<Option<f64>>,
    ys: Vec<Option<f64>>,
    origin: String,
    ox: Vec<Option<f64>>,
    oy: Vec<Option<f64>>,
    use_radians: bool,
) -> Result<Handle, String> {
    let origins = row_origins(&origin, &ox, &oy, column.0.len())?;
    column
        .0
        .skew_rows(&xs, &ys, &origins, use_radians)
        .map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn affine_transform(column: Handle, matrix: Vec<f64>) -> Result<Handle, String> {
    if let Ok(matrix) = <[f64; 12]>::try_from(matrix.as_slice()) {
        return column.0.affine_transform3(&matrix).map(wrap);
    }
    let matrix: [f64; 6] = matrix
        .try_into()
        .map_err(|_| "affine_transform needs 6 or 12 numbers".to_string())?;
    column.0.affine_transform(&matrix).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn wrap_longitude(column: Handle) -> Result<Handle, String> {
    column.0.wrap_longitude().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn interpolate(
    column: Handle,
    distance: Vec<Option<f64>>,
    normalized: bool,
) -> Result<Handle, String> {
    column.0.interpolate(&distance, normalized).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn project(left: Handle, right: Handle, normalized: bool) -> Result<Vec<Option<Real>>, String> {
    reals_result(left.0.project(&right.0, normalized))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn segmentize(column: Handle, max_segment_length: f64) -> Result<Handle, String> {
    column.0.segmentize(max_segment_length).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn remove_repeated_points(column: Handle, tolerance: f64) -> Result<Handle, String> {
    column.0.remove_repeated_points(tolerance).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn reverse(column: Handle) -> Result<Handle, String> {
    column.0.reverse().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn orient_polygons(column: Handle, exterior_cw: bool) -> Result<Handle, String> {
    column.0.orient_polygons(exterior_cw).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn ccw_flags(column: Handle) -> Vec<Option<bool>> {
    column.0.is_ccw()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn hausdorff_distance(
    left: Handle,
    right: Handle,
    densify: Option<f64>,
) -> Result<Vec<Option<Real>>, String> {
    reals_result(left.0.hausdorff_distance(&right.0, densify))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn frechet_distance(
    left: Handle,
    right: Handle,
    densify: Option<f64>,
) -> Result<Vec<Option<Real>>, String> {
    reals_result(left.0.frechet_distance(&right.0, densify))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn shortest_line(left: Handle, right: Handle) -> Result<Handle, String> {
    left.0.shortest_line(&right.0).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn line_merge(column: Handle, directed: bool) -> Result<Handle, String> {
    column.0.line_merge(directed).map(wrap)
}

// -- batch E: constructive geometry ----------------------------------------------------

#[rustler::nif(schedule = "DirtyCpu")]
fn concave_hull(column: Handle, ratio: f64, allow_holes: bool) -> Result<Handle, String> {
    column.0.concave_hull(ratio, allow_holes).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn minimum_rotated_rectangle(column: Handle) -> Result<Handle, String> {
    column.0.minimum_rotated_rectangle().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn minimum_bounding_circle(column: Handle) -> Result<Handle, String> {
    column.0.minimum_bounding_circle().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn minimum_bounding_radius(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.minimum_bounding_radius())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn maximum_inscribed_circle(column: Handle, tolerance: Option<f64>) -> Result<Handle, String> {
    column.0.maximum_inscribed_circle(tolerance).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn delaunay_triangles(column: Handle, tolerance: f64, only_edges: bool) -> Result<Handle, String> {
    column.0.delaunay_triangles(tolerance, only_edges).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn constrained_delaunay_triangles(column: Handle) -> Result<Handle, String> {
    column.0.constrained_delaunay_triangles().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn voronoi_polygons(
    column: Handle,
    tolerance: f64,
    extend_to: Option<Handle>,
    only_edges: bool,
    ordered: bool,
) -> Result<Handle, String> {
    column
        .0
        .voronoi_polygons(
            tolerance,
            extend_to.as_ref().map(|h| &h.0),
            only_edges,
            ordered,
        )
        .map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn sample_points(column: Handle, sizes: Vec<usize>, seed: Option<u64>) -> Result<Handle, String> {
    column.0.sample_points(&sizes, seed).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn make_grid(
    column: Handle,
    cell_size: f64,
    grid_type: String,
    feature_type: String,
    offset: Option<(f64, f64)>,
    intersect: bool,
    flat_topped: bool,
) -> Result<Handle, String> {
    let grid_type =
        GridType::from_name(&grid_type).ok_or_else(|| format!("unknown grid_type: {grid_type}"))?;
    let feature = GridFeature::from_name(&feature_type)
        .ok_or_else(|| format!("unknown feature_type: {feature_type}"))?;
    column
        .0
        .make_grid(
            cell_size,
            grid_type,
            feature,
            offset,
            intersect,
            flat_topped,
        )
        .map(wrap)
}

#[rustler::nif]
fn geographic(column: Handle) -> Option<bool> {
    column.0.is_geographic()
}

#[rustler::nif]
fn crs_equivalent(a: String, b: String) -> bool {
    geoframe_core::crs_equivalent(&a, &b)
}

#[rustler::nif]
fn crs_is_geographic(crs: String) -> Option<bool> {
    geoframe_core::crs_is_geographic(&crs)
}

// -- batch I1: simplicity, canonical form, precision, vertex constructors ----------------

#[rustler::nif(schedule = "DirtyCpu")]
fn simple_flags(column: Handle) -> Vec<Option<bool>> {
    column.0.is_simple()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn self_intersects_flags(column: Handle) -> Vec<Option<bool>> {
    column.0.self_intersects()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn normalize(column: Handle) -> Result<Handle, String> {
    column.0.normalize().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn set_precision(column: Handle, grid_size: f64, mode: String) -> Result<Handle, String> {
    column
        .0
        .set_precision(grid_size, PrecisionMode::parse(&mode)?)
        .map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn from_vertices(
    kind: String,
    rows: Vec<Option<VertexParts>>,
    crs: Option<String>,
) -> Result<Handle, String> {
    GeoColumn::from_vertices_z(VertexKind::parse(&kind)?, &rows, crs.as_deref()).map(wrap)
}

/// The raw `geo` metadata JSON of a GeoParquet file.
#[rustler::nif(schedule = "DirtyIo")]
fn read_geoparquet_metadata(path: String) -> Result<String, String> {
    geoparquet_metadata(&path)
}

// -- batch I2: snapping, shared paths, clearance ---------------------------------------

#[rustler::nif(schedule = "DirtyCpu")]
fn snap(left: Handle, right: Handle, tolerance: f64) -> Result<Handle, String> {
    left.0.snap(&right.0, tolerance).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn shared_paths(left: Handle, right: Handle) -> Result<Handle, String> {
    left.0.shared_paths(&right.0).map(wrap)
}

/// Per row: the clearance, `f64::MAX` when none exists (the Elixir side maps it to infinity).
#[rustler::nif(schedule = "DirtyCpu")]
fn minimum_clearance(column: Handle) -> Vec<Option<Real>> {
    reals(column.0.minimum_clearance())
}

#[rustler::nif(schedule = "DirtyCpu")]
fn minimum_clearance_line(column: Handle) -> Result<Handle, String> {
    column.0.minimum_clearance_line().map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn polygonize(column: Handle, node: bool) -> Result<(Handle, Handle, Handle, Handle), String> {
    let out = column.0.polygonize(node)?;
    Ok((
        wrap(out.polygons),
        wrap(out.cuts),
        wrap(out.dangles),
        wrap(out.invalid),
    ))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn build_area(column: Handle, node: bool) -> Result<Handle, String> {
    column.0.build_area(node).map(wrap)
}

// -- batch I3: coverages ------------------------------------------------------------------

#[rustler::nif(schedule = "DirtyCpu")]
fn valid_coverage(column: Handle, gap_width: f64) -> Result<bool, String> {
    column.0.is_valid_coverage(gap_width)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn invalid_coverage_edges(column: Handle, gap_width: f64) -> Result<Handle, String> {
    column.0.invalid_coverage_edges(gap_width).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn simplify_coverage(
    column: Handle,
    tolerance: f64,
    simplify_boundary: bool,
) -> Result<Handle, String> {
    column
        .0
        .simplify_coverage(tolerance, simplify_boundary)
        .map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn offset_curve(
    column: Handle,
    distances: Vec<Option<f64>>,
    quad_segs: u32,
    join_style: String,
    mitre_limit: f64,
) -> Result<Handle, String> {
    let params = OffsetParams {
        quad_segs,
        join: OffsetJoin::parse(&join_style)?,
        mitre_limit,
    };
    column.0.offset_curve(&distances, &params).map(wrap)
}

// -- batched reads ------------------------------------------------------------------------

/// An open file read batch by batch. `None` once closed; dropping the resource (garbage
/// collection of a stream that was never closed) releases the file as well.
pub struct ReaderResource(Mutex<Option<BatchReader>>);

#[rustler::resource_impl]
impl rustler::Resource for ReaderResource {}

type Reader = ResourceArc<ReaderResource>;

fn new_reader(reader: BatchReader) -> Reader {
    ResourceArc::new(ReaderResource(Mutex::new(Some(reader))))
}

/// Opens a GeoParquet file for batched reading (`columns` and `bbox` as `read_parquet`).
#[rustler::nif(schedule = "DirtyIo")]
fn open_parquet_reader(
    path: String,
    columns: Option<Vec<String>>,
    bbox: Option<(f64, f64, f64, f64)>,
    batch_size: usize,
) -> Result<Reader, String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    BatchReader::open_geoparquet(&path, columns.as_deref(), bbox, batch_size).map(new_reader)
}

/// Opens a FlatGeobuf file for batched reading (`bbox` as `read_flatgeobuf_file`).
#[rustler::nif(schedule = "DirtyIo")]
fn open_flatgeobuf_reader(
    path: String,
    bbox: Option<(f64, f64, f64, f64)>,
    batch_size: usize,
) -> Result<Reader, String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    BatchReader::open_flatgeobuf(&path, bbox, batch_size).map(new_reader)
}

/// Opens a Feather v2 / Arrow IPC file for batched reading (`columns` and `geometry` as
/// `read_feather_file`).
#[rustler::nif(schedule = "DirtyIo")]
fn open_feather_reader(
    path: String,
    columns: Option<Vec<String>>,
    geometry: Option<String>,
    batch_size: usize,
) -> Result<Reader, String> {
    BatchReader::open_feather(&path, columns.as_deref(), geometry.as_deref(), batch_size)
        .map(new_reader)
}

/// Opens a shapefile (`.shp` or `.zip`) for batched reading (`encoding` as
/// `read_shapefile_file`; `bbox` is applied record by record).
#[rustler::nif(schedule = "DirtyIo")]
fn open_shapefile_reader(
    path: String,
    encoding: Option<String>,
    bbox: Option<(f64, f64, f64, f64)>,
    batch_size: usize,
) -> Result<Reader, String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    BatchReader::open_shapefile(&path, encoding.as_deref(), bbox, batch_size).map(new_reader)
}

/// Opens one GeoPackage layer for batched reading (`layer` as `read_gpkg_file`; `columns`
/// restricts the attributes; `bbox` uses the layer's R-tree when it has one).
#[rustler::nif(schedule = "DirtyIo")]
fn open_gpkg_reader(
    path: String,
    layer: Option<String>,
    columns: Option<Vec<String>>,
    bbox: Option<(f64, f64, f64, f64)>,
    batch_size: usize,
) -> Result<Reader, String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    BatchReader::open_gpkg(
        &path,
        layer.as_deref(),
        columns.as_deref(),
        bbox,
        batch_size,
    )
    .map(new_reader)
}

/// Opens a GeoJSON file for batched reading (`bbox` is applied feature by feature).
#[rustler::nif(schedule = "DirtyIo")]
fn open_geojson_reader(
    path: String,
    bbox: Option<(f64, f64, f64, f64)>,
    batch_size: usize,
) -> Result<Reader, String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    BatchReader::open_geojson(&path, bbox, batch_size).map(new_reader)
}

type NextBatch<'a> = Option<(Handle, Option<String>, Binary<'a>, Secondary)>;

/// The next batch as `{geometry, geometry_column | nil, attributes_ipc, secondary}`, or `nil`
/// at the end of the file (or after `close_reader`).
#[rustler::nif(schedule = "DirtyIo")]
fn next_batch<'a>(env: Env<'a>, reader: Reader) -> Result<NextBatch<'a>, String> {
    let mut guard = reader
        .0
        .lock()
        .map_err(|_| "the reader is poisoned".to_string())?;
    let Some(open) = guard.as_mut() else {
        return Ok(None);
    };
    let Some(batch) = open.next_batch()? else {
        return Ok(None);
    };
    let ipc = binary(env, &batch.attributes_ipc);
    let secondary = batch
        .secondary_geometries
        .into_iter()
        .map(|(name, column)| (name, wrap(column)))
        .collect();
    Ok(Some((
        wrap(batch.geometry),
        batch.geometry_column,
        ipc,
        secondary,
    )))
}

/// Drops the first `count` rows the reader would yield, so the next batch starts there
/// (GeoParquet skips row groups without reading them). Call it before the first `next_batch`.
#[rustler::nif(schedule = "DirtyIo")]
fn skip_rows(reader: Reader, count: usize) -> Result<rustler::Atom, String> {
    let mut guard = reader
        .0
        .lock()
        .map_err(|_| "the reader is poisoned".to_string())?;
    if let Some(open) = guard.as_mut() {
        open.skip_rows(count)?;
    }
    Ok(ok())
}

/// Releases the file now; later `next_batch` calls return `nil`. Idempotent.
#[rustler::nif]
fn close_reader(reader: Reader) -> rustler::Atom {
    if let Ok(mut guard) = reader.0.lock() {
        guard.take();
    }
    ok()
}

/// How many batch readers are open in this VM (a diagnostic used by the tests).
#[rustler::nif]
fn open_readers() -> usize {
    open_batch_readers()
}

rustler::init!("Elixir.GeoFrame.Native");
