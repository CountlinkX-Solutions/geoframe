# GeoFrame

GeoPandas-style geospatial dataframes for Elixir, powered by
[GeoArrow](https://geoarrow.org) and Rust.

> Status: latest release `0.1.2`, distributed from GitHub (not on Hex yet); see the
> [changelog](CHANGELOG.md). Behaviour is checked against GeoPandas 1.2 / Shapely 2.2 on
> 18 864 reference cases, but the API may still change before 1.0.

Elixir has [Explorer](https://github.com/elixir-explorer/explorer) for dataframes, but no way
to carry geometry with spatial semantics. GeoFrame adds it without forking Explorer:

- **Attributes** live in an `Explorer.DataFrame`.
- **Geometry** lives in Rust as a GeoArrow column behind a Rustler resource, so chained
  spatial operations never round-trip through Elixir terms.
- **Algorithms** come from the [GeoRust](https://georust.org) `geo` crate, `proj4rs` and
  pure-Rust ports of the GEOS algorithms GeoPandas relies on. No GDAL, GEOS or PROJ install.
- **Livebook** integration through [Kino](https://github.com/livebook-dev/kino).

```
%GeoFrame.GeoDataFrame{
  data: Explorer.DataFrame,      # attribute columns (Polars)
  geometry: GeoFrame.GeoSeries,  # active GeoArrow column held in Rust
  secondary_geometries: [...],   # more geometry columns, each with its own CRS
  geometry_column: "geometry"
}
```

Row-changing operations (filter, sort, slice, join) compute row indices once and apply `take`
to both the attributes and the geometry, so they never drift apart.

## Install

```elixir
def deps do
  [
    {:geoframe, github: "CountlinkX-Solutions/geoframe", tag: "v0.1.2"},
    # optional
    {:kino, "~> 0.19"},           # Livebook rendering, deck.gl widget, Smart Cells
    {:kino_maplibre, "~> 0.1"},   # the Map tab (the deck.gl widget needs nothing extra)
    {:postgrex, "~> 0.22"},       # PostGIS
    {:req, "~> 0.5"}              # only for the notebooks (they download sample data)
  ]
end
```

GeoFrame is not on Hex yet, so depend on the `v0.1.2` git tag as above (or use a `path:`
dependency on a checkout). The NIF is
compiled on your machine by [Rustler](https://hex.pm/packages/rustler) (a dependency of
GeoFrame; you do not add it), so you need a stable Rust toolchain. Elixir 1.17 or later;
development uses Elixir 1.19 / OTP 28.

## Quickstart

```elixir
alias Explorer.DataFrame
alias GeoFrame.{GeoDataFrame, GeoSeries}

zones =
  GeoDataFrame.new(
    DataFrame.new(zone: ["downtown", "beach"]),
    GeoSeries.from_wkt!(
      [
        "POLYGON ((-80.21 25.75, -80.18 25.75, -80.18 25.78, -80.21 25.78, -80.21 25.75))",
        "POLYGON ((-80.14 25.76, -80.12 25.76, -80.12 25.80, -80.14 25.80, -80.14 25.76))"
      ],
      crs: "EPSG:4326"
    )
  )

places =
  GeoDataFrame.new(
    DataFrame.new(name: ["cafe", "pier", "park"]),
    GeoSeries.from_xy([-80.19, -80.13, -80.0], [25.76, 25.79, 25.9], crs: "EPSG:4326")
  )

# Which zone is each place in? A left join keeps "park", which is in none.
joined = GeoFrame.sjoin(places, zones, how: :left, predicate: :within)

# Reproject to UTM 17N and measure in square metres.
zones |> GeoDataFrame.to_crs("EPSG:32617") |> GeoDataFrame.area() |> Explorer.Series.to_list()

# Write and read any supported format.
:ok = GeoFrame.write_file(zones, "zones.gpkg")
zones = GeoFrame.read_file!("zones.gpkg")
```

## What it does

Every operation GeoFrame shares with GeoPandas is tested against it; the member-by-member
matrix and the known deviations are in [docs/parity.md](docs/parity.md).

### Reading and writing

| Format | Read | Write | Extensions |
|--------|------|-------|------------|
| GeoParquet 1.0/1.1 | `read_parquet/2` (`bbox:` filter) | `write_parquet/3` (compression, row groups, native GeoArrow, covering bbox) | `.parquet`, `.geoparquet` |
| GeoJSON (RFC 7946) | `read_geojson/2` | `write_geojson/3` | `.geojson`, `.json` |
| FlatGeobuf | `read_flatgeobuf/2` (`bbox:` via the index) | `write_flatgeobuf/3` | `.fgb` |
| Feather v2 (Arrow IPC file) | `read_feather/2` | `write_feather/3` | `.feather`, `.arrow` |
| Shapefile (`.cpg`, `.prj`, zipped) | `read_shapefile/2` | `write_shapefile/3` | `.shp`, `.zip` |
| GeoPackage (several layers, rtree) | `read_gpkg/2` (`layer:`) | `write_gpkg/3` (`layer:`, `mode:`) | `.gpkg` |
| PostGIS (optional `:postgrex`) | `read_postgis/3` | `to_postgis/4` | database |
| Arrow IPC stream (binary) | `from_arrow/2` | `to_arrow/2` (on `GeoSeries` and `GeoDataFrame`) | in memory |

- `read_file/2` and `write_file/3` pick the format from the extension or a `driver:` option;
  `list_layers/2` and `read_file_info/2` describe a file. Files written by GeoPandas 1.2 /
  pyogrio read directly, and GeoPandas reads what GeoFrame writes.
- Pure Rust, except GeoPackage, which is SQLite through `rusqlite` with the bundled SQLite
  amalgamation (the only C code GeoFrame compiles).
- **Several geometry columns**: a frame has one active geometry and any number of secondary
  `GeoSeries` columns (`put_geometry/3`, `set_geometry/3`, `geometry_columns/1`). GeoParquet,
  Feather and Arrow keep all of them; the single-geometry formats raise, as GeoPandas
  `to_file` does.
- **Z and M**: geometries can be XY, XYZ, XYM or XYZM (`has_z/1`, `z/1`, `force_2d/1`,
  `from_xy(z:)`, `get_coordinates(include_z:)`). Every format keeps them (GeoJSON keeps Z only,
  as RFC 7946 has no M). See [Z and M](docs/parity.md#z-and-m).

### Coordinate reference systems

- `set_crs/2`, `to_crs/2` and `estimate_utm_crs/2` work with any EPSG geographic, geocentric,
  projected or compound code, and with PROJJSON or WKT (WKT1, WKT2) definitions that have no
  code, as `pyproj.CRS.from_user_input` does. `to_crs` agrees with pyproj to 1e-6 projected
  units on the reference cases.
- Datum shifts follow PROJ without a network: a grid if one is installed (`GeoFrame.Grids`,
  NTv2 `.gsb` and GeoTIFF `.tif`; nothing is downloaded), else the Helmert operation PROJ picks
  by area of use. See [Datum shifts and grids](docs/parity.md#datum-shifts-and-grids) and
  [CRS definitions and codes outside the table](docs/parity.md#crs-definitions-and-codes-outside-the-table).

### Geometry operations

On `GeoSeries`; the common ones are also on `GeoDataFrame` and `GeoFrame`.

- *Measures*: `area`, `length`, `distance` (planar, geodesic, haversine), `hausdorff_distance`,
  `frechet_distance`, `bounds`, `total_bounds`, `minimum_clearance`, `project`.
- *Predicates*: `intersects`, `contains`, `within`, `touches`, `dwithin`, `relate`, `is_valid`,
  `is_simple`, ... and the coverage validators `is_valid_coverage`, `invalid_coverage_edges`.
- *Constructive*: `buffer`, `offset_curve`, `simplify`, `simplify_coverage`, `convex_hull`,
  `concave_hull`, bounding shapes, `maximum_inscribed_circle`, `delaunay_triangles`,
  `voronoi_polygons`, `polygonize`, `snap`, `segmentize`, `set_precision`, `make_valid`.
- *Set operations*: `union`, `intersection`, `difference`, `symmetric_difference`, `union_all`,
  `clip`, `overlay` (all five `how`s), `explode`, `dissolve`.
- *Joins*: `sjoin` and `sjoin_nearest` (R-tree, every predicate, `dwithin`), `sindex_query`,
  `sindex_nearest`.
- *Transforms*: `translate`, `rotate`, `scale`, `skew`, `affine_transform`, `wrap_longitude`.
- *Grids*: `GeoSeries.make_grid/3` (square and hexagon cells).
- *Explorer verbs on a `GeoDataFrame`*: `select`, `discard`, `rename`, `put`, `pull`,
  `filter_with`, `sort_with`, `mutate_with`, `head`, `tail`, `slice`, `sample`, `distinct`,
  `drop_nil`, `join`, `concat_rows`; the geometry follows the rows.

### Livebook

With Kino installed (see `GeoFrame.Kino`):

- A `GeoDataFrame` renders with **Map**, **Table** and **Inspect** tabs (a `GeoSeries` with
  **Map** and **Inspect**). The map needs `kino_maplibre` and draws up to 50,000 features and
  500,000 vertices; the Table tab shows at most 100,000 rows and 250,000 vertices, with a
  notice when rows are left out. Every cap is an option or a `config :geoframe` key.
- `GeoFrame.Kino.deck/2` sends binary buffers to deck.gl (no GeoJSON), so a million features
  stay interactive, with hover tooltips and colour or size by column.
- Two Smart Cells are registered when Kino is present: **Read spatial file** and
  **Spatial join**, both under **Smart** in a Livebook section.

### PostGIS

```elixir
{:ok, conn} =
  Postgrex.start_link(hostname: "localhost", database: "gis", types: GeoFrame.Postgrex.Types)

{:ok, gdf} = GeoFrame.read_postgis(conn, "SELECT id, geom FROM places WHERE id > $1", params: [10])
:ok = GeoFrame.to_postgis(gdf, conn, "places_copy", if_exists: :replace, spatial_index: true)
```

`geometry` columns decode to EWKB (parsed in Rust; the SRID becomes the CRS) and `numeric`
columns keep every digit. Without the types module, select `ST_AsEWKB(geom) AS geom`. See
`GeoFrame.PostGIS`.

## Notebooks

Open them in [Livebook](https://livebook.dev). They download
[Natural Earth](https://www.naturalearthdata.com/) data on first run, and the maps need
internet access in the browser for the basemap.

| Notebook | What it shows |
|----------|---------------|
| [`notebooks/tour.livemd`](notebooks/tour.livemd) | A tour on one story (Europe): every file format, reprojection, buffers, spatial joins, overlay, dissolve, simplification, distances and the coverage tools. |
| [`notebooks/deck.livemd`](notebooks/deck.livemd) | The deck.gl widget on real data: countries, cities, rivers, colour and size by column, the Smart Cells, one million points. |
| [`notebooks/compare.livemd`](notebooks/compare.livemd) | GeoFrame and GeoPandas side by side: ten analyses on real data (buffer and dissolve, overlay, joins, nearest, clipping, geodesic distances, Voronoi), each in GeoFrame and in an executable Python cell (Pythonx), with a PASS/FAIL table per metric. `scripts/compare_geopandas.sh` repeats the comparison from the command line. |
| [`notebooks/spike.livemd`](notebooks/spike.livemd) | The short first walkthrough: read GeoParquet, measures, reprojection, filtering, maps, Z coordinates, write and read back. |

Livebook has no headless runner, so `elixir scripts/run_livemd.exs notebooks/tour.livemd`
extracts a notebook's code cells and runs them in order. It also runs the Python cells through Pythonx. CI does this for all four.

## Limits and known deviations

- Where GeoPandas returns `False`, `0`, `NaN` or an empty geometry for a null or empty input,
  GeoFrame returns `nil`; a few operations on degenerate or invalid input differ from GEOS.
  Every difference is listed, and tested, in [docs/parity.md](docs/parity.md).
- Inputs that would abort the VM (a stack overflow, a huge allocation) are rejected with an
  error; the caps (WKT depth, grid cells, buffer vertices, ...) are in
  [Resource limits](docs/parity.md#resource-limits).
- Files are read into memory whole; there is no streaming or batched read.
- Not planned: `plot` (use the Kino map), geocoding, pandas index alignment, a persistent
  spatial index object, and OGR drivers that need GDAL (GML, KML, GPX, FileGDB, ...; convert
  with `ogr2ogr` first).

## Development

Requires Elixir 1.19 / OTP 28 (see `.tool-versions`) and a Rust toolchain.

```sh
mix deps.get
mix test                  # Elixir tests, doctests and the GeoPandas reference suite
cargo test --workspace    # Rust tests
mix format --check-formatted && mix credo --strict && mix compile --warnings-as-errors
mix docs                  # ExDoc; CI fails on warnings
```

- **Reference suite**: `test/reference/generate.py` (dev-only, needs GeoPandas 1.2) writes the
  fixtures in `test/reference/fixtures/`, and `test/geo_frame/reference_test.exs` compares
  GeoFrame with them in CI without Python. `REFERENCE_REPORT=1 mix test
  test/geo_frame/reference_test.exs` prints the cases per operation; known differences live in
  `test/reference/known_deviations.exs`. See [test/reference/README.md](test/reference/README.md).
- **I/O cross-check**: `scripts/crosscheck_io.sh` writes files with GeoFrame and reads them
  back in GeoPandas; `scripts/crosscheck_postgis.sh` does the same for PostGIS.
- **PostGIS tests** are excluded unless a database is given:

  ```sh
  docker run -d --name geoframe-postgis -e POSTGRES_PASSWORD=geoframe -p 55432:5432 postgis/postgis:17-3.5
  GEOFRAME_POSTGIS_URL=postgres://postgres:geoframe@127.0.0.1:55432/postgres mix test --only postgis
  ```

- **Benchmarks**: `mix run scripts/bench_sjoin.exs` times large spatial joins.

## License

Apache-2.0. See [LICENSE](LICENSE).

The EPSG definitions and datum operations GeoFrame embeds come from the EPSG Geodetic Parameter
Dataset (IOGP), through the `crs-definitions` crate and tables generated from the PROJ database;
see [NOTICE](NOTICE) for the attribution and the EPSG terms of use.
