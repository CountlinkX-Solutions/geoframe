# GeoPandas / GeoPolars (dev) / GeoFrame parity matrix

Sources: GeoPandas stable API reference (geoseries, geodataframe, io, tools, sindex, testing pages all fetched OK; the
reference index page was not needed). GeoPolars `dev` snapshot (`docs/Roadmap.md`, `docs/Tier per expression.md`,
`geopolars/geopolars/geo/*.py`, `expr/geo.py`). GeoFrame = branch `feat/io-formats` (`lib/**`, `native/geoframe_core`).

Legend: ✅ done | 🟡 partial | ❌ missing | ⛔ out of scope.
GeoPolars column: `yes` = exposed in the Python API on `dev`; `no`/`roadmap` = absent (the Roadmap lists binary predicates,
set ops, interpolate/project, line joining etc. as unchecked). GeoFrame names are `GeoFrame.GeoSeries.*` unless noted.
GeoFrame backend today: `geo` 0.33, `geoarrow-*` 0.9, `geoparquet` 0.9, `proj4rs`, `flatgeobuf`, `geozero`, `geo-buf`-style buffer.
Note: GeoFrame has no GeoDataFrame-level delegation of GeoSeries methods other than area/length/bounds/centroid
(users call `GeoSeries` on `gdf.geometry`); this is counted as ✅ for the series member and noted once in section 12.

## Reference testing

Since batch V every operation GeoFrame shares with GeoPandas is checked against the real library:
`test/reference/generate.py` (dev-only; GeoPandas 1.2.0, Shapely 2.2.0, GEOS 3.14.1, pyproj 3.8.0) computes
the results for a fixed corpus and commits them as JSON in `test/reference/fixtures/`;
`test/geo_frame/reference_test.exs` compares GeoFrame with them in CI without Python (about 10 800 cases
across 77 operations, plus sjoin, sjoin_nearest, dissolve and union_all on small frames). Known differences
are listed in `test/reference/known_deviations.exs` and the suite asserts that each one still deviates. See
`test/reference/README.md` for the tolerances and how to regenerate. Documented deviations:

- **Null and empty inputs.** GeoFrame returns `nil` where GeoPandas returns `False`, `0`, `NaN` or an empty
  geometry (`area`, predicates, `centroid`, `envelope`, `bounds`, ...); `count_geometries` of an empty atomic
  geometry is 0 (Shapely: 1). `POINT EMPTY` cannot be stored in `geo` types.
- **Invalid input.** Predicates and `relate` on a self-intersecting bowtie polygon follow geo's relate and differ
  from GEOS in a few pairs. Valid inputs agree in every case.
- **Zero-area polygons and degenerate lines.** `is_valid` accepts a polygon whose ring is collinear (GEOS: invalid)
  and `make_valid` leaves it unchanged; `is_ring` is false for the two-point closed line `LINESTRING (1 1, 1 1)`;
  `is_valid_reason` uses geo's wording, not GEOS' text.
- **`envelope` of a horizontal/vertical line.** GeoFrame returns a line; GEOS 3.14 returns a zero-area polygon with
  repeated vertices.
- **`buffer` mitre/bevel joins** on the acute corner of a closed line differ between geo-buf and GEOS; round caps
  and joins, flat and square caps on open lines agree (area within 2 %).
- **`line_merge` of a polygon** gives an empty collection (GEOS merges its boundary).
- **Set operations.** Line x line and mixed-dimension combinations are `nil`; lower-dimensional parts of a result
  (shared edges or vertices) are dropped; mixed-family `union_all`/`dissolve` is `nil`. Batch B closes these.
  Results agree within 1e-6, not bit-exactly: `geo`'s boolean operations snap intersection points to an integer
  grid (about 1e-8 of the extent).
- **Tolerances.** `to_crs` against PROJ: within 1e-6 projected units (1e-9 degrees back), including the Florida
  East (ftUS) transverse Mercator. Geodesic area, length and point distance: within 1e-6 of `pyproj.Geod`.

Fixed thanks to the suite: `distance`/`dwithin` returned `0.0`/`true` (or `f64::MAX` for an empty collection) for
empty inputs instead of `nil`; `remove_repeated_points` reduced a collapsed line to a one-point `LINESTRING`
(GEOS keeps two points).

