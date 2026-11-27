# Changelog

All notable changes to GeoFrame are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The API may still
change before 1.0.

## [Unreleased]

### Fixed

- Compiling GeoFrame without the optional `kino` dependency no longer warns that
  `Kino.SmartCell.register/1` is undefined (Elixir 1.19). CI now compiles the library without
  its optional dependencies (`kino`, `maplibre`, `kino_maplibre`, `postgrex`) with warnings as
  errors.

## [0.3.3] - 2026-11-26

The first release published to Hex, as the `geoframe` package. The Hex package downloads a
precompiled NIF for nine targets, verified against the checksum file it ships, so it needs no
Rust toolchain. The `v0.3.3` git tag still builds the NIF from source (a stable Rust toolchain is
required). No public function changes.

### Documentation

- The README installs GeoFrame from Hex (`{:geoframe, "~> 0.3.3"}`), keeps the git tag and
  `path:` dependency as the way to build from source, and shows Hex and HexDocs badges.
- The release steps in the README publish the package from `MIX_ENV=prod` (where Rustler is an
  optional dependency) and the docs from `:dev`.
- The Hex package links to its documentation on HexDocs.
- The README link to the reference suite guide (`test/reference/README.md`) is an absolute
  GitHub URL: as a relative link, ExDoc pointed it at the README page itself.
- Community files for the public repository: a contributing guide, the Contributor Covenant
  2.1 code of conduct, a security policy, issue and pull request templates, and Dependabot
  configuration. The README lists the supported Elixir, OTP and Rust versions, and both Rust
  crates declare their Apache-2.0 license.

## [0.3.2] - 2026-11-13

Like earlier releases, it is distributed through the `v0.3.2` git tag and GitHub release, not
through Hex yet; the release attaches precompiled NIFs for nine targets. A git dependency builds
the NIF from source (a stable Rust toolchain is required).

A patch release that fixes the two known issues listed with 0.3.1: rows holding an empty member
point, and `get_coordinates` on non-finite values. No public function changes its signature.

### Fixed

- `get_coordinates` returns NaN and infinite ordinates (`:nan`, `:infinity`, `:neg_infinity`)
  as Shapely does instead of raising `ArgumentError`: an infinite X, Y, Z or M, or a NaN X or Y,
  could not cross from the NIF.
- `to_wkt` writes an empty member as `EMPTY`, as GEOS does. A `MULTIPOINT` with an empty member
  point (Shapely writes `MULTIPOINT (EMPTY, (1 1))` to WKB as a NaN point) made `to_wkt` raise a
  NIF panic, and an empty member line of a `MULTILINESTRING` or an empty ring of a `POLYGON` was
  written as `()`, which no reader accepts. `from_wkt` now also reads `MULTIPOINT (EMPTY, ...)`.
- A `MULTIPOINT` or collection with an empty member point (as Shapely writes
  `MULTIPOINT (EMPTY, (1 1))` to WKB) is no longer treated as missing. As in GEOS, the member
  takes no part in computations (`is_empty` is false, and `bounds`, `centroid`, the measures,
  predicates and constructive functions see the other members) but is still a member
  (`count_geometries` counts it, `get_geometry` and `explode` return it as `POINT EMPTY`, and the
  affine functions keep it). Before, `is_empty` was true and every measure null, and a
  GeoPackage written from such a row read back as null. Checked on 1 200 random rows against
  Shapely 2.2.

## [0.3.1] - 2026-11-13

Like earlier releases, it is distributed through the `v0.3.1` git tag and GitHub release, not
through Hex yet; the release attaches precompiled NIFs for nine targets. A git dependency builds
the NIF from source (a stable Rust toolchain is required).

A patch release: WKT is read as GEOS reads it in two more cases. Text that 0.3.0 read is read the
same way, and no public function changes.

### Fixed

- WKT without a `Z`, `M` or `ZM` tag whose coordinates have three or four ordinates is read as
  GEOS reads it instead of raising: `POINT (1 2 3)` is `POINT Z (1 2 3)` and four ordinates are
  ZM (never M). The first coordinate sets the dimension, and every other coordinate of that
  geometry must match it, as in GEOS. A collection member infers its own dimension
  (`GEOMETRYCOLLECTION Z (POINT (1 2 3))` is valid; `GEOMETRYCOLLECTION M (POINT (1 2 3))`
  raises "cannot mix dimensionality"). This applies to `GeoSeries.from_wkt` and to WKT geometry
  columns in GeoParquet, Feather and Arrow. Text that was read before is read the same way. On
  12 000 random WKT texts, accepted and rejected inputs match Shapely 2.2, and so do the
  coordinates, except for one GEOS quirk: `MULTIPOINT (1 2 3 4, 5 6 7 8)` (no inner
  parentheses) is ZM here, while GEOS drops its M.
