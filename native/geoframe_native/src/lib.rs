//! Thin Rustler wrapper around `geoframe_core`. All logic and its tests live in the core
//! crate; this layer only converts between BEAM terms and core types.

use geoframe_core::{
    from_arrow_ipc, read_feather, read_flatgeobuf, read_geojson, read_geojson_str,
    read_geoparquet_filtered, to_arrow_ipc, write_feather, write_flatgeobuf, write_geojson,
    write_geoparquet_with, BufferOptions, FeatherCompression, GeoColumn, GeometryEncoding,
    GridFeature, GridType, NearestPairs, Origin, ParquetOptions, Predicate, QueryPredicate, SetOp,
    TableRead, UnaryOp, VectorRead,
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
fn area_geodesic(column: Handle) -> Result<Vec<Option<f64>>, String> {
    column.0.area_geodesic()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn length_geodesic(column: Handle) -> Result<Vec<Option<f64>>, String> {
    column.0.length_geodesic()
}

#[rustler::nif(schedule = "DirtyCpu")]
fn distance_geodesic(
    left: Handle,
    right: Handle,
    haversine: bool,
) -> Result<Vec<Option<f64>>, String> {
    left.0.distance_geodesic(&right.0, haversine)
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
fn sindex_nearest(
    left: Handle,
    right: Handle,
    max_distance: Option<f64>,
    exclusive: bool,
) -> Result<NearestPairs, String> {
    left.0.nearest_pairs(&right.0, max_distance, exclusive)
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
fn to_crs(column: Handle, target: String) -> Result<Handle, String> {
    column.0.to_crs(&target).map(wrap)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn estimate_utm_crs(column: Handle) -> Result<String, String> {
    column.0.estimate_utm_crs()
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

/// Reads a GeoParquet file. Returns `{geometry, geometry_column, attributes_ipc}` where the
/// attributes are an Arrow IPC stream (one copy; Explorer has no zero-copy NIF access).
/// `bbox` keeps only the rows whose bounding box intersects it.
#[rustler::nif(schedule = "DirtyIo")]
fn read_parquet<'a>(
    env: Env<'a>,
    path: String,
    columns: Option<Vec<String>>,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Result<(Handle, String, Binary<'a>), String> {
    let bbox = bbox.map(|(a, b, c, d)| [a, b, c, d]);
    let read = read_geoparquet_filtered(&path, columns.as_deref(), bbox)?;
    let ipc = binary(env, &read.attributes_ipc);
    Ok((wrap(read.geometry), read.geometry_column, ipc))
}

/// `options` is `{compression, compression_level, row_group_size, geometry_encoding,
/// covering_bbox, schema_version}`.
#[rustler::nif(schedule = "DirtyIo")]
fn write_parquet(
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    path: String,
    options: (String, Option<i32>, Option<usize>, String, bool, String),
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
    write_geoparquet_with(
        &path,
        &column.0,
        &geometry_column,
        attributes_ipc.as_slice(),
        &options,
    )
}

fn table_read<'a>(env: Env<'a>, read: TableRead) -> (Handle, String, Binary<'a>) {
    let ipc = binary(env, &read.attributes_ipc);
    (wrap(read.geometry), read.geometry_column, ipc)
}

/// Reads a Feather v2 file. Returns `{geometry, geometry_column, attributes_ipc}`.
#[rustler::nif(schedule = "DirtyIo")]
fn read_feather_file<'a>(
    env: Env<'a>,
    path: String,
    columns: Option<Vec<String>>,
    geometry_column: Option<String>,
) -> Result<(Handle, String, Binary<'a>), String> {
    read_feather(&path, columns.as_deref(), geometry_column.as_deref())
        .map(|read| table_read(env, read))
}

#[rustler::nif(schedule = "DirtyIo")]
fn write_feather_file(
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    path: String,
    geometry_encoding: String,
    compression: String,
) -> Result<(), String> {
    write_feather(
        &path,
        &column.0,
        &geometry_column,
        attributes_ipc.as_slice(),
        GeometryEncoding::parse(&geometry_encoding)?,
        FeatherCompression::parse(&compression)?,
    )
}

/// Encodes geometry plus attributes (empty bytes: none) as an Arrow IPC stream.
#[rustler::nif(schedule = "DirtyCpu")]
fn to_arrow_stream<'a>(
    env: Env<'a>,
    column: Handle,
    geometry_column: String,
    attributes_ipc: Binary,
    geometry_encoding: String,
) -> Result<Binary<'a>, String> {
    let bytes = to_arrow_ipc(
        &column.0,
        &geometry_column,
        attributes_ipc.as_slice(),
        GeometryEncoding::parse(&geometry_encoding)?,
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
) -> Result<(Handle, String, Binary<'a>), String> {
    from_arrow_ipc(
        bytes.as_slice(),
        columns.as_deref(),
        geometry_column.as_deref(),
    )
    .map(|read| table_read(env, read))
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
    let matrix: [f64; 6] = matrix
        .try_into()
        .map_err(|_| "affine_transform needs exactly 6 numbers".to_string())?;
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
fn project(left: Handle, right: Handle, normalized: bool) -> Result<Vec<Option<f64>>, String> {
    left.0.project(&right.0, normalized)
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
) -> Result<Vec<Option<f64>>, String> {
    left.0.hausdorff_distance(&right.0, densify)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn frechet_distance(
    left: Handle,
    right: Handle,
    densify: Option<f64>,
) -> Result<Vec<Option<f64>>, String> {
    left.0.frechet_distance(&right.0, densify)
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
fn minimum_bounding_radius(column: Handle) -> Vec<Option<f64>> {
    column.0.minimum_bounding_radius()
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
fn crs_is_geographic(crs: String) -> Option<bool> {
    geoframe_core::crs_is_geographic(&crs)
}

rustler::init!("Elixir.GeoFrame.Native");