## 1. Constructors / conversion (GeoSeries)

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `GeoSeries.from_wkt` | `from_wkt` yes | `from_wkt/2`, `from_wkt!/2` | ✅ | |
| `GeoSeries.from_wkb` | `from_wkb` yes | `from_wkb/2`, `from_wkb!/2` | ✅ | |
| `GeoSeries.from_xy` / `points_from_xy` | `point`, `line_string`, `multi_point`, `polygon`... constructors from columns yes | `from_xy/3`, `GeoFrame.points_from_xy/3` | ✅ | lists or Explorer series; nil/NaN -> null point; `crs:`; `z:` validated then dropped (2D storage) |
| `GeoSeries.from_file` | — | `GeoFrame.read_file` (GDF only) | 🟡 | series-only variant missing; trivial wrapper |
| `GeoSeries.from_arrow` | — (geoarrow native) | — | ❌ | geoarrow-array available in core |
| `GeoSeries.to_wkt` | `to_wkt` yes | `to_wkt/1` | ✅ | |
| `GeoSeries.to_wkb` (hex option) | `to_wkb` yes | `to_wkb/2` | ✅ | `hex: true` gives upper-case hex strings |
| `GeoSeries.to_json` | — | `to_geojson/1` | 🟡 | returns list of decoded maps, not a FeatureCollection string |
| `GeoSeries.to_arrow` | — | — | ❌ | |
| `GeoSeries.to_file` | — | — | ❌ | trivial wrapper over write_file (needs a GDF) |
| `GeoSeries.__geo_interface__` | — | — | ❌ | `to_geojson` is the closest |
| `GeoSeries(...)` constructor from geometries list/Shapely | datatypes | — | ⛔ | no Shapely in Elixir; WKT/WKB/xy are the entry points |
| — | `to_polygon` (box struct -> polygon) yes | — | ⛔ | GeoFrame has no box dtype; `envelope` covers it |
| — | `line_string_from_vertices`, `multi_*_from_*`, `polygon_from_rings` etc. yes | — | ❌ | build constructors from columns/lists; same batch as from_xy |

## 2. Attributes / measurements

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `area` | `area`, `area_planar` yes | `area/2` (`method: :planar \| :geodesic`) | ✅ | planar by default; zero for points/lines (GeoPolars errors unless `allow_non_polygons`) |
| — | `area_geodesic` yes (PROJ+geographiclib, needs CRS) | `area_geodesic/1` | ✅ | m², WGS 84, unsigned (orientation-independent: rings are oriented first because geo returns the ellipsoid complement for clockwise exteriors); projected CRS reprojected to lon/lat; nil CRS raises |
| `length` | `length` yes (planar/geodesic) | `length/2` (`method:`), `length_geodesic/1` | ✅ | metres on WGS 84; polygon perimeter includes holes |
| `distance` | `distance` yes (planar/geodesic) | `distance/3` (`method: :planar \| :geodesic \| :haversine`) | 🟡 | no `align`; geodesic/haversine exact for point pairs, approximate otherwise (planar nearest points in lon/lat, then geodesic between them) |
| `bounds` | `bounds` yes | `bounds/1` (DataFrame minx..maxy) | ✅ | |
| `total_bounds` | — | `total_bounds/1` | ✅ | |
| `geom_type` | `dimension_of`/dtype | `geom_type/1` | ✅ | |
| `x`, `y` | `x`,`y` yes | `x/1`, `y/1` | ✅ | nil for non-points (GeoPandas raises) |
| `z`, `m` | `z`,`m` yes | — | ❌ | GeoFrame is 2D-only (Z/M dropped); needs dimension-aware storage -> large |
| `get_coordinates` | `coordinates` (x/y lists) | `get_coordinates/2` | 🟡 | DataFrame `x`,`y`; `include_index:` adds row position; no `include_z`, `ignore_index`, `index_parts` |
| `count_coordinates` | yes | `count_coordinates/1` | ✅ | |
| `count_geometries` | — | `count_geometries/1` | ✅ | null rows are `nil` (Shapely: 0); an empty atomic geometry gives 0 (Shapely: 1) |
| `count_interior_rings` | — | `count_interior_rings/1` | ✅ | 0 for non-polygons as Shapely; null rows `nil` |
| `get_geometry(index)` | — | `get_geometry/2` | 🟡 | scalar integer index only (negative from end); no per-row index array |
| `get_precision` / `set_precision` | — | — | ❌ | needs grid snapping; low value |
| `hausdorff_distance` | — | `hausdorff_distance/3` | ✅ | GEOS discrete algorithm (vertex to whole geometry), `densify:`; not geo's vertex-to-vertex `HausdorffDistance` |
| `frechet_distance` | — | `frechet_distance/3` | 🟡 | `densify:`; discrete over vertices; `LineString` only, other types are `nil` |
| `minimum_bounding_radius` | — | — | ❌ | Welzl; geo has no direct trait (compute from circle) |
| `minimum_clearance` | — | — | ❌ | no geo trait; hard |
| `offset_curve` | — | — | ❌ | geo-buf crate may provide; otherwise own impl |
| `interiors` | `interior` yes (rings after first) | `interiors/1` | 🟡 | MultiLineString of holes per polygon (GeoPandas returns a list of rings); nil for non-polygons |
| `exterior` | `exterior` yes | `exterior/1` | ✅ | nil for non-polygons |
| `boundary` | `boundary` yes | `boundary/1` | ✅ | |
| `representative_point` | — | `representative_point/1` | ✅ | geo `InteriorPoint` |
| — | `mean_coordinate` yes | `mean_coordinate/1` | ✅ | vertex mean; polygon ring closing vertex not double counted |
| — | `is_geographic` yes | `is_geographic/1`, `GeoFrame.CRS.geographic?/1` | ✅ | proj4rs `is_latlong`; `nil` without or with an unresolvable CRS |
| — | `rings_overlap`, `self_intersects` yes | — | ❌ | niche; partial overlap with `is_simple` |