- WKT ordinates `NaN`, `inf` and `Infinity` (any case, optional sign) are read as GEOS reads
  them instead of raising, so the WKT that `to_wkt` writes for a collection whose members mix
  dimensions (`GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),...)`, since 0.3.0) reads back, and Shapely's
  `Infinity` is understood. `POINT (NaN NaN)` reads as `POINT EMPTY`, the GeoArrow empty point.

## [0.3.0] - 2026-11-13

Like earlier releases, it is distributed through the `v0.3.0` git tag and GitHub release, not
through Hex yet; the release attaches precompiled NIFs for nine targets. A git dependency builds
the NIF from source (a stable Rust toolchain is required).

This release closes the GEOS and GDAL parity gaps left from 0.2.0. GeoFrame now reads curved
geometries and collections whose members mix dimensions, and follows GEOS for single-sided
polygon buffers and simplify's repair. No public function changes its signature. The results that change are
listed under Changed and Fixed.

### Added

- Curved geometries are read linearized, as GDAL reads them: `CircularString`, `CompoundCurve`,
  `CurvePolygon`, `MultiCurve` and `MultiSurface` (ISO WKB, PostGIS EWKB and GeoPackage
  binary, with Z, M or both, either byte order) become `LineString`, `Polygon`,
  `MultiLineString` and `MultiPolygon`. GDAL 3.12's arc stroking is ported (4° steps,
  `OGR_ARC_STEPSIZE` honoured, full circles and collinear arcs), so vertices match pyogrio bit
  for bit on 30 fixture cases and within 3e-12 on 26 000 fuzzed arcs. It covers GeoPackage
  (`read_file` and `stream_file`), `read_postgis`, `GeoSeries.from_wkb` and GeoParquet / Feather
  WKB columns, and `read_file_info` / `list_layers` report the linear type. Before, one curve
  row aborted the whole read. WKT curves are still an error.
- A `GEOMETRYCOLLECTION` whose members have different dimensions (a 2D point next to a Z point)
  is read as GEOS reads it instead of raising. The collection has Z when any member has Z and M
  when any has M, whatever the WKB header says. A GeoArrow collection holds one dimension, so
  the members are promoted with `NaN` for the ordinates they lack, which is what Shapely's
  `get_coordinates` reports. `has_z`, `has_m`, `z`, `m` and `force_2d` agree with GeoPandas,
  but the promoted member's WKT and WKB carry `Z NaN`. In WKT, a member of another dimension
  inside a collection tagged `Z`, `M` or `ZM` is still an error, as in GEOS.

### Changed

- `buffer(single_sided: true)` on polygons follows GEOS. The polygon gets the ordinary buffer,
  the input and buffer boundaries are noded, and the largest face is kept when there is more
  than one. For a square of side `s`, a positive distance `d` now returns the polygon itself
  from `s/d ≈ 4.68`, and a negative one returns the ordinary negative buffer from about 6.83;
  the old guide of "about four times the distance" was wrong. 2 700 fuzzed cases agree with
  Shapely within 2 % area. Lines and points are unchanged.
- `simplify(preserve_topology: false)` repairs an invalid Douglas-Peucker result with GEOS's
  `buffer(0)`, ported to Rust (`DPTransformer::createValidArea`: per polygon, once per
  multipolygon). A self-crossing ring keeps its positive-winding lobes, and overlapping or
  edge-touching parts dissolve. Results match Shapely vertex for vertex, including ring start,
  direction and part order: 15 475 fuzzed repairs, the 50 European countries of the tour at 2, 5
  and 20 km, and 242 countries at 50 km. Valid results are untouched.
- On a polygon with Z, the vertices that repair adds take GEOS's Z instead of `NaN`. A new
  crossing node gets the mean of the Z interpolated along its two segments, and a `NaN` Z lying
  on another ring is interpolated along it. 1 390 of 1 504 fuzzed repairs agree on every Z; the
  rest are points where three or more rings meet.

### Fixed

- `dissolve(observed: false)` takes each key's levels from its own values. It used
  `Explorer.Series.categories/1`, which in Explorer 0.13 returns every string categorized
  anywhere in the process, so unrelated categories (and, under concurrency, values from other
  frames) could appear as groups. Unobserved combinations of several keys are still added, as
  pandas does.

### Documentation

- `set_precision` on an invalid multipolygon keeps its merged, valid result in `valid_output`
  and `keep_collapsed` where GEOS raises `TopologyException` (about half of random invalid
  multipolygons). This is a decision, pinned by a test.
