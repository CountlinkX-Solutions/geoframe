# Differential reference tests

`test/geo_frame/reference_test.exs` compares GeoFrame, operation by operation, with the
results of GeoPandas 1.2 / Shapely 2.2 (GEOS 3.14) / pyproj 3.8 on a fixed corpus of
geometries. **CI needs no Python**: the reference results are committed as JSON in
`test/reference/fixtures/`.

## Files

| File | Role |
|---|---|
| `generate.py` | Dev-only generator. Fixed corpus (points, lines, polygons with holes, multi geometries, collections, empties, an invalid bowtie, degenerate cases, touching/overlapping/disjoint pairs, lon/lat geometries near Miami and far apart). Computes every supported operation with GeoPandas/Shapely/pyproj. Deterministic: no randomness, sorted JSON keys, full-precision numbers, 12-decimal WKT. |
| `fixtures/*.json` | Generated output, one file per category: `measures`, `unary_predicates`, `binary_predicates`, `constructive`, `constructive_e` (batch E: hulls, bounding shapes, inscribed circle, constrained Delaunay), `collections` (batch E: `make_grid`, `delaunay_triangles`, `voronoi_polygons`; each case is a whole series because GeoPandas treats it as one point cloud, and the result is a list of geometries), `set_ops`, `affine`, `linear_referencing`, `crs`, `geodesic`, `io`, `parity_i` (batch I1: `is_simple`, `normalize`, `set_precision`, `get_precision`, one-feature `to_json` and `__geo_interface__` documents compared as decoded JSON), `parity_j` (batch I2: `offset_curve` over lines and polygons with round, mitre and bevel joins, `snap`, `shared_paths`, `minimum_clearance`, `minimum_clearance_line`; curves are compared by length and Hausdorff distance, shared paths in canonical form per half, clearance lines by equality or equal length, and `inf` is stored as `1.7976931348623157e308`), `parity_j_aggregate` (`polygonize` with `full=True` and `build_area`, whole-series cases compared as sets of shapes) and `parity_k` (batch I3: `is_valid_coverage`, `invalid_coverage_edges` and `simplify_coverage`; every case is a whole series from `sets`, rounded to a 0.005 grid when random, and geometries are compared vertex for vertex as the crate prints them; the random sets come from `fuzz_coverage.py`, which also writes the case files of the ignored Rust fuzz tests), `frames` (sjoin, sjoin_nearest, dissolve), `overlay` (frame-level `overlay`, `clip`, `explode`, `intersection_all`; rows are compared column by column, geometries as overlay results), plus `corpus.json` (the geometries and the tool versions). |
| `generate_io.py` | Dev-only generator of the I/O fixtures (batch G1): small Feather (lz4, zstd, uncompressed), GeoParquet (snappy, zstd, gzip, lz4, brotli, none, covering bbox, native GeoArrow, schema 1.0.0, several CRS) and Arrow IPC stream files written by GeoPandas 1.2 / pyarrow into `fixtures/io/`, plus `fixtures/io/manifest.json` with the expected CRS, WKT and attributes. `test/geo_frame/parity_g1_test.exs` reads them in CI without Python. Batch G3 adds `fixtures/io/vector/` (Shapefile incl. a zip, latin-1 `.cpg`, truncated field names, UTM lines, and two GeoPackages, one with two layers) written by GeoPandas 1.2 / pyogrio (GDAL 3.12), with `vector/manifest.json` also holding pyogrio's `read_info` for each; `test/geo_frame/parity_g3_test.exs` compares them. |
| `check_geoframe_io.py` | Dev-only cross-check in the other direction: reads files written by GeoFrame with `geopandas.read_feather`, `read_parquet` (including a `bbox=` read on covering files), `GeoDataFrame.from_arrow(pa.ipc.open_stream(...))` and checks CRS (as pyproj), geometry, attributes and the GeoArrow extension metadata. Since batch G3 it also reads Shapefile (plain and zipped, several CRS, encodings, truncated names) and GeoPackage files (several layers, append, no index, mixed geometry types, dates) with `geopandas.read_file`, checks SQLite `integrity_check`, the GPKG application id, pyogrio's layer list and feature count, and that a `bbox=` read through the rtree agrees with a full scan. Driven by `scripts/write_io_samples.exs` (writes 48 files with GeoFrame) through `scripts/crosscheck_io.sh`. |
| `known_deviations.exs` | Machine-readable list of known differences (op, case ids, reason, link to `docs/parity.md#reference-testing`). |
| `../support/reference.ex` | The comparison engine (test-only module `GeoFrame.Reference`). |

## Comparison rules

* Numbers: relative and absolute tolerance 1e-9. Geodesic measures: 1e-6 (pyproj `Geod` against
  `geographiclib-rs`). `to_crs`: 1e-6 projected units, 1e-9 degrees (proj4rs against PROJ).
* Geometries: topological equality (`geom_equals`); exact vertex equality within 1e-9
  (`geom_equals_exact`) where order matters (`reverse`, `interpolate`, affine, `simplify`,
  `segmentize`, `to_crs`, ...). Overlay results also pass when within 1e-6 (Hausdorff distance and
  area), because `geo`'s boolean operations snap intersection points to an integer grid.
  `buffer` compares the geometry type and the area within 2 %, since vertex counts differ.
  `representative_point` only has to lie in the input.
* Batch E: hulls, rotated rectangles and bounding circles must be topologically equal or within 1e-9
  (Hausdorff distance and area). Triangulations must be the same set of triangles; when cocircular points
  allow several triangulations the same count of triangles over the same union (or, for edges, edge sets that
  differ only by flipped diagonals) also agrees. Voronoi cells and sides must each have a counterpart within
  1e-9. `maximum_inscribed_circle` is a search, so the radius is compared within its tolerance and the center
  must lie in the input. `make_grid` is compared element by element in order. `sample_points` is random and
  has no fixture: it is tested by properties in `test/geo_frame/parity_e_test.exs`.
* Booleans and strings: exact.
* Null and empty inputs: GeoFrame returns `nil` where GeoPandas returns `False`/`0`/`NaN`; that
  whole class is one documented deviation.

## Known deviations policy

A case listed in `known_deviations.exs` is asserted to **still deviate**. When a deviation is
fixed the suite fails with the list of stale entries, so the entry (and its note in
`docs/parity.md`) must be removed. A differing case that is not listed fails the suite.

## Regenerating

```sh
python -m venv ~/.venvs/geopandas-ref
~/.venvs/geopandas-ref/bin/pip install geopandas==1.2.0 shapely==2.2.0 pyproj==3.8.0 pyarrow pyogrio
~/.venvs/geopandas-ref/bin/python -I test/reference/generate.py
mix test test/geo_frame/reference_test.exs
```

I/O fixtures (batch G1) and the GeoFrame-to-GeoPandas cross-check:

```sh
~/.venvs/geopandas-ref/bin/python -I test/reference/generate_io.py   # fixtures/io/*, deterministic
mix test test/geo_frame/parity_g1_test.exs
scripts/crosscheck_io.sh   # GeoFrame writes, GeoPandas reads; set GEOPANDAS_PYTHON to override the venv
```

Run twice and the fixtures are byte-identical. A summary table (cases, agree, documented
deviations) is printed with `REFERENCE_REPORT=1 mix test test/geo_frame/reference_test.exs`.
Upgrading GeoPandas/Shapely changes the pinned versions asserted by the suite; regenerate,
rerun, and reclassify any new difference (fix it, or add it to `known_deviations.exs`).