## 3. Unary predicates

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `is_valid` | `is_valid` yes | `is_valid/1` | ✅ | geo `Validation`; a zero-area collinear ring is valid here, invalid in GEOS (see Reference testing) |
| `is_valid_reason` | — | `is_valid_reason/1` | 🟡 | "Valid Geometry" as Shapely; first reason only, in `geo` wording rather than GEOS (reference-tested: only "Valid Geometry" matches) |
| `is_empty` | yes | `is_empty/1` | ✅ | |
| `is_ring` | `is_ring` yes | `is_ring/1` | ✅ | closed, >= 4 vertices, simple; quadratic check; GEOS also accepts the degenerate `LINESTRING (1 1, 1 1)` |
| `is_closed` | — | `is_closed/1` | ✅ | lines and multilines; others false (Shapely) |
| `is_simple` | `self_intersects` (inverse, lines) | — | ❌ | linestring self-intersection via geo `Intersects`/sweep |
| `is_ccw` | `is_ccw` yes | `is_ccw/1` | ✅ | closed `LineString` only; open lines and other types `false` |
| `has_z`, `has_m` | via dtype | — | ⛔ | GeoFrame is 2D only (revisit with z/m) |
| `is_valid_coverage`, `invalid_coverage_edges` | — | — | ❌ | GEOS-only concept; no geo equivalent; low value |
| — | `null_out_invalid` yes | `null_out_invalid/1` | ✅ | |
| `isna` / `notna` | null semantics | `isna/1`, `notna/1` | ✅ | only actual nulls, not empty geometries (GeoPandas >= 0.6) |
| `fillna` | — | `fillna/2` | ✅ | default `GEOMETRYCOLLECTION EMPTY`; WKT or one-row series; CRS checked |
| `is_empty` (missing/empty) | yes | ✅ | ✅ | listed above |

## 4. Binary predicates

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `intersects` | roadmap (no) | `intersects/2` | ✅ | all predicates: null/empty rows are `nil` (GeoPandas `False`); results on invalid bowtie input can differ from GEOS (see Reference testing) |
| `contains` | no | `contains/2` | ✅ | |
| `within` | no | `within/2` | ✅ | |
| `touches` | no | `touches/2` | ✅ | |
| `crosses` | no | `crosses/2` | ✅ | |
| `overlaps` | no | `overlaps/2` | ✅ | |
| `disjoint` | no | `disjoint/2` | ✅ | |
| `covers` | no | `covers/2` | ✅ | |
| `covered_by` | no | `covered_by/2` | ✅ | |
| `geom_equals` | no | `geom_equals/2` | ✅ | topological equality |
| `geom_equals_exact` | no | `geom_equals_exact/3` | ✅ | structure + vertex order, planar tolerance (inclusive) |
| `geom_equals_identical` | no | `geom_equals_identical/2` | ✅ | exact coordinates, NaN equals NaN |
| `contains_properly` | no | `contains_properly/2` | ✅ | DE-9IM `T**FF*FF*`; also a `sindex_query`/`sjoin` predicate |
| `dwithin` | no | `dwithin/3,4` | ✅ | `method:` as `distance/3`; geodesic limit in metres |
| `relate` | no | `relate/2` | ✅ | DE-9IM string |
| `relate_pattern` | no | `relate_pattern/3` | ✅ | validated 9-char pattern (`T F * 0 1 2`, case-insensitive) |
| `align=` parameter on all binary ops | — | — | ⛔ | pandas index alignment; GeoFrame pairs by position |