- The reference suite has 19 118 cases: 18 393 agree with GeoPandas / Shapely, 725 are
  documented deviations and none is unexplained (0.2.0: 18 379 agree, 739 documented).

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
  coordinates, use `to_crs/2`.
- **`read_file(bbox:)`, `stream_file(bbox:)` and FlatGeobuf reads use GDAL's semantics: a
  feature is kept when its geometry intersects the box, not when its envelope does.** A diagonal
  `LINESTRING (0 0, 10 10)` is dropped by the box `6 0 9 3`. Shapefile, GeoPackage and GeoJSON
  gain the option with the same rule; GeoParquet keeps its covering-box semantics, as GeoPandas.
  Migration: results can lose rows that only the envelope touched; if you relied on the envelope
  test, filter with `GeoSeries.total_bounds` or an envelope intersection after reading.
- **`write_feather/3` and `to_arrow` write 65 536-row record batches by default**, as pyarrow
  and GeoPandas `to_feather`, instead of one record batch per file. Migration: pass
  `record_batch_size:` (a positive integer) to choose another size; a file with a single batch
  needs a size at least as large as the frame. Existing files read the same.
- **On a grouped frame, `head`, `tail`, `slice` and `sample` act per group** (Explorer's grouped
  semantics, pandas `groupby.head`), keeping the original row order, with the geometry aligned.
  Migration: call `ungroup/2` first to get the old global behaviour. Ungrouped frames are
  unchanged.
- **`summarise` with `:n_distinct` no longer counts nulls** (pandas `nunique`). Migration: the
  previous value is the new one plus one for each group whose column holds a null.

### Added

