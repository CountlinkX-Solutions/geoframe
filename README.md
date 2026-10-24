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
| GeoParquet 1.0/1.1 (several geometry columns) | `GeoFrame.read_parquet/2` (`bbox:` filter) | `GeoFrame.write_parquet/3` (compression, row groups, native GeoArrow, covering bbox) | `.parquet`, `.geoparquet` |
| GeoJSON (RFC 7946) | `GeoFrame.read_geojson/2` | `GeoFrame.write_geojson/3` | `.geojson`, `.json` |
| FlatGeobuf | `GeoFrame.read_flatgeobuf/2` (optional `bbox:` via the index) | `GeoFrame.write_flatgeobuf/3` | `.fgb` |
| Feather v2 (Arrow IPC file, several geometry columns) | `GeoFrame.read_feather/2` | `GeoFrame.write_feather/3` | `.feather`, `.arrow` |
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

A frame can hold several geometry columns (one active, the others secondary `GeoSeries` with
their own CRS). GeoParquet, Feather and Arrow keep all of them; GeoPackage, GeoJSON, FlatGeobuf,
Shapefile and PostGIS hold one geometry per feature and raise for such a frame, as GeoPandas
`to_file` does (drop the extra columns or convert them to WKT). `clip`, `overlay`, `sjoin`,
`sjoin_nearest` and `dissolve` carry the secondary columns through like GeoPandas.