## 5. Set-theoretic / overlay

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `intersection` | no | `intersection/2` | 🟡 | polygonal full; line x polygon and point cases; other combos `nil`; lower-dim results dropped (shared edge gives `POLYGON EMPTY`); results agree with GEOS within 1e-6 (integer-grid snapping) |
| `union` | no | `union/2` | 🟡 | same limits; no line x line, no mixed dim |
| `difference` | no | `difference/2` | 🟡 | same limits |
| `symmetric_difference` | no | `symmetric_difference/2` | 🟡 | same limits |
| `clip_by_rect` | no | — | ❌ | geo `BooleanOps` with rect, or dedicated clip; medium |
| `clip` (GeoSeries/GDF) | no | — | ❌ | `intersection` with a mask + filter; medium |
| `overlay` (GDF; how=intersection/union/identity/symmetric_difference/difference) | no | — | ❌ | needs sindex + set ops + attribute assembly; depends on lower-dim handling; large |
| `intersection_all` | no | — | ❌ | fold of `intersection`; easy |
| `union_all` | no | `union_all/1` | ✅ | (`unary_union` deprecated: not needed) |
| — | — | `union_groups/3` | ✅ | GeoFrame-only, used by dissolve |

## 6. Constructive methods

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `buffer` | no | `buffer/3` | 🟡 | cap/join/quad_segs/mitre; no `single_sided`; no per-row distance series; planar; reference-tested area within 2 %, mitre/bevel joins on acute closed-line corners differ from GEOS |
| `centroid` | `centroid` yes | `centroid/1` | ✅ | |
| `convex_hull` | `convex_hull` yes | `convex_hull/1` | ✅ | |
| `envelope` | via `bounds`/`to_polygon` | `envelope/1` | ✅ | horizontal/vertical line gives a line; GEOS 3.14 gives a zero-area polygon |
| `simplify` | `simplify` yes | `simplify/3` | ✅ | `preserve_topology` supported |
| `simplify_coverage` | no | — | ❌ | needs topology-preserving shared edges; hard; geo `SimplifyVw` is not coverage-aware |
| `make_valid` | no | `make_valid/1` | 🟡 | geo `MakeValid`; polygons only; differs from GEOS in vertex detail; leaves a zero-area collinear polygon unchanged |
| `representative_point` | no | ✅ | ✅ | see §2 |
| `concave_hull` | no | — | ❌ | geo `ConcaveHull` (`ratio`, `allow_holes` -> only ratio) |
| `minimum_rotated_rectangle` | no | — | ❌ | geo `MinimumRotatedRect` |
| `minimum_bounding_circle` | no | — | ❌ | implement (Welzl) |
| `maximum_inscribed_circle` | no | — | ❌ | polylabel (geo has `Polylabel`? verify; geo 0.33 has `polylabel` feature) |
| `minimum_clearance_line` | no | — | ❌ | hard |
| `extract_unique_points` | no | `extract_unique_points/1` | ✅ | MultiPoint in first-seen order |
| `remove_repeated_points` | no | `remove_repeated_points/2` | ✅ | `tolerance`; own thinning keeps first and last vertex; 0 uses geo |
| `reverse` | no | `reverse/1` | ✅ | every type; multi-geometry part order kept |
| `normalize` | no | — | ❌ | canonical ordering; medium (GEOS-specific order) |
| `orient_polygons` | no | `orient_polygons/2` | ✅ | `exterior_cw:` |
| `segmentize` | no | `segmentize/2` | ✅ | Euclidean, `Densify` |
| `force_2d` | dtype | (always 2D) | ✅ | implicit |
| `force_3d` | dtype | — | ⛔ | no Z storage |
| `snap` | no | — | ❌ | no geo trait; hard |
| `shortest_line` | no | `shortest_line/2` | ✅ | composed from `ClosestPoint` both ways plus segment crossings; ties may differ from GEOS |
| `sample_points` | no | — | ❌ | random points in polygons; needs rand; medium |
| `make_grid` (tools) | no | — | ❌ | pure construction; easy |
| `transform` (apply function to coordinates) | no | — | ⛔ | arbitrary callbacks over NIF coordinates; `to_crs`/affine cover real uses |
| `build_area` / `polygonize` | `polygon_from_rings` (different) | — | ❌ | no geo equivalent; hard (noding) |
| `delaunay_triangles` | no | — | ❌ | geo `TriangulateDelaunay` (spade) |
| `constrained_delaunay_triangles` | no | — | ❌ | geo `TriangulateSpade` constrained |
| `voronoi_polygons` | no | — | ❌ | geo `Voronoi` (spade) |
| `explode` | no | — | ❌ | multi -> single; needs index_parts; GDF explode duplicates attributes; medium |
| `get_geometry` / `geometry` parts | no | — | ❌ | see §2 |

