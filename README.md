# GeoFrame

GeoPandas-style geospatial dataframes for Elixir, powered by
[GeoArrow](https://geoarrow.org) and Rust.

> Status: early development (Phase 0 spike). APIs will change.

## Why

Elixir has [Explorer](https://github.com/elixir-explorer/explorer) for dataframes, but no
way to carry geometry with spatial semantics: no GeoArrow, no GeoParquet, no spatial joins.
GeoFrame fills that gap without forking Explorer:

- **Attributes** live in an `Explorer.DataFrame`.
- **Geometry** lives in Rust as a GeoArrow column behind a Rustler resource, so chained
  spatial operations never round-trip through Elixir terms.
- **Algorithms** come from the [GeoRust](https://georust.org) `geo` crate via
  [geoarrow-rs](https://github.com/geoarrow/geoarrow-rs).
- **Livebook** integration through [Kino](https://github.com/livebook-dev/kino).

## Architecture

```
%GeoFrame.GeoDataFrame{
  data: Explorer.DataFrame,   # attribute columns (Polars)
  geometry: GeoFrame.GeoSeries,  # GeoArrow column held in Rust
  crs: "EPSG:4326"
}
```

Row-changing operations (filter, sort, slice, join) compute row indices once and apply
`take` to both the attributes and the geometry, keeping them aligned.

## File formats

| Format | Read | Write | Extensions |
|--------|------|-------|------------|
| GeoParquet 1.0/1.1 | `GeoFrame.read_parquet/2` (`bbox:` filter) | `GeoFrame.write_parquet/3` (compression, row groups, native GeoArrow, covering bbox) | `.parquet`, `.geoparquet` |
| GeoJSON (RFC 7946) | `GeoFrame.read_geojson/2` | `GeoFrame.write_geojson/3` | `.geojson`, `.json` |
| FlatGeobuf | `GeoFrame.read_flatgeobuf/2` (optional `bbox:` via the index) | `GeoFrame.write_flatgeobuf/3` | `.fgb` |
| Feather v2 (Arrow IPC file) | `GeoFrame.read_feather/2` | `GeoFrame.write_feather/3` | `.feather`, `.arrow` |
| Shapefile (`.cpg` encodings, `.prj` CRS, zipped) | `GeoFrame.read_shapefile/2` | `GeoFrame.write_shapefile/3` (GDAL-style field-name truncation) | `.shp`, `.zip` |
| GeoPackage (several layers, rtree index) | `GeoFrame.read_gpkg/2` (`layer:`) | `GeoFrame.write_gpkg/3` (`layer:`, `mode: :w \| :a`, `spatial_index:`) | `.gpkg` |
| PostGIS (optional `:postgrex`) | `GeoFrame.read_postgis/3` | `GeoFrame.to_postgis/4` (`if_exists`, `schema`, `spatial_index`) | n/a (database) |
| Arrow IPC stream (binary) | `GeoSeries.from_arrow/2`, `GeoDataFrame.from_arrow/2` | `GeoSeries.to_arrow/2`, `GeoDataFrame.to_arrow/2` | n/a (in memory) |

`GeoFrame.read_file/2` and `GeoFrame.write_file/3` choose the format from the extension or
a `driver:` option. No format needs GDAL: all are pure Rust except GeoPackage, which is SQLite
through `rusqlite` with the **bundled** SQLite amalgamation (the only C code GeoFrame
compiles, built by `cc` together with the NIF; no system library is required). GeoJSON writes
need a lon/lat CRS unless `force: true`; FlatGeobuf with its spatial index stores rows in
Hilbert order (`index: false` keeps row order). A shapefile holds one geometry type and field
names of at most 10 bytes (longer names are truncated exactly like GDAL, with a logged
warning); `GeoFrame.list_layers/2` and `GeoFrame.read_file_info/2` describe a file (all
layers of a GeoPackage, pyogrio-style info). Files written by GeoPandas 1.2 / pyogrio read
directly, and GeoPandas reads what GeoFrame writes (`scripts/crosscheck_io.sh`).

Arrow interchange is IPC bytes (the BEAM has no Arrow C Data Interface binding), with GeoArrow
extension metadata (`geoarrow.wkb` or native types, CRS as PROJJSON) on the geometry field, so
GeoPandas, pyarrow, Explorer and ADBC read what GeoFrame writes and the reverse. Feather and
GeoParquet files written by GeoPandas 1.2 read directly (LZ4/Zstd/Snappy/gzip/brotli, WKB and
native encodings, covering bbox).

### PostGIS

Add `{:postgrex, "~> 0.22"}` and start the connection with the GeoFrame types so `geometry`
columns decode to raw EWKB (parsed in Rust, SRID becomes the CRS):

```elixir
{:ok, conn} = Postgrex.start_link(hostname: "localhost", database: "gis", types: GeoFrame.Postgrex.Types)
{:ok, gdf} = GeoFrame.read_postgis(conn, "SELECT id, geom FROM places WHERE id > $1", params: [10])
:ok = GeoFrame.to_postgis(gdf, conn, "places_copy", if_exists: :replace, spatial_index: true)
```

Without the types module, select `ST_AsEWKB(geom) AS geom`. Tests tagged `:postgis` need
`GEOFRAME_POSTGIS_URL` (see Development).

## Limits

Inputs that would abort the whole VM (stack overflow or a huge allocation) are rejected with an
error (`{:error, message}` or `ArgumentError`) instead. The caps:

| Area | Limit |
|------|-------|
| WKT text | parenthesis nesting depth 64 |
| WKB / EWKB | collection nesting depth 64 (the `wkb` reader itself stops at 128) |
| FlatGeobuf read | every feature must fit in the file; `bbox:` reads need index node size 16 |
| `make_grid` | 10,000,000 cells (square and hexagon) |
| `sample_points` | 10,000,000 samples in total per call (a scalar size counts once per row) |
| `maximum_inscribed_circle` | seed cells are enlarged until at most 5,000,000 fit; refinement is also capped at 5,000,000 cells |
| `segmentize` | 10,000,000 estimated output vertices |
| `hausdorff_distance` / `frechet_distance` `densify:` | fraction at least `1.0e-4` (10,000 pieces per segment) and 10,000,000 densified vertices in total |
| `union_groups` | `n_groups` at most the number of ids; ids below `n_groups` |
| `offset_curve` | `quad_segs` at most 10,000; an estimated 10,000,000 raw vertices (vertices times `2 * quad_segs`) is an error; a geometry whose cutting needs over 50,000,000 segment pairs is `nil`; non-finite distance or coordinates give `nil` |
| `snap` | vertices times snap points at most 50,000,000 per geometry pair (a row over it is `nil`) |
| `polygonize` / `build_area` | the noding examines at most 50,000,000 candidate segment pairs (an error beyond that) |

Writers (GeoParquet, Feather, GeoJSON, FlatGeobuf) write to `<path>.tmp-<id>` beside the
destination and rename it over `path` on success; a failure never removes an existing file.
CRS labels that differ only by case, or `OGC:CRS84` against `EPSG:4326`, are compatible for
binary operations, joins, overlay and `concat`.

Shapefile and zip input/output:

- Zip archives (zipped Shapefiles): a member may decompress to at most 2 GiB (`MAX_ZIP_MEMBER_BYTES`); larger members are an error.
- dBase tables (`.dbf`): a table may expand to at most 2^28 cells (records times fields), and its header must be consistent
  (record length covers the fields, header length covers the descriptors); inconsistent or amplifying headers are an error.
- Shapefile sidecars are named after the path: a path ending in `.shp` swaps that suffix, any other path gets the extension
  appended to its full name (`out.v1` writes `out.v1.shp`, `out.v1.dbf`, ...). Shapefile and zip writes go through temporary
  files and renames, so a failed write leaves existing files as they were.

## Roadmap

| Phase | Scope |
|---|---|
| 0 | Spike: GeoSeries in Rust, measures, GeoParquet read, Livebook demo |
| 1 | GeoSeries: constructors, WKB/WKT, predicates, unary ops, CRS metadata |
| 2 | GeoDataFrame: Explorer alignment, GeoParquet/GeoJSON/FlatGeobuf IO |
| 3 | Spatial index, `sjoin`, `sjoin_nearest`, `overlay`, `dissolve` |
| 4 | Reprojection (`to_crs`) |
| 5 | Kino: deck.gl map layer fed with GeoArrow buffers, Smart Cells |

## Livebook

With [Kino](https://github.com/livebook-dev/kino) installed, a `GeoFrame.GeoDataFrame` renders
in Livebook with **Map**, **Table** and **Inspect** tabs (a `GeoFrame.GeoSeries` with **Map**
and **Inspect**). The map needs the optional `kino_maplibre` package and draws up to 50,000
features; see `GeoFrame.Kino`. Try the walkthrough in [`notebooks/spike.livemd`](notebooks/spike.livemd).

Livebook has no headless runner, so `elixir scripts/run_livemd.exs notebooks/spike.livemd`
extracts the notebook's code cells and runs them in order to check they still work.

## Development

Requires Elixir 1.19 / OTP 28 (see `.tool-versions`) and a Rust toolchain.

```sh
mix deps.get
mix test
cargo test --workspace
```

PostGIS tests are excluded unless a database is given:

```sh
docker run -d --name geoframe-postgis -e POSTGRES_PASSWORD=geoframe -p 55432:5432 postgis/postgis:17-3.5
GEOFRAME_POSTGIS_URL=postgres://postgres:geoframe@127.0.0.1:55432/postgres mix test --only postgis
```

## License

Apache-2.0. See [LICENSE](LICENSE).
