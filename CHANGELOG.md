# Changelog

All notable changes to GeoFrame are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The API may still
change before 1.0.

## [Unreleased]

## [0.2.0] - 2026-11-13

Like 0.1.x, it is distributed through the `v0.2.0` git tag and GitHub release, not through Hex
yet. The release attaches precompiled NIFs for nine targets; they are used once GeoFrame is on
Hex, where no Rust toolchain is needed. A git dependency builds the NIF from source (a stable
Rust toolchain is required).

This release adds batched reads (`stream_file/2`), grouped aggregation (`group_by`, `summarise`,
`group_map`) and precompiled NIFs, closes a set of GeoPandas parity gaps, and makes a few
behaviours stricter or closer to GDAL and GEOS. Five changes need a look in existing code; see
Breaking changes.

### Breaking changes

- **`set_crs/3` no longer overrides an existing CRS.** Setting a different CRS on a series or
  frame that already has one raises `ArgumentError`, as in GeoPandas; the same CRS is a no-op, a
  missing CRS is set, and `nil` still clears. Migration: to relabel on purpose, pass
  `allow_override: true` (`set_crs(gdf, "EPSG:3857", allow_override: true)`); to change
  coordinates, use `to_crs/2`
  ([#131](https://github.com/CountlinkX-Solutions/geoframe/pull/131)).
- **`read_file(bbox:)`, `stream_file(bbox:)` and FlatGeobuf reads use GDAL's semantics: a
  feature is kept when its geometry intersects the box, not when its envelope does.** A diagonal
  `LINESTRING (0 0, 10 10)` is dropped by the box `6 0 9 3`. Shapefile, GeoPackage and GeoJSON
  gain the option with the same rule; GeoParquet keeps its covering-box semantics, as GeoPandas.
  Migration: results can lose rows that only the envelope touched; if you relied on the envelope
  test, filter with `GeoSeries.total_bounds` or an envelope intersection after reading
  ([#141](https://github.com/CountlinkX-Solutions/geoframe/pull/141)).
- **`write_feather/3` and `to_arrow` write 65 536-row record batches by default**, as pyarrow
  and GeoPandas `to_feather`, instead of one record batch per file. Migration: pass
  `record_batch_size:` (a positive integer) to choose another size; a file with a single batch
  needs a size at least as large as the frame. Existing files read the same
  ([#141](https://github.com/CountlinkX-Solutions/geoframe/pull/141)).
- **On a grouped frame, `head`, `tail`, `slice` and `sample` act per group** (Explorer's grouped
  semantics, pandas `groupby.head`), keeping the original row order, with the geometry aligned.
  Migration: call `ungroup/2` first to get the old global behaviour. Ungrouped frames are
  unchanged ([#143](https://github.com/CountlinkX-Solutions/geoframe/pull/143)).
- **`summarise` with `:n_distinct` no longer counts nulls** (pandas `nunique`). Migration: add one
  to the result for groups where you counted the null as a value
  ([#143](https://github.com/CountlinkX-Solutions/geoframe/pull/143)).

### Added

- Precompiled NIFs through `rustler_precompiled`: the release workflow builds
  `aarch64-apple-darwin`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`,
  `x86_64-apple-darwin`, `x86_64-pc-windows-gnu`, `x86_64-pc-windows-msvc`,
  `x86_64-unknown-freebsd`, `x86_64-unknown-linux-gnu` and `x86_64-unknown-linux-musl` (NIF
  version 2.15) on each `v*` tag and attaches them to the release. A Hex package verifies the
  download against a shipped checksum file; `GEOFRAME_BUILD=true` forces a source build
  ([#128](https://github.com/CountlinkX-Solutions/geoframe/pull/128)).
- A git or path dependency builds the NIF from source automatically (no `GEOFRAME_BUILD`
  needed), because a package without the checksum file cannot download a precompiled NIF; Rustler
  is then fetched as a dependency
  ([#145](https://github.com/CountlinkX-Solutions/geoframe/pull/145)).
- `GeoFrame.stream_file/2`: a lazy `Stream` of `GeoDataFrame` batches for GeoParquet,
  FlatGeobuf, Feather / Arrow IPC, Shapefile (`.shp`, `.zip`), GeoPackage and GeoJSON, with
  `batch_size:` (default 65 536), `bbox:`, `columns:` and format options. Concatenating the
  batches equals `read_file/2`; early halts release the reader. On 500 000 points with
  10 000-row batches, peak memory is 155 to 196 MB against 235 to 308 MB for a whole read
  (GeoJSON 72 MB: 175 MB against 637 MB)
  ([#138](https://github.com/CountlinkX-Solutions/geoframe/pull/138),
  [#139](https://github.com/CountlinkX-Solutions/geoframe/pull/139),
  [#140](https://github.com/CountlinkX-Solutions/geoframe/pull/140)).
- `read_file(path, rows: n | first..last)` for every format, as GeoPandas `rows=`: the first `n`
  rows or an inclusive range, applied after `bbox:` and skipping rows cheaply where the format
  allows (for example `rows: 10` on a GeoParquet file: 1 ms against 239 ms)
  ([#140](https://github.com/CountlinkX-Solutions/geoframe/pull/140)).
- `read_file(bbox:)` for Shapefile, GeoPackage and GeoJSON, and `record_batch_size:` on
  `write_feather` / `to_arrow`
  ([#141](https://github.com/CountlinkX-Solutions/geoframe/pull/141)).
- `GeoDataFrame.group_by/2`, `ungroup/2`, `groups/1` and `summarise/3`: attribute aggregations
  (`:sum :mean :median :min :max :count :first :last :n_distinct`) and geometry aggregations
  (`:union_all :intersection_all :collect :envelope :convex_hull :centroid :first :last :count`)
  computed in one NIF call, 8 to 14 times faster than a per-group loop at 100 000 rows and 1 000
  groups. `summarise(group_by(gdf, by), [])` equals `dissolve(gdf, by: by)`
  ([#142](https://github.com/CountlinkX-Solutions/geoframe/pull/142)).
- `GeoDataFrame.group_map/4` (pandas `groupby(by).apply(fun)`) and the top-level `GeoFrame`
  delegates `group_by/2`, `summarise/3`, `ungroup/2` and `group_map/4`
  ([#143](https://github.com/CountlinkX-Solutions/geoframe/pull/143)).
- `sindex_query/4` accepts a list of WKT strings or a single WKT string as input (a single
  geometry returns the ascending tree indices, as GeoPandas) and the `sort:` option;
  `get_geometry/2` takes a per-row index list or series
  ([#129](https://github.com/CountlinkX-Solutions/geoframe/pull/129)).
- `GeoDataFrame.from_features/2` keeps the property column order for ordered input (keyword
  lists, `{key, value}` pairs, `Jason.OrderedObject`); `to_geo_dict/2` with GeoPandas' `na:`,
  `show_bbox:` and `drop_id:`; `GeoDataFrame.to_wkb/2` (`hex:`, `srid:`), `to_wkt/2` and
  `rename_geometry/2`
  ([#130](https://github.com/CountlinkX-Solutions/geoframe/pull/130)).
- `dissolve` accepts `as_index: false` and `observed: false` (unobserved categories become empty
  groups); `as_index: true` and `level:` raise because GeoFrame has no index. The default stays
  `observed: true`
  ([#131](https://github.com/CountlinkX-Solutions/geoframe/pull/131)).
- `frechet_distance` works on every geometry type, as GEOS, not only on `LineString`s
  ([#132](https://github.com/CountlinkX-Solutions/geoframe/pull/132)).
- `buffer/3` takes a per-row distance (a list with `nil`, or a series) and `single_sided:`; lines
  and points match GEOS, polygons return the band, which matches GEOS only for polygons small
  relative to the distance (documented in `docs/parity.md`)
  ([#133](https://github.com/CountlinkX-Solutions/geoframe/pull/133)).
- The vertex constructors (`line_strings_from_vertices`, `polygons_from_rings`, ...) accept
  `{x, y, z}` and `[x, y, z]` vertices as Shapely 2.2
  ([#135](https://github.com/CountlinkX-Solutions/geoframe/pull/135)).
- `GeoFrame.Kino.deck/2`: click popups (`popup: true | [columns]`) and several layers
  (`deck([gdf1, {gdf2, name: ..., color: ...}], opts)`) with a layer-control panel; a single
  frame sends the same payload as before
  ([#136](https://github.com/CountlinkX-Solutions/geoframe/pull/136)).
- Shapefiles and GeoPackages written with one of the 1 217 supplementary EPSG codes keep their
  definition: a WKT1 `.prj` is written and the GeoPackage `definition` is filled, instead of a
  warning and `undefined`. 119 codes have no WKT1 in PROJ (Krovak Modified 5515 and 5516, 3D,
  geocentric and compound CRSs) and behave as before
  ([#144](https://github.com/CountlinkX-Solutions/geoframe/pull/144)).

### Changed

- A missing Z reads as 0, as GDAL: a two-value position in a 3D GeoJSON geometry, and a row
  without Z written to a Shapefile or FlatGeobuf layer with Z (a stored `NaN` Z stays `NaN`)
  ([#135](https://github.com/CountlinkX-Solutions/geoframe/pull/135)).
- `is_valid` is false and `make_valid` repairs a polygon whose interior is disconnected, as GEOS
  (`Interior is disconnected`): a hole touching the shell at two or more points, or a chain or
  cycle of holes cutting the interior. A single touch stays valid. `null_out_invalid` nulls
  these polygons ([#134](https://github.com/CountlinkX-Solutions/geoframe/pull/134)).
- `is_valid_reason` follows GEOS's check order and wording (`Self-intersection[2 2]`,
  `Hole lies outside shell[...]`, `Too few points in geometry component[...]`, and so on)
  ([#132](https://github.com/CountlinkX-Solutions/geoframe/pull/132)).
- `polygonize` and `build_area` with `node: true` and lines crossing polygons match GEOS: no extra
  small face, and the crossing point is a vertex of the merged ring. On a 2 000-case fuzz,
  `polygonize` mismatches fell from 46 to 2 (and 54 to 3 on a second seed), with no case worse
  ([#137](https://github.com/CountlinkX-Solutions/geoframe/pull/137)).

### Documentation

- `docs/parity.md` and the README follow every change above. The reference suite is
  now 19 118 cases: 18 379 agree with GeoPandas / Shapely and 739 are documented deviations,
  none unexplained (0.1.1: 18 134 agree and 730 documented of 18 864).

### Internal

- The release workflow uploads artifacts on pull requests that change it; the `.cargo` config
  links musl cdylibs without static CRT
  ([#128](https://github.com/CountlinkX-Solutions/geoframe/pull/128)).

## [0.1.2] - 2026-11-12

Like 0.1.1, it is distributed through the `v0.1.2` git tag and GitHub release, not through Hex.

CRS handling now follows PROJ more closely. Results change in two places, each to match
pyproj 3.8 / PROJ 9.8: the datum operation chosen for some EPSG codes, and ESRI WKT1 `.prj`
files that were refused or answered without the right datum shift.

### Changed

- The datum operation to WGS 84 is chosen as PROJ chooses it: a null operation listed first
  wins (GDA94 and GDA2020 no longer get a 1.7 m Helmert step), operations whose area is offshore
  come before onshore ones, a datum with one operation applies it everywhere, and `helmert exact`
  operations are applied exactly (Saba, Sint Eustatius, Bonaire). By EPSG code, CRSs more than
  5 cm from pyproj fell from 294 to 95 of 5 934 (GDA94 / GDA2020 1.7 m, RD/83 and PD/83 182 m,
  Saba 9 m now agree). Operations PROJ orders by a projected CRS's own area of use are not
  modelled yet (for example Minna / UTM 12 m, ED50 / UTM up to 1.6 m)
  ([#126](https://github.com/CountlinkX-Solutions/geoframe/pull/126)).
- An ESRI WKT1 `.prj` on a non-Greenwich prime meridian resolves through its EPSG geographic CRS,
  identified by the datum name and the prime meridian, and reprojects like pyproj: NTF (Paris) /
  Lambert zone II, Lisbon, CH1903 (Bern), S-JTSK (Ferro), Batavia and Gunung Segara (Jakarta),
  MGI (Ferro) Austria grids. Text that GeoFrame cannot reproject correctly is refused with a clear
  error instead of answered tens to hundreds of metres off: MGI (Ferro) and Monte Mario (Rome)
  geographic text, and EPSG:5221 written as ESRI text
  ([#126](https://github.com/CountlinkX-Solutions/geoframe/pull/126)).

### Fixed

- ESRI WKT1 projection names that were refused with `unsupported WKT definition` are read, onto
  methods GeoFrame already had: `Gauss_Kruger`, `Albers`, `Cassini`, `Double_Stereographic`
  (RD New, EPSG:28992), polar `Stereographic` variants, Hotine and Rectified Skew Orthomorphic
  variants, Mercator variant B, Krovak with `X_Scale` / `Y_Scale` / `XY_Plane_Rotation`, LCC 2SP
  Michigan, `Equal_Earth`, Lambert azimuthal equal area and a few more. Of 5 330 projected EPSG
  CRSs written as WKT1_ESRI, 36 are still refused (was 1 007), and 30 are more than 5 cm from
  pyproj (was 115) ([#125](https://github.com/CountlinkX-Solutions/geoframe/pull/125),
  [#126](https://github.com/CountlinkX-Solutions/geoframe/pull/126)).

## [0.1.1] - 2026-11-12

Like 0.1.0, it is distributed through the `v0.1.1` git tag and GitHub release, not through Hex.

Results change in three places, each to match GEOS or PROJ: `is_valid` and `make_valid` on
collinear (zero-area) rings, `polygonize` and `build_area` with `node: true`, and `to_crs` on
ESRI WKT1 `.prj` files, which now get the datum shift PROJ applies.

### Added

- `set_crs/2` on `GeoSeries` and `GeoDataFrame` accepts a positive integer EPSG code, like
  `to_crs/2` already did: `set_crs(series, 3857)` stores `"EPSG:3857"`, the same label as the
  string input. A non-positive integer raises `ArgumentError`
  ([#116](https://github.com/CountlinkX-Solutions/geoframe/pull/116)).

### Changed

- A polygon ring whose vertices are all collinear (zero area) is now invalid, as in GEOS:
  `is_valid` is false (reason `Self-intersection[...]` or `Too few points in geometry
  component[...]`) and `make_valid` collapses it to linework (`MULTILINESTRING`, `LINESTRING` or
  `POINT`) instead of returning it unchanged. A valid polygon with a collinear hole, or a
  multipolygon with a collinear part, becomes a `GEOMETRYCOLLECTION` of the polygon and the
  linework ([#117](https://github.com/CountlinkX-Solutions/geoframe/pull/117)). The reference
  suite is now 18 134 agree and 730 documented deviations out of 18 864.
- `polygonize` and `build_area` with `node: true` keep every input vertex that lies on the merged
  outline, as GeoPandas does (`shapely.polygonize([union_all()])`). Two squares sharing an edge
  now give `POLYGON ((0 0, 0 1, 1 1, 2 1, 2 0, 1 0, 0 0))` instead of dropping the collinear
  vertices. On a 400-case fuzz against GEOS, `polygonize` mismatches fell from 232 to 8 and
  `build_area` from 245 to 134, with no case worse
  ([#119](https://github.com/CountlinkX-Solutions/geoframe/pull/119)).
- An ESRI WKT1 `.prj` (`GCS_...` / `D_...` names, no `AUTHORITY`) now gets the datum shift PROJ
  applies, identified by the DATUM name instead of the GEOGCS name: ED50 about 138 m, OSGB36
  about 109 m, Tokyo about 463 m, NAD27 UTM about 20 m. Before, `to_crs/2` returned the unshifted
  coordinates for that text. Against pyproj on 1 110 CRSs written as WKT1_ESRI, mismatches over
  5 cm fell from 716 to 60, with no case worse. GDA94, GDA2020 and CR-SIRGAS are not covered, and
  a definition with a non-Greenwich prime meridian (Ferro, Paris, Rome, ...) keeps its previous
  result ([#122](https://github.com/CountlinkX-Solutions/geoframe/pull/122)).

### Fixed

- An ESRI WKT1 `.prj` for a StatePlane CRS (for example ESRI:102649, Arizona Central in US
  survey feet) is now read by `to_crs/2`, `set_crs/2` and `GeoFrame.Kino.deck/2` instead of
  raising `unsupported WKT definition`. WKT1 parameter names are matched without regard to
  case (ESRI writes `False_Easting`, `Standard_Parallel_1`), and ESRI's
  `Lambert_Conformal_Conic` is read as the 2SP method when it has a second standard parallel, and
  as the 1SP method (with `Standard_Parallel_1` equal to `Latitude_Of_Origin`, as ESRI writes it)
  otherwise; a lone standard parallel that differs from the origin is still refused.
  `Foot_US` and `Foot` units are honoured. Reprojection matches pyproj
  ([#110](https://github.com/CountlinkX-Solutions/geoframe/issues/110)).
- Mixed-dimension geometry collections (XY, XYZ, XYM and XYZM members together) no longer print
  GeoArrow panic messages on stderr. The error returned is the same and behaviour is unchanged
  ([#120](https://github.com/CountlinkX-Solutions/geoframe/pull/120)).

### Documentation

- The description of GEOS `set_precision` on invalid multipolygons in `docs/parity.md` is
  corrected: GEOS keeps a part whose edges meet no other part's, merges parts with coinciding
  edges, and raises `TopologyException` on crossing edges
  ([#118](https://github.com/CountlinkX-Solutions/geoframe/pull/118)).

### Internal

- CI caches the compiled NIF together with `_build`
  ([#113](https://github.com/CountlinkX-Solutions/geoframe/pull/113)).
- The linear-referencing property test no longer asserts that `project` returns the earliest
  pass on overlapping lines, which GEOS does not guarantee either
  ([#121](https://github.com/CountlinkX-Solutions/geoframe/pull/121)).

## [0.1.0] - 2026-11-12

First tagged release. It is distributed through the `v0.1.0` git tag and GitHub release, not
through Hex. The NIF is compiled on your machine, so you need a stable Rust toolchain; Elixir
1.17 or later.

### Added

- **Core**: `GeoFrame.GeoSeries`, a GeoArrow geometry column held in Rust behind a Rustler
  resource, and `GeoFrame.GeoDataFrame`, an Explorer `DataFrame` of attributes aligned with that
  geometry. Row-changing operations apply the same row indices to both. A frame can carry
  secondary geometry columns, each with its own CRS (#93).
- **Reading and writing**: GeoParquet (with `bbox:` filter and write options), GeoJSON,
  FlatGeobuf, Feather v2 and Arrow IPC, Shapefile (`.cpg`, `.prj`, zipped), GeoPackage (several
  layers), and WKT/WKB. `read_file/2` and `write_file/3` pick the format from the extension;
  `list_layers/2` and `read_file_info/2` describe a file. Files written by GeoPandas 1.2 read
  directly, and GeoPandas reads what GeoFrame writes.
- **PostGIS**: `read_postgis/3` and `to_postgis/4` over the optional `:postgrex` dependency,
  with an EWKB Postgrex extension and exact `numeric` reads.
- **Coordinate reference systems**: `set_crs/2`, `to_crs/2` and `estimate_utm_crs/2`, in pure
  Rust on `proj4rs` (no PROJ install). EPSG codes plus PROJJSON and WKT1/WKT2 definitions
  without a code; datum shifts through NTv2/GeoTIFF grids (`GeoFrame.Grids`, nothing is
  downloaded) or the Helmert operation PROJ picks by area of use. `to_crs` agrees with pyproj
  to 1e-6 projected units on the reference cases.
- **Spatial joins and overlays**: `sjoin` and `sjoin_nearest` on an R-tree (every predicate,
  `dwithin`, ties, `max_distance`), `sindex_query`, `sindex_nearest`, `dissolve` with attribute
  aggregation, and `overlay` (all five `how`s), `clip`, `explode`, `union_all` and
  `intersection_all`.
- **Geometry operations** with GeoPandas parity: measures (planar, geodesic, haversine,
  Hausdorff, Frechet), element-wise predicates, constructive operations (buffer, offset curve,
  simplify, hulls, bounding shapes, Delaunay, Voronoi, polygonize, snap, set precision,
  `make_valid`), linear referencing, affine transforms, coverage validation and simplification,
  and `make_grid`. See [docs/parity.md](docs/parity.md) for the full GeoPandas parity matrix and
  the documented deviations.
- **Z and M coordinates**: XY, XYZ, XYM and XYZM geometries, `has_z/1`, `has_m/1`, `z/1`,
  `m/1`, `force_2d/1`, `force_3d/1`, `from_xy(z:)` and `get_coordinates(include_z:, include_m:)`,
  preserved through reprojection, affine operations, overlay and every I/O format (GeoJSON keeps
  Z only, as RFC 7946 has no M).
- **Explorer verbs on `GeoDataFrame`**: `select`, `discard`, `rename`, `put`, `pull`,
  `filter_with`, `sort_with`, `mutate_with`, `head`, `tail`, `slice`, `sample`, `distinct`,
  `drop_nil`, `join`/merge with a `DataFrame` or another `GeoDataFrame`, and `concat_rows`; the
  geometry follows the rows.
- **Livebook and Kino**: `GeoDataFrame` and `GeoSeries` render with Map, Table and Inspect
  tabs; `GeoFrame.Kino.deck/2` is a deck.gl widget fed with binary geometry buffers (a million
  features stay interactive) with hover tooltips, colour and size by column, a colour legend
  and a scale bar; two Smart Cells, **Read spatial file** and **Spatial join**. Payloads are
  capped by feature and vertex counts.
- **Testing helpers**: `GeoFrame.Testing` assertions for geometry series and frames, including
  secondary geometry columns.
- **Quality**: a differential suite checks every operation shared with GeoPandas 1.2 /
  Shapely 2.2 (18 864 reference cases: 18 131 agree, 733 are documented deviations), inputs are
  bounded and files are written atomically,
  and four notebooks (tour, deck.gl widget, GeoFrame vs GeoPandas, spike) run in CI.

### Known issues

- An ESRI WKT `.prj` for a StatePlane CRS in US survey feet (for example
  `NAD_1983_StatePlane_Arizona_Central_FIPS_0202_Feet`) is not recognised, so
  `GeoDataFrame.to_crs/2` and the Kino deck reprojection raise `ArgumentError`
  ([#110](https://github.com/CountlinkX-Solutions/geoframe/issues/110)). Workaround:
  `GeoDataFrame.set_crs(frame, "EPSG:2223")`, the same projection in international feet (about
  2 ppm off, fine for maps).

[Unreleased]: https://github.com/CountlinkX-Solutions/geoframe/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.2.0
[0.1.2]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.1.2
[0.1.1]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.1.1
[0.1.0]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.1.0
