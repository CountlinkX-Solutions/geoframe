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
| GeoParquet 1.1 | `GeoFrame.read_parquet/2` | `GeoFrame.write_parquet/3` | `.parquet`, `.geoparquet` |
| GeoJSON (RFC 7946) | `GeoFrame.read_geojson/2` | `GeoFrame.write_geojson/3` | `.geojson`, `.json` |
| FlatGeobuf | `GeoFrame.read_flatgeobuf/2` (optional `bbox:` via the index) | `GeoFrame.write_flatgeobuf/3` | `.fgb` |

`GeoFrame.read_file/2` and `GeoFrame.write_file/3` choose the format from the extension or
a `driver:` option. All formats are pure Rust (no GDAL). GeoJSON writes need a lon/lat CRS
unless `force: true`; FlatGeobuf with its spatial index stores rows in Hilbert order
(`index: false` keeps row order). Shapefile is not supported yet.

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

## License

Apache-2.0. See [LICENSE](LICENSE).