## 7. Affine transformations

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `translate` | `translate` yes | `translate/3` | ✅ | numbers, lists or Explorer series per row |
| `scale` | `scale`, `scale_about_centroid` yes | `scale/4` | ✅ | origin: `:center`, `:centroid`, `{x,y}` |
| `rotate` | `rotate`, `rotate_about_centroid` yes | `rotate/3` | ✅ | |
| `skew` | `skew`, `skew_about_centroid` yes | `skew/4` | ✅ | degrees or `use_radians: true`; origin as in `scale/4` |
| `affine_transform` (6/12-param matrix) | `affine_transform` yes | `affine_transform/2` | ✅ | Shapely order `[a, b, d, e, xoff, yoff]`; the 12-element 3D form raises (2D storage) |
| per-row parameters (column-valued offsets/angles/origin) | yes | `translate`/`scale`/`rotate`/`skew` | ✅ | each numeric argument is a number, list or Explorer series; `nil` nulls the row; `origin:` may be a list of `{x, y}` |

## 8. Linestring / linear referencing

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `interpolate` | roadmap | `interpolate/3` | ✅ | `normalized:`, negative distance, per-row distance; MultiLineString parts concatenated; non-lines `nil` |
| `project` | roadmap | `project/3` | ✅ | `normalized:`, `other` broadcasts |
| `line_merge` | roadmap ("Linestring joining") | `line_merge/2` | ✅ | own endpoint-graph merge; `directed:`; non-lines give an empty collection (GEOS merges a polygon's boundary) |
| `shared_paths` | no | — | ❌ | hard |
| `offset_curve` | no | — | ❌ | see §2 |
| `shortest_line` | no | `shortest_line/2` | ✅ | see §6 |
| `chaikin`-style smoothing | no | — | ❌ | not in GeoPandas; skip |

## 9. Aggregation / dissolve

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `union_all` | no | ✅ | ✅ | |
| `intersection_all` | no | — | ❌ | |
| `GeoDataFrame.dissolve` | no | `dissolve/2` (`GeoFrame.Dissolve`) | 🟡 | `by`, `aggfunc` (first,last,sum,mean,min,max,count), `dropna`, `sort`; missing: per-column aggfunc map, `as_index`, `level`, `observed` |
| `unary_union` (deprecated) | — | — | ⛔ | deprecated |
| `GeoDataFrame.explode` | no | — | ❌ | see §6 |

## 10. Projection handling

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `crs` | CRS in metadata | `crs/1` (Series + GDF) | ✅ | |
| `set_crs` | `set_crs` yes | `set_crs/2` (Series + GDF) | 🟡 | relabel only; no `allow_override`, no integer EPSG input (string/nil only) |
| `to_crs` | `to_crs` yes (PROJ) | `to_crs/2` | 🟡 | proj4rs (no datum-grid shifts, no pipeline); accepts string or integer |
| `estimate_utm_crs` | — | `estimate_utm_crs/1` | ✅ | 80S-84N |
| `crs` mismatch behaviour | — | raises | ✅ | stricter than GeoPandas (which warns) |
| `Series.crs` setter from pyproj CRS objects | — | — | ⛔ | no pyproj object model |

## 11. Spatial index and joins

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `sindex` / `has_sindex` | no | (internal rstar) | ⛔ | built per call, not a persistent object |
| `sindex.query` | no | `sindex_query/4` | 🟡 | returns `{left_idx, right_idx}`; predicates incl. `:dwithin` w/ `:distance`; no `sort`, no single-geometry/array-of-geometry input mode |
| `sindex.nearest` | no | `sindex_nearest/3` | ✅ | `:max_distance`, `:exclusive`, returns distances |
| `sindex.intersection` (bbox) | no | `sindex_intersection/2` | ✅ | envelope test, linear scan over bounds (no persistent tree) |
| `sindex.valid_query_predicates` | no | `sindex_valid_query_predicates/0` | ✅ | list of atoms; `:disjoint` excluded |
| `sindex.size`, `sindex.is_empty` | no | — | ⛔ | no persistent index object |
| `sjoin` | no | `sjoin/3` | ✅ | `how` inner/left/right, `predicate`, `distance`, suffixes |
| `sjoin_nearest` | no | `sjoin_nearest/3` | ✅ | `max_distance`, `distance_col`, `exclusive`, `how` |
| `GeoSeries.cx` / `GeoDataFrame.cx` | no | `GeoSeries.cx/5`, `GeoDataFrame.cx/5` (+ `read_flatgeobuf(bbox:)`) | ✅ | `nil` bound = unbounded; exact `intersects` against the box |

## 12. GeoDataFrame specifics

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `GeoDataFrame(...)` | DataFrame + geometry column | `GeoDataFrame.new/3` | ✅ | |
| `from_features` | — | `GeoDataFrame.from_features/2` | 🟡 | list or FeatureCollection; `crs:`, `columns:`; GeoJSON inference; property columns sorted by name (Elixir maps) |
| `from_arrow` / `to_arrow` | geoarrow native | — | ❌ | |
| `from_postgis` / `to_postgis` / `read_postgis` | roadmap | — | ❌ | needs Postgrex + WKB; Elixir-side, medium |
| `to_geo_dict` | — | `to_geojson/1` | 🟡 | features list; no `na`, `show_bbox`, `drop_id` |
| `iterfeatures` | — | — | ⛔ | Python generator |
| `to_json` | — | `write_geojson` | 🟡 | file only; no string-returning variant |
| `to_wkb`, `to_wkt` (frame) | yes | `to_explorer(encoding: :wkb/:wkt)` | 🟡 | differently shaped |
| `rename_geometry` | — | `rename/2` | 🟡 | via generic rename |
| `set_geometry`, `active_geometry_name` | — | `geometry/1`, `new/3`, `names/1` | 🟡 | single geometry column; no multi-geometry frames |
| `crs` / `set_crs` / `to_crs` / `estimate_utm_crs` | yes | ✅ | ✅ | |
| `explode` | no | — | ❌ | |
| `dissolve`, `sjoin`, `sjoin_nearest` | no | ✅/🟡 | see §9/§11 | |
| `clip`, `overlay` | no | — | ❌ | |
| pandas ops (`head`, `tail`, `take`, `select`, `rename`, filter, sort, mutate) | polars | `head`, `tail`, `take`, `slice`, `select`, `rename`, `filter_with`, `sort_with`, `mutate_with` | ✅ | wraps Explorer; GDF-level delegation exists for `area`, `length`, `bounds`, `centroid` only; others via `gdf.geometry` |
| index/merge/groupby/apply/concat of GDFs | polars | `GeoFrame.concat/1`, `GeoDataFrame.concat_rows/1`, `GeoSeries.concat/1` | 🟡 | row-wise concat done (CRS and geometry column must match); merge/join of GDFs missing (only through Explorer on `.data`) |

## 13. I/O

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `read_parquet` (GeoParquet) | `from_geoparquet`, `geoparquet_metadata` yes | `read_parquet/2` (`columns:`) | ✅ | |
| `to_parquet` | writes yes, `with_covering` bbox | `write_parquet/3` | 🟡 | GeoParquet 1.1 WKB; opts ignored (no compression, covering bbox, row-group size) |
| `read_feather` / `to_feather` | — | — | ❌ | Arrow IPC with geoarrow; moderate |
| `read_file` | no | `read_file/2` | 🟡 | dispatch by extension: parquet, geojson, flatgeobuf only; no shapefile, GPKG, GML, KML etc. (GDAL formats) |
| `to_file` | no | `write_file/3` | 🟡 | same formats |
| GeoJSON read/write | roadmap | `read_geojson`, `write_geojson` | ✅ | |
| FlatGeobuf read/write | no | `read_flatgeobuf` (bbox), `write_flatgeobuf` | ✅ | |
| `read_file_info`, `list_layers` | no | — | ❌ | only meaningful with multi-layer drivers (GPKG) |
| Shapefile / GeoPackage / OGR | roadmap "GDAL vector files" | — | ❌ | either pure-Rust (`shapefile`, `rusqlite`+gpkg) or GDAL binding; large; decide |
| `read_postgis` / `to_postgis` | roadmap | — | ❌ | see §12 |
| GeoSeries `to_arrow` / `from_arrow` | yes | — | ❌ | |

## 14. Tools

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `points_from_xy` | `point` from columns | `GeoFrame.points_from_xy/3` | ✅ | alias of `GeoSeries.from_xy/3` |
| `geocode`, `reverse_geocode` | — | — | ⛔ | needs network geocoding service (geopy) |
| `tools.collect` | — | `collect/2` | ✅ | one-row series; `multi:` option |
| `tools.clip` | — | — | ❌ | see §5 |
| `tools.overlay` | — | — | ❌ | see §5 |
| `tools.sjoin`, `sjoin_nearest` | — | ✅ | ✅ | |
| `tools.make_grid` | — | — | ❌ | easy |
| `tools.geocode` | — | — | ⛔ | |

## 15. Plotting / interaction

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `plot` (matplotlib) | — | `GeoFrame.Kino.map/2`, `Kino.Render` (MapLibre) | ⛔ | replaced by Livebook map; `max_map_features` cap |
| `explore` (folium) | — | `GeoFrame.Kino.map/2` | 🟡 | static MapLibre render; no popups/tooltip/choropleth options verified |
| `preview` | — | `preview/1` | ✅ | GeoFrame-only |

## 16. Testing

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| `testing.assert_geoseries_equal` | — | `GeoFrame.Testing.assert_geoseries_equal/3` | ✅ | topological by default, `check_less_precise` uses exact with 1e-6; `check_crs`, `check_geom_type`, `normalize` |
| `testing.assert_geodataframe_equal` | — | `GeoFrame.Testing.assert_geodataframe_equal/3` | ✅ | adds `check_dtype`; column order must match |

## 17. GeoPolars-only members not covered elsewhere

| GeoPandas | GeoPolars dev | GeoFrame | Status | Notes |
|---|---|---|---|---|
| — | `area_geodesic` | `area_geodesic/1` | ✅ | see §2 |
| — | `wrap_longitude` | `wrap_longitude/1` | ✅ | x into [-180, 180); geographic or no CRS; geometries are not split at the antimeridian |
| — | `skew`, `skew_about_centroid` | `skew/4` (`origin: :centroid`) | ✅ | see §7 |
| — | `null_out_invalid` | `null_out_invalid/1` | ✅ | see §3 |
| — | `count_coordinates` | `count_coordinates/1` | ✅ | |
| — | `is_ccw` | `is_ccw/1` | ✅ | see §3 |
| — | `mean_coordinate` | `mean_coordinate/1` | ✅ | see §2 |
| — | `affine_transform` | `affine_transform/2` | ✅ | see §7 |
| — | `rotate_about_centroid`, `scale_about_centroid` | `rotate`/`scale` with `origin: :centroid` | ✅ | |
| — | `is_geographic` | `is_geographic/1` | ✅ | see §2 |
| — | `rings_overlap`, `self_intersects` | — | ❌ | |
| — | `with_covering` (GeoParquet bbox covering) | — | ❌ | `write_parquet` opts ignored |
| — | `geoparquet_metadata` | — | ❌ | expose `geo` metadata of a file (cheap) |
| — | `is_close` (internal) | — | ⛔ | internal helper |

---

# Gap summary and proposed batches

Status counts: see bottom of this file (computed by script).

Verified geo 0.33.1 traits (in `~/.cargo/registry/src/*/geo-0.33.1/src/algorithm/`): `ConcaveHull`, `Voronoi`,
`TriangulateDelaunay`/`TriangulateDelaunayUnconstrained`/`TriangulateSpade`/`TriangulateEarcut`, `HausdorffDistance`,
`FrechetDistance` (line_measures), `LineInterpolatePoint`, `LineLocatePoint`, `MinimumRotatedRect`, `ChaikinSmoothing`,
`Densify` (line_measures), `RemoveRepeatedPoints`, `InteriorPoint`, `Orient`, `Validation`, `MakeValid` (repair_polygon),
`KMeans`, `SimplifyVw*`, `GeodesicArea`, `Geodesic` metric space (`line_measures/metric_spaces/geodesic.rs`, with `Length`/`Distance`),
`Skew`, `AffineOps`/`AffineTransform`, `Haversine`/`Rhumb`/`Vincenty*`, `Stitch`. NOT in geo: line_merge, polygonize, offset_curve,
snap, minimum_bounding_circle, shortest_line (compose from `ClosestPoint`), simplify_coverage, normalize.

Ordered by user value; each batch roughly one PR (400-800 authored lines).

## Batch A - Constructors and extraction (high value, easy)
- Members: `from_xy` / `points_from_xy` (+ z), GDF `from_features`, `get_coordinates`, `count_geometries`, `count_interior_rings`, `get_geometry`, `interiors`, `extract_unique_points`, `collect`, `mean_coordinate`, `isna`/`notna`/`fillna`, `is_closed`, `is_ring`, `is_valid_reason`, `null_out_invalid`, `to_wkb(hex: true)`.
- Crates/traits: `geo-types` constructors, `CoordsIter`, `Validation` (reasons), existing `Native.unary` plumbing.
- Difficulty: low. Mostly Elixir plus small NIFs; unblocks real-world data entry (lat/lon columns).

## Batch B - Overlay-family: explode, clip, overlay, intersection_all (high value, medium-hard)
- Members: `explode` (Series+GDF, `index_parts`), `clip`, `clip_by_rect`, `intersection_all`, `overlay` (how: intersection/union/identity/symmetric_difference/difference), fix set-op lower-dimension and mixed-dimension gaps (🟡 in §5).
- Crates: `geo::BooleanOps` (i_overlay), `MultiPolygon` conversion, existing `sindex`, `Intersects`.
- Difficulty: medium-high (attribute assembly in Elixir, same pattern as `sjoin`; lower-dim results need `i_overlay` options or custom handling).

## Batch C - Linear referencing and linestring ops (medium-high value, easy-medium)
- Members: `interpolate`, `project`, `segmentize`, `remove_repeated_points`, `reverse`, `orient_polygons`, `is_ccw`, `line_merge`, `shortest_line`, `hausdorff_distance`, `frechet_distance`.
- Crates: `LineInterpolatePoint`, `LineLocatePoint`, `Densify`, `RemoveRepeatedPoints`, `Orient`, `HausdorffDistance`, `FrechetDistance`, `ClosestPoint`; `line_merge` own implementation (endpoint adjacency graph).
- Difficulty: low except `line_merge` and `shortest_line` (medium).
- Status: done (issue #38). Deviations: `hausdorff_distance` follows GEOS (vertex to whole geometry) instead of geo's vertex-to-vertex trait; `remove_repeated_points` with a tolerance and `interpolate` for multi-part lines are own code; `frechet_distance` is `LineString` only.

## Batch D - Affine completeness and per-row parameters (medium value, easy)
- Members: `skew`, `affine_transform`, per-row (column) offsets/angles/origins for `translate`/`scale`/`rotate`/`skew`, `wrap_longitude`, `is_geographic`.
- Crates: `geo::Skew`, `AffineOps`/`AffineTransform`, `proj4rs`.
- Difficulty: low.

## Batch E - Constructive geometry (medium value, medium)
- Members: `concave_hull`, `minimum_rotated_rectangle`, `minimum_bounding_circle`, `minimum_bounding_radius`, `maximum_inscribed_circle`, `make_grid`, `delaunay_triangles`, `constrained_delaunay_triangles`, `voronoi_polygons`, `sample_points`.
- Crates: `ConcaveHull`, `MinimumRotatedRect`, `Voronoi`/`TriangulateDelaunay`/`TriangulateSpade` (spade), `polylabel` (check geo feature flag) for inscribed circle, `rand` for sampling; bounding circle via own Welzl.
- Difficulty: medium (Voronoi/Delaunay are collection-valued: decide on output shape).

## Batch F - Geodesic measurement (medium value, medium)
- Members: `area_geodesic`, geodesic `length`, geodesic `distance` (a `method:` option), `GeoSeries.dwithin` geodesic variant optional.
- Crates: `geo::GeodesicArea`, `Geodesic` metric space with `Length`/`Distance` (geographiclib-rs already in the registry), or `Haversine`.
- Difficulty: medium (CRS gating: require geographic CRS; reproject projected inputs first).
- Status: done (issue #40). Deviations: WGS 84 only (other geographic datums are measured on WGS 84); nil CRS raises, projected CRS is reprojected to lon/lat; geo's unsigned area returns the ellipsoid complement for clockwise rings, so polygons are oriented first; geodesic distance for non-point pairs is an approximation (planar nearest points in lon/lat, then geodesic distance between them).

## Batch G - I/O breadth (high value for adoption, hard)
- Members: `read_feather`/`to_feather`, `from_arrow`/`to_arrow`, GeoParquet write options (compression, `with_covering`, `geoparquet_metadata`), `read_postgis`/`to_postgis`, Shapefile and GeoPackage via `read_file`/`write_file`, `list_layers`, `read_file_info`, string `to_json`.
- Crates: `geoarrow-array`, `arrow-ipc`, `geoparquet`, `postgrex` (Elixir side) or `sqlx`, pure-Rust `shapefile`, `geozero` (with-shp / with-gpkg features) or GDAL binding.
- Difficulty: high. Split into G1 (Arrow/Feather + parquet opts), G2 (PostGIS), G3 (Shapefile/GPKG).

## Batch H - Parity of comparison and testing (low-medium value, easy)
- Members: `geom_equals_exact`, `geom_equals_identical`, `contains_properly`, `relate_pattern`, `sindex.intersection`, `valid_query_predicates`, `cx`/bbox filter, `GeoFrame.Testing.assert_geoseries_equal` / `assert_geodataframe_equal`, GDF concat.
- Crates: `geo::Relate`, `ContainsProperly` (verify trait export), rstar.
- Difficulty: low. Testing helpers are valuable for downstream users.

## Batch I - Hard / low value (defer)
- Members: `simplify_coverage`, `snap`, `build_area`/`polygonize`, `shared_paths`, `offset_curve`, `minimum_clearance(_line)`, `normalize`, `set_precision`/`get_precision`, `is_valid_coverage`, `invalid_coverage_edges`, `z`/`m` dimension support (also `has_z`, `has_m`, `force_3d`).
- Crates: mostly none in `geo`; would need GEOS (C dependency) or custom algorithms; `geo-buf` for `offset_curve`.
- Difficulty: high. Z/M support is a storage-model change touching every NIF; treat as its own design.

## Not planned (⛔)
`plot` (use Kino map), `geocode`/`reverse_geocode`, pandas index alignment (`align=`), `iterfeatures`, Shapely object constructors, persistent `sindex` object, arbitrary-callback `transform`, deprecated `unary_union`.

---
Row status counts (sections 1-17, incl. a few cross-reference rows): {'✅': 102, '🟡': 30, '❌': 53, '⛔': 15}