Z and M survive every format (GeoJSON keeps Z only, as RFC 7946 has no M). GeoJSON writes Z as
the third ordinate and reads positions with three values as Z; FlatGeobuf sets the header
`has_z`/`has_m` flags and stores one Z/M value per vertex; a shapefile with Z is written as
`PointZ`/`PolyLineZ`/`PolygonZ`/`MultiPointZ` and one with only M as the M types (measures that
are NaN are no data); a GeoPackage layer gets `z`/`m` flags of 0, 1 or 2 and Z/M ranges in the
blob envelopes; GeoParquet and Arrow list `"Point Z"`-style `geometry_types` and keep Z/M in
native GeoArrow columns too. Formats that store one dimension per file (FlatGeobuf, Shapefile,
native GeoArrow) promote rows with fewer ordinates with NaN, except native GeoArrow, which
needs one dimension. See [docs/parity.md](docs/parity.md#z-and-m).

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

Without the types module, select `ST_AsEWKB(geom) AS geom`.

Attribute columns map to exact dtypes: `numeric` becomes an Explorer `{:decimal, p, s}` series
(no float rounding, so a `numeric(20, 0)` holding a u64 survives; `NaN` / `Infinity` force
float64 and values wider than 38 digits are read as exact text), and a result with no rows,
or an all-`NULL` column, keeps the dtype of its Postgres type. Intervals, ranges, `inet`,
`uuid` and geometric types are read as text. Duplicate column names are an error (alias them).
Tests tagged `:postgis` need
`GEOFRAME_POSTGIS_URL` (see Development).

## Z and M coordinates

Geometries can be XY, XYZ, XYM or XYZM (`POINT Z`, `LINESTRING M`, `POLYGON ZM`, ...). A
column is stored as GeoArrow `geometry`, whose union has a child per dimension, so rows of one
column may differ; there is no side buffer for Z.

```elixir
pts = GeoFrame.GeoSeries.from_xy([1, 2], [3, 4], z: [10, 20])   # POINT Z (1 3 10), ...
GeoFrame.GeoSeries.has_z(pts)                                   # [true, true]
GeoFrame.GeoSeries.get_coordinates(pts, include_z: true)         # x, y, z columns
GeoFrame.GeoSeries.force_2d(pts)                                # drops Z and M
```

* **API**: `has_z`, `has_m`, `z`/`m`, `force_2d`, `force_3d`, `from_xy(z:)` and
  `get_coordinates(include_z:, include_m:)` (NaN for a vertex without the ordinate).
* **Operations**: vertex copies and maps keep Z (`translate`, `scale`, `rotate`, `to_crs`,
  `simplify`, `reverse`, `boundary`, ...); `segmentize`, `interpolate` and the overlay family
  interpolate it as GEOS does; M survives only pure vertex copies (`reverse`, `normalize`,
  `boundary`, `remove_repeated_points`, ...), as in Shapely. Operations that build new
  geometry from scratch (`buffer`, `centroid`, `envelope`, `voronoi_polygons`) return 2D.
* **I/O**: WKT, WKB, EWKB, GeoParquet, Feather/Arrow, GeoPackage, FlatGeobuf and Shapefile keep
  Z and M; GeoJSON keeps Z only (RFC 7946); see the table above.
* **Livebook**: the map draws the 2D projection; Inspect and Table show `Z`/`M` in WKT.

Every claim is checked against GeoPandas 1.2 / Shapely 2.2 fixtures; deviations are listed in
[docs/parity.md](docs/parity.md#z-and-m).

## Limits

Inputs that would abort the whole VM (stack overflow or a huge allocation) are rejected with an
error (`{:error, message}` or `ArgumentError`) instead. The caps:

| Area | Limit |
|------|-------|
| WKT text | parenthesis nesting depth 64 |
| WKB / EWKB | collection nesting depth 64 (the `wkb` reader itself stops at 128) |
| FlatGeobuf read | every feature must fit in the file; `bbox:` reads need index node size 16; a header that declares Z or M requires one value per vertex in every feature |
| Shapefile read (Z/M) | the part and point counts of a Z or M record must fit the record, so a crafted count is an error, not an allocation |
| `make_grid` | 10,000,000 cells (square and hexagon) |
| `sample_points` | 10,000,000 samples in total per call (a scalar size counts once per row) |
| `maximum_inscribed_circle` | seed cells are enlarged until at most 5,000,000 fit; refinement is also capped at 5,000,000 cells |
| `segmentize` | 10,000,000 estimated output vertices |
| `hausdorff_distance` / `frechet_distance` `densify:` | fraction at least `1.0e-4` (10,000 pieces per segment) and 10,000,000 densified vertices in total |
| `union_groups` | `n_groups` at most the number of ids; ids below `n_groups` |
| `offset_curve` | `quad_segs` at most 10,000; an estimated 10,000,000 raw vertices (vertices times `2 * quad_segs`) is an error; a geometry whose cutting needs over 50,000,000 segment pairs is `nil`; non-finite distance or coordinates give `nil` |
| `snap` | vertices times snap points at most 50,000,000 per geometry pair (a row over it is `nil`) |
| `polygonize` / `build_area` | the noding examines at most 50,000,000 candidate segment pairs (an error beyond that) |
| `is_valid_coverage` / `invalid_coverage_edges` | candidate segment pairs plus point-in-polygon steps at most 50,000,000 per call (an error beyond that); `gap_width` must be finite; rows with non-finite coordinates are `nil` |
| `simplify_coverage` | at most 10,000,000 vertices and 50,000,000 vertex-in-triangle checks (an error beyond that); `tolerance` must be finite; rows with non-finite coordinates are `nil` |

Writers (GeoParquet, Feather, GeoJSON, FlatGeobuf) write to `<path>.tmp-<id>` beside the
destination and rename it over `path` on success; a failure never removes an existing file.
CRS labels that differ only by case, or `OGC:CRS84` against `EPSG:4326`, are compatible for
binary operations, joins, overlay and `concat`.

`to_crs` / `set_crs` take any EPSG geographic, geocentric, projected or compound CRS (the
`crs-definitions` table plus a 51 KB supplementary table generated from the PROJ database), or a
PROJJSON or WKT (WKT1, WKT2) definition without a code, as `pyproj.CRS.from_user_input` does.
A definition is converted for the common projection methods; any other method is an error that
names it. See [CRS definitions and codes outside the table](docs/parity.md#crs-definitions-and-codes-outside-the-table).

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
features and 500,000 vertices (GeometryCollections member by member); the Table tab shows at
most 100,000 rows and 250,000 vertices, with a notice when rows are left out. All caps are
options or `config :geoframe` keys; see `GeoFrame.Kino`. Try the walkthrough in [`notebooks/spike.livemd`](notebooks/spike.livemd).

For large inputs use `GeoFrame.Kino.deck/2`: it sends binary buffers to deck.gl (no GeoJSON),
so a million features stay interactive, with hover tooltips and colour or size by column;
it is capped by `:max_features` and `:max_vertices` (see [`notebooks/deck.livemd`](notebooks/deck.livemd)).

[`notebooks/tour.livemd`](notebooks/tour.livemd) is a longer tour on one story (Europe): every file format,
reprojection, buffers, spatial joins, overlay, dissolve, simplification, distance measures and the
coverage tools, each ending in a table or a deck.gl map.

Two Smart Cells are registered when the `:geoframe` application starts with Kino present. Add
them from the **Smart** menu of a Livebook section:

* **Read spatial file** (`GeoFrame.Kino.ReadFileCell`): path, driver (auto from the extension),
  GeoPackage layer, bounding box, target CRS and an optional map; generates `GeoFrame.read_file!/2`.
* **Spatial join** (`GeoFrame.Kino.SpatialJoinCell`): pick two `GeoDataFrame` variables of the
  notebook, a predicate or nearest mode, `how` and an optional map; generates `GeoFrame.sjoin/3`
  or `GeoFrame.sjoin_nearest/3`.

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

The EPSG definitions and datum operations GeoFrame embeds come from the EPSG Geodetic Parameter
Dataset (IOGP), through the `crs-definitions` crate and tables generated from the PROJ database;
see [NOTICE](NOTICE) for the attribution and the EPSG terms of use.