- Precompiled NIFs through `rustler_precompiled`: the release workflow builds
  `aarch64-apple-darwin`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`,
  `x86_64-apple-darwin`, `x86_64-pc-windows-gnu`, `x86_64-pc-windows-msvc`,
  `x86_64-unknown-freebsd`, `x86_64-unknown-linux-gnu` and `x86_64-unknown-linux-musl` (NIF
  version 2.15) on each `v*` tag and attaches them to the release. A Hex package verifies the
  download against a shipped checksum file; `GEOFRAME_BUILD=true` forces a source build.
- A git or path dependency builds the NIF from source automatically (no `GEOFRAME_BUILD`
  needed), because a package without the checksum file cannot download a precompiled NIF; Rustler
  is then fetched as a dependency.
- `GeoFrame.stream_file/2`: a lazy `Stream` of `GeoDataFrame` batches for GeoParquet,
  FlatGeobuf, Feather / Arrow IPC, Shapefile (`.shp`, `.zip`), GeoPackage and GeoJSON, with
  `batch_size:` (default 65 536), `bbox:`, `columns:` and format options. Concatenating the
  batches equals `read_file/2`; early halts release the reader. On 500 000 points with
  10 000-row batches, peak memory is 155 to 196 MB against 235 to 308 MB for a whole read
  (GeoJSON 72 MB: 175 MB against 637 MB).
- `read_file(path, rows: n | first..last)` for every format, as GeoPandas `rows=`: the first `n`
  rows or an inclusive range, applied after `bbox:` and skipping rows cheaply where the format
  allows (for example `rows: 10` on a GeoParquet file: 1 ms against 239 ms).
- `read_file(bbox:)` for Shapefile, GeoPackage and GeoJSON, and `record_batch_size:` on
  `write_feather` / `to_arrow`.
- `GeoDataFrame.group_by/2`, `ungroup/2`, `groups/1` and `summarise/3`: attribute aggregations
  (`:sum :mean :median :min :max :count :first :last :n_distinct`) and geometry aggregations
  (`:union_all :intersection_all :collect :envelope :convex_hull :centroid :first :last :count`)
  computed in one NIF call, 8 to 14 times faster than a per-group loop at 100 000 rows and 1 000
  groups. `summarise(group_by(gdf, by), [])` equals `dissolve(gdf, by: by)`.
- `GeoDataFrame.group_map/4` (pandas `groupby(by).apply(fun)`) and the top-level `GeoFrame`
  delegates `group_by/2`, `summarise/3`, `ungroup/2` and `group_map/4`.
- `sindex_query/4` accepts a list of WKT strings or a single WKT string as input (a single
  geometry returns the ascending tree indices, as GeoPandas) and the `sort:` option;
  `get_geometry/2` takes a per-row index list or series.
- `GeoDataFrame.from_features/2` keeps the property column order for ordered input (keyword
  lists, `{key, value}` pairs, `Jason.OrderedObject`); `to_geo_dict/2` with GeoPandas' `na:`,
  `show_bbox:` and `drop_id:`; `GeoDataFrame.to_wkb/2` (`hex:`, `srid:`), `to_wkt/2` and
  `rename_geometry/2`.
- `dissolve` accepts `as_index: false` and `observed: false` (unobserved categories become empty
  groups); `as_index: true` and `level:` raise because GeoFrame has no index. The default stays
  `observed: true`.
- `frechet_distance` works on every geometry type, as GEOS, not only on `LineString`s.
- `buffer/3` takes a per-row distance (a list with `nil`, or a series) and `single_sided:`; lines
  and points match GEOS, polygons return the band, which matches GEOS only for polygons small
  relative to the distance (documented in `docs/parity.md`).
- The vertex constructors (`line_strings_from_vertices`, `polygons_from_rings`, ...) accept
  `{x, y, z}` and `[x, y, z]` vertices as Shapely 2.2.
- `GeoFrame.Kino.deck/2`: click popups (`popup: true | [columns]`) and several layers
  (`deck([gdf1, {gdf2, name: ..., color: ...}], opts)`) with a layer-control panel; a single
  frame sends the same payload as before.
- Shapefiles and GeoPackages written with one of the 1 217 supplementary EPSG codes keep their
  definition: a WKT1 `.prj` is written and the GeoPackage `definition` is filled, instead of a
  warning and `undefined`. 119 codes have no WKT1 in PROJ (Krovak Modified 5515 and 5516, 3D,
  geocentric and compound CRSs) and behave as before.

### Changed

- A missing Z reads as 0, as GDAL: a two-value position in a 3D GeoJSON geometry, and a row
  without Z written to a Shapefile or FlatGeobuf layer with Z (a stored `NaN` Z stays `NaN`).
- `is_valid` is false and `make_valid` repairs a polygon whose interior is disconnected, as GEOS
  (`Interior is disconnected`): a hole touching the shell at two or more points, or a chain or
  cycle of holes cutting the interior. A single touch stays valid. `null_out_invalid` nulls
  these polygons.
- `is_valid_reason` follows GEOS's check order and wording (`Self-intersection[2 2]`,
  `Hole lies outside shell[...]`, `Too few points in geometry component[...]`, and so on).
- `polygonize` with `node: true` and lines crossing polygons matches GEOS: no extra small face,
  and the crossing point is a vertex of the merged ring. On a 2 000-case fuzz, `polygonize`
  mismatches fell from 46 to 2 (and 54 to 3 on a second seed), with no case worse; `build_area`
  improves too (495 to 472 and 474 to 450) but still differs from GEOS where GEOS drops collinear
  vertices.

### Documentation

- `docs/parity.md` and the README follow every change above. The reference suite is
  now 19 118 cases: 18 379 agree with GeoPandas / Shapely and 739 are documented deviations,
  none unexplained (0.1.1: 18 134 agree and 730 documented of 18 864).

### Internal

- The release workflow uploads artifacts on pull requests that change it; the `.cargo` config
  links musl cdylibs without static CRT.

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
  modelled yet (for example Minna / UTM 12 m, ED50 / UTM up to 1.6 m).
- An ESRI WKT1 `.prj` on a non-Greenwich prime meridian resolves through its EPSG geographic CRS,
  identified by the datum name and the prime meridian, and reprojects like pyproj: NTF (Paris) /
  Lambert zone II, Lisbon, CH1903 (Bern), S-JTSK (Ferro), Batavia and Gunung Segara (Jakarta),
  MGI (Ferro) Austria grids. Text that GeoFrame cannot reproject correctly is refused with a clear
  error instead of answered tens to hundreds of metres off: MGI (Ferro) and Monte Mario (Rome)
  geographic text, and EPSG:5221 written as ESRI text.

### Fixed

- ESRI WKT1 projection names that were refused with `unsupported WKT definition` are read, onto
  methods GeoFrame already had: `Gauss_Kruger`, `Albers`, `Cassini`, `Double_Stereographic`
  (RD New, EPSG:28992), polar `Stereographic` variants, Hotine and Rectified Skew Orthomorphic
  variants, Mercator variant B, Krovak with `X_Scale` / `Y_Scale` / `XY_Plane_Rotation`, LCC 2SP
  Michigan, `Equal_Earth`, Lambert azimuthal equal area and a few more. Of 5 330 projected EPSG
  CRSs written as WKT1_ESRI, 36 are still refused (was 1 007), and 30 are more than 5 cm from
  pyproj (was 115).

## [0.1.1] - 2026-11-12

Like 0.1.0, it is distributed through the `v0.1.1` git tag and GitHub release, not through Hex.

Results change in three places, each to match GEOS or PROJ: `is_valid` and `make_valid` on
collinear (zero-area) rings, `polygonize` and `build_area` with `node: true`, and `to_crs` on
ESRI WKT1 `.prj` files, which now get the datum shift PROJ applies.

### Added

- `set_crs/2` on `GeoSeries` and `GeoDataFrame` accepts a positive integer EPSG code, like
  `to_crs/2` already did: `set_crs(series, 3857)` stores `"EPSG:3857"`, the same label as the
  string input. A non-positive integer raises `ArgumentError`.

### Changed

- A polygon ring whose vertices are all collinear (zero area) is now invalid, as in GEOS:
  `is_valid` is false (reason `Self-intersection[...]` or `Too few points in geometry
  component[...]`) and `make_valid` collapses it to linework (`MULTILINESTRING`, `LINESTRING` or
  `POINT`) instead of returning it unchanged. A valid polygon with a collinear hole, or a
  multipolygon with a collinear part, becomes a `GEOMETRYCOLLECTION` of the polygon and the
  linework. The reference
  suite is now 18 134 agree and 730 documented deviations out of 18 864.
- `polygonize` and `build_area` with `node: true` keep every input vertex that lies on the merged
  outline, as GeoPandas does (`shapely.polygonize([union_all()])`). Two squares sharing an edge
  now give `POLYGON ((0 0, 0 1, 1 1, 2 1, 2 0, 1 0, 0 0))` instead of dropping the collinear
  vertices. On a 400-case fuzz against GEOS, `polygonize` mismatches fell from 232 to 8 and
  `build_area` from 245 to 134, with no case worse.
- An ESRI WKT1 `.prj` (`GCS_...` / `D_...` names, no `AUTHORITY`) now gets the datum shift PROJ
  applies, identified by the DATUM name instead of the GEOGCS name: ED50 about 138 m, OSGB36
  about 109 m, Tokyo about 463 m, NAD27 UTM about 20 m. Before, `to_crs/2` returned the unshifted
  coordinates for that text. Against pyproj on 1 110 CRSs written as WKT1_ESRI, mismatches over
  5 cm fell from 716 to 60, with no case worse. GDA94, GDA2020 and CR-SIRGAS are not covered, and
  a definition with a non-Greenwich prime meridian (Ferro, Paris, Rome, ...) keeps its previous
  result.

### Fixed

- An ESRI WKT1 `.prj` for a StatePlane CRS (for example ESRI:102649, Arizona Central in US
  survey feet) is now read by `to_crs/2`, `set_crs/2` and `GeoFrame.Kino.deck/2` instead of
  raising `unsupported WKT definition`. WKT1 parameter names are matched without regard to
  case (ESRI writes `False_Easting`, `Standard_Parallel_1`), and ESRI's
  `Lambert_Conformal_Conic` is read as the 2SP method when it has a second standard parallel, and
  as the 1SP method (with `Standard_Parallel_1` equal to `Latitude_Of_Origin`, as ESRI writes it)
  otherwise; a lone standard parallel that differs from the origin is still refused.
  `Foot_US` and `Foot` units are honoured. Reprojection matches pyproj.
- Mixed-dimension geometry collections (XY, XYZ, XYM and XYZM members together) no longer print
  GeoArrow panic messages on stderr. The error returned is the same and behaviour is unchanged.

### Documentation

- The description of GEOS `set_precision` on invalid multipolygons in `docs/parity.md` is
  corrected: GEOS keeps a part whose edges meet no other part's, merges parts with coinciding
  edges, and raises `TopologyException` on crossing edges.

### Internal

- CI caches the compiled NIF together with `_build`.
- The linear-referencing property test no longer asserts that `project` returns the earliest
  pass on overlapping lines, which GEOS does not guarantee either.

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
  `GeoDataFrame.to_crs/2` and the Kino deck reprojection raise `ArgumentError`. Workaround:
  `GeoDataFrame.set_crs(frame, "EPSG:2223")`, the same projection in international feet (about
  2 ppm off, fine for maps).

[Unreleased]: https://github.com/CountlinkX-Solutions/geoframe/compare/v0.3.3...HEAD
[0.3.3]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.3.3
[0.3.2]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.3.2
[0.3.1]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.3.1
[0.3.0]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.3.0
[0.2.0]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.2.0
[0.1.2]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.1.2
[0.1.1]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.1.1
[0.1.0]: https://github.com/CountlinkX-Solutions/geoframe/releases/tag/v0.1.0
