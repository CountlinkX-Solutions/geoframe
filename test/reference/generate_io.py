"""Dev-only generator for the GeoPandas-written I/O fixtures.

Writes small Feather, GeoParquet and Arrow IPC stream files with GeoPandas 1.2 / pyarrow
into test/reference/fixtures/io/ plus manifest.json (the expected CRS, WKT and attributes of
each file), and Shapefile / GeoPackage files with GeoPandas 1.2 / pyogrio (GDAL) into
test/reference/fixtures/io/vector/ plus vector/manifest.json. CI never runs this file:
test/geo_frame/parity_g1_test.exs and parity_g3_test.exs read the committed files and compare
them with the manifests.

Run:  ~/.venvs/geopandas-ref/bin/python -I test/reference/generate_io.py
"""

import json
import os
import shutil
import zipfile

import geopandas as gpd
import pandas as pd
import pyarrow as pa
import shapely
import warnings
from shapely import wkt as swkt

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "fixtures", "io")

MIXED = [
    "POINT (1 2)",
    "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0), (1 1, 2 1, 2 2, 1 1))",
    None,
    "LINESTRING (0 0, 3 4, 6 4)",
    "MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))",
]
POINTS = ["POINT (0 0)", "POINT (1 1)", None, "POINT (3 3)", "POINT (4 4)"]
POLYS = [
    "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))",
    "MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))",
    None,
    "POLYGON ((5 5, 6 5, 6 6, 5 5))",
    "POLYGON ((7 7, 9 7, 9 9, 7 7))",
]
ATTRS = {
    "id": [1, 2, 3, None, 5],
    "name": ["a", "b", None, "d", "e"],
    "score": [1.5, None, 3.25, 4.0, -5.5],
    "flag": [True, False, None, True, False],
}


def frame(wkts, crs):
    geoms = [swkt.loads(w) if w else None for w in wkts]
    data = {k: v for k, v in ATTRS.items()}
    df = gpd.GeoDataFrame(data, geometry=geoms, crs=crs)
    df["id"] = df["id"].astype("Int64")
    df["flag"] = df["flag"].astype("boolean")
    return df


def expected(df):
    return {
        "crs": df.crs.to_string() if df.crs else None,
        "wkt": [None if g is None else shapely.to_wkt(g, rounding_precision=-1) for g in df.geometry],
        "columns": {c: [None if pd.isna(v) else (v.item() if hasattr(v, "item") else v)
                        for v in df[c].tolist()] for c in ATTRS},
    }


# EPSG codes whose PROJJSON GeoFrame writes into GeoParquet metadata (issue #88): geographic,
# UTM, Web Mercator and projected CRS from the bundled table (several projection methods).
PROJJSON_CODES = [4326, 4258, 4269, 3857, 32633, 32760, 3035, 2154, 27700, 5070, 3031, 2056,
                  3395, 25832, 6933]


def generate_crs_projjson():
    """pyproj's own PROJJSON for each code, to compare with what GeoFrame writes."""
    import pyproj

    docs = {str(c): pyproj.CRS.from_epsg(c).to_json_dict() for c in PROJJSON_CODES}
    with open(os.path.join(OUT, "crs_projjson.json"), "w") as handle:
        json.dump({"pyproj": pyproj.__version__, "crs": docs}, handle, indent=1, sort_keys=True)
        handle.write("\n")


def main():
    os.makedirs(OUT, exist_ok=True)
    for name in os.listdir(OUT):
        if os.path.isfile(os.path.join(OUT, name)):
            os.remove(os.path.join(OUT, name))
    manifest = {"pyarrow": pa.__version__, "geopandas": gpd.__version__, "files": {}}

    def record(file, df, **extra):
        manifest["files"][file] = {**expected(df), **extra}

    mixed = frame(MIXED, "EPSG:4326")
    mixed_3857 = frame(MIXED, "EPSG:3857")
    mixed_nocrs = frame(MIXED, None)
    points = frame(POINTS, "EPSG:4326")
    polys = frame(POLYS, "EPSG:4326")

    # Feather (Arrow IPC file): default compression is lz4, plus uncompressed and zstd.
    for file, df, kw in [
        ("wkb_lz4.feather", mixed, {}),
        ("wkb_uncompressed.feather", mixed, {"compression": "uncompressed"}),
        ("wkb_zstd.feather", mixed, {"compression": "zstd"}),
        ("wkb_3857.feather", mixed_3857, {}),
        ("wkb_nocrs.feather", mixed_nocrs, {}),
    ]:
        df.to_feather(os.path.join(OUT, file), **kw)
        record(file, df)

    # GeoParquet: every compression, covering bbox, native encodings, schema 1.0.0.
    for comp in ["snappy", "zstd", "gzip", "lz4", "brotli", "none"]:
        file = f"wkb_{comp}.parquet"
        mixed.to_parquet(os.path.join(OUT, file), compression=comp)
        record(file, mixed)
    mixed.to_parquet(os.path.join(OUT, "wkb_covering.parquet"), write_covering_bbox=True, row_group_size=2)
    record("wkb_covering.parquet", mixed, covering=True)
    mixed.to_parquet(os.path.join(OUT, "wkb_v1_0_0.parquet"), schema_version="1.0.0")
    record("wkb_v1_0_0.parquet", mixed)
    mixed_3857.to_parquet(os.path.join(OUT, "wkb_3857.parquet"))
    record("wkb_3857.parquet", mixed_3857)
    mixed_nocrs.to_parquet(os.path.join(OUT, "wkb_nocrs.parquet"))
    record("wkb_nocrs.parquet", mixed_nocrs)
    # Projected CRS other than Web Mercator and UTM (issue #88): GeoFrame must read them.
    for code in (3035, 32633):
        df = frame(MIXED, f"EPSG:{code}")
        df.to_parquet(os.path.join(OUT, f"wkb_{code}.parquet"))
        record(f"wkb_{code}.parquet", df)
    generate_crs_projjson()
    points.to_parquet(os.path.join(OUT, "geoarrow_points.parquet"), geometry_encoding="geoarrow")
    record("geoarrow_points.parquet", points)
    points.to_parquet(
        os.path.join(OUT, "geoarrow_points_covering.parquet"),
        geometry_encoding="geoarrow", write_covering_bbox=True, row_group_size=2,
    )
    record("geoarrow_points_covering.parquet", points, covering=True)
    # Native multipolygon columns read back as MultiPolygon, so the expectation is GeoPandas's
    # own read of the file rather than the input frame.
    polys.to_parquet(os.path.join(OUT, "geoarrow_polygons.parquet"), geometry_encoding="geoarrow")
    record("geoarrow_polygons.parquet", gpd.read_parquet(os.path.join(OUT, "geoarrow_polygons.parquet")))

    # Arrow IPC streams of GeoDataFrame.to_arrow().
    def stream(file, table):
        with pa.OSFile(os.path.join(OUT, file), "wb") as sink:
            with pa.ipc.new_stream(sink, table.schema) as writer:
                writer.write_table(table)

    stream("wkb.arrows", pa.table(mixed.to_arrow()))
    record("wkb.arrows", mixed)
    stream("geoarrow_points_interleaved.arrows", pa.table(points.to_arrow(geometry_encoding="geoarrow")))
    record("geoarrow_points_interleaved.arrows", points)
    poly_table = pa.table(polys.to_arrow(geometry_encoding="geoarrow", interleaved=False))
    stream("geoarrow_polygons_separated.arrows", poly_table)
    record("geoarrow_polygons_separated.arrows", gpd.GeoDataFrame.from_arrow(poly_table))

    with open(os.path.join(OUT, "manifest.json"), "w") as handle:
        json.dump(manifest, handle, indent=1, sort_keys=True)
        handle.write("\n")

    generate_vector()
    generate_zm()
    generate_multi_geometry()


def generate_multi_geometry():
    """GeoParquet files with several geometry columns (issue #61), written by GeoPandas 1.2.

    Own directory and manifest so regenerating them leaves the other fixtures alone. Run only
    this with: python -I -c "import runpy; runpy.run_path('test/reference/generate_io.py',
    run_name='gen')['generate_multi_geometry']()"
    """
    out = os.path.join(OUT, "multi")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    base = gpd.GeoDataFrame(
        {"id": pd.array([1, 2, 3], dtype="Int64"), "name": ["a", "b", None]},
        geometry=gpd.GeoSeries.from_wkt(["POINT (0 0)", "POINT (1 1)", None], crs=4326),
    )
    base["footprint"] = gpd.GeoSeries.from_wkt(
        ["POLYGON ((0 0, 1 0, 1 1, 0 0))", None, "POLYGON ((5 5, 6 5, 6 6, 5 5))"], crs=3857
    )
    base["centroid"] = gpd.GeoSeries.from_wkt(
        ["POINT (5 5)", "POINT (6 6)", "POINT (7 7)"], crs=None
    )
    manifest = {"geopandas": gpd.__version__, "files": {}}
    # The native encoding needs one geometry kind per column, which these columns have.
    for file, kw in [("two_wkb.parquet", {}), ("two_native.parquet", {"geometry_encoding": "geoarrow"}),
                     ("two_covering.parquet", {"write_covering_bbox": True})]:
        base.to_parquet(os.path.join(out, file), **kw)
        back = gpd.read_parquet(os.path.join(out, file))
        manifest["files"][file] = {
            "primary": "geometry",
            "geometry_columns": [c for c in back.columns if str(back[c].dtype) == "geometry"],
            "id": [1, 2, 3],
            "name": ["a", "b", None],
            "columns": {
                c: {
                    "crs": back[c].crs.to_string() if back[c].crs else None,
                    "wkt": [None if g is None else shapely.to_wkt(g, rounding_precision=-1) for g in back[c]],
                }
                for c in back.columns if str(back[c].dtype) == "geometry"
            },
        }
    with open(os.path.join(out, "manifest.json"), "w") as handle:
        json.dump(manifest, handle, indent=1, sort_keys=True)
        handle.write("\n")


def plain(value):
    if value is None or (not isinstance(value, str) and pd.isna(value)):
        return None
    if isinstance(value, pd.Timestamp):
        return value.strftime("%Y-%m-%dT%H:%M:%S")
    return value.item() if hasattr(value, "item") else value


def generate_vector():
    """Shapefile and GeoPackage fixtures written by GeoPandas 1.2 / pyogrio (GDAL)."""
    import pyogrio

    out = os.path.join(OUT, "vector")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    manifest = {"geopandas": gpd.__version__, "pyogrio": pyogrio.__version__,
                "gdal": pyogrio.__gdal_version__[0:3], "files": {}}

    def record(name, df, **extra):
        manifest["files"][name] = {
            "crs": df.crs.to_string() if df.crs else None,
            "wkt": [None if g is None else shapely.to_wkt(g, rounding_precision=-1) for g in df.geometry],
            "columns": {c: [plain(v) for v in df[c].tolist()] for c in df.columns if c != "geometry"},
            **extra,
        }

    def attrs(n, **more):
        base = {
            "id": [1, 2, 3, 4, 5][:n],
            "name": ["a", "b c", None, "d", "e"][:n],
            "score": [1.5, None, 3.25, 4.0, -5.5][:n],
            "ok": [True, False, True, False, True][:n],
        }
        base.update(more)
        return base

    pts = [swkt.loads(w) if w else None for w in ["POINT (1 2)", "POINT (-80.5 25.25)", None, "POINT (3 4)", "POINT (0 0)"]]
    points = gpd.GeoDataFrame(attrs(5), geometry=pts, crs="EPSG:4326")
    points.to_file(os.path.join(out, "points.shp"))
    record("points.shp", gpd.read_file(os.path.join(out, "points.shp")),
           info=info_of(os.path.join(out, "points.shp")))

    polys = gpd.GeoDataFrame(
        {"id": [1, 2, 3]},
        geometry=[swkt.loads(w) if w else None for w in [
            "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0), (1 1, 2 1, 2 2, 1 1))",
            "MULTIPOLYGON (((10 10, 10 11, 11 11, 10 10)), ((20 20, 20 21, 21 21, 20 20)))",
            None]],
        crs="EPSG:3857",
    )
    tmp = os.path.join(out, "_polys")
    os.makedirs(tmp)
    polys.to_file(os.path.join(tmp, "polygons.shp"))
    with zipfile.ZipFile(os.path.join(out, "polygons.zip"), "w", zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(os.listdir(tmp)):
            archive.write(os.path.join(tmp, name), name)
    zipped = gpd.read_file(os.path.join(out, "polygons.zip"))
    record("polygons.zip", zipped, info=info_of(os.path.join(out, "polygons.zip")))
    shutil.rmtree(tmp)

    lines = gpd.GeoDataFrame(
        {"id": [1, 2]},
        geometry=[swkt.loads("LINESTRING (500000 4000000, 500100 4000100, 500200 4000050)"),
                  swkt.loads("MULTILINESTRING ((0 0, 1 1), (5 5, 6 6, 7 5))")],
        crs="EPSG:32617",
    )
    lines.to_file(os.path.join(out, "lines_utm.shp"))
    record("lines_utm.shp", gpd.read_file(os.path.join(out, "lines_utm.shp")))

    latin = gpd.GeoDataFrame({"name": ["café", "ñandú", None]},
                             geometry=[swkt.loads("POINT (1 1)"), swkt.loads("POINT (2 2)"), swkt.loads("POINT (3 3)")],
                             crs="EPSG:4326")
    latin.to_file(os.path.join(out, "latin1.shp"), encoding="ISO-8859-1")
    record("latin1.shp", gpd.read_file(os.path.join(out, "latin1.shp")), cpg="ISO-8859-1")

    long_names = gpd.GeoDataFrame(
        {"population_total": [1, 2], "population_urban": [3, 4], "population_rural": [5, 6], "short": [7, 8]},
        geometry=[swkt.loads("POINT (1 1)"), swkt.loads("POINT (2 2)")], crs="EPSG:4326")
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        long_names.to_file(os.path.join(out, "truncated.shp"))
    record("truncated.shp", gpd.read_file(os.path.join(out, "truncated.shp")))

    when = pd.to_datetime(["2020-01-02 03:04:05", None, "2021-03-04 00:00:00"])
    gp_pts = gpd.GeoDataFrame(
        {"id": pd.array([1, None, 3], dtype="Int64"), "name": ["a", None, "c"], "v": [1.5, 2.5, None],
         "flag": [True, False, True], "when": when},
        geometry=[swkt.loads("POINT (1 2)"), None, swkt.loads("POINT (3 4)")], crs="EPSG:4326")
    gp_polys = gpd.GeoDataFrame(
        {"k": [1, 2]},
        geometry=[swkt.loads("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
                  swkt.loads("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))")],
        crs="EPSG:3857")
    multi = os.path.join(out, "multi.gpkg")
    gp_pts.to_file(multi, layer="pts", driver="GPKG")
    gp_polys.to_file(multi, layer="polys", driver="GPKG")
    manifest["layers"] = {"multi.gpkg": pyogrio.list_layers(multi).tolist()}
    record("multi.gpkg:pts", gpd.read_file(multi, layer="pts"), info=info_of(multi, "pts"))
    record("multi.gpkg:polys", gpd.read_file(multi, layer="polys"), info=info_of(multi, "polys"))

    single = os.path.join(out, "single.gpkg")
    gpd.GeoDataFrame({"n": [1, 2, 3]}, geometry=[swkt.loads("LINESTRING (0 0, 1 1)"), None,
                     swkt.loads("LINESTRING (2 2, 3 5)")], crs="EPSG:4269").to_file(single, layer="roads", driver="GPKG")
    record("single.gpkg", gpd.read_file(single), info=info_of(single))

    # A null row is a feature without a geometry (issue #78). GDAL refuses to write one into an
    # indexed FlatGeobuf file, so the fixture has no spatial index.
    nulls = os.path.join(out, "nulls.fgb")
    gpd.GeoDataFrame(
        {"id": [1, 2, 3], "name": ["a", None, "c"]},
        geometry=[swkt.loads("POINT (1 2)"), None, swkt.loads("POINT (3 4)")], crs="EPSG:4326",
    ).to_file(nulls, driver="FlatGeobuf", SPATIAL_INDEX="NO")
    record("nulls.fgb", gpd.read_file(nulls))

    shrink(multi)
    shrink(single)

    with open(os.path.join(out, "manifest.json"), "w") as handle:
        json.dump(manifest, handle, indent=1, sort_keys=True)
        handle.write("\n")


# -- Z and M (batch Z4) -------------------------------------------------------------------


def wkts_of(geoms):
    return [None if g is None else shapely.to_wkt(g, rounding_precision=-1) for g in geoms]


def coords_zm(geom):
    """Flat (x, y, z, m) rows of a geometry with NaN for the missing ordinates."""
    import numpy as np
    return shapely.get_coordinates(geom, include_z=True, include_m=True) if hasattr(shapely, "has_m") else np.array([])


def shp_record(shape_type, shape):
    """One shapefile record body (without the 8-byte record header) written from the ESRI
    specification: `shape` is None (null shape) or a list of parts of (x, y, z, m) tuples; NaN
    in z or m is written as no data (-1e39 for m). Z types carry an M section only when
    `m` values exist."""
    import math
    import struct

    base = shape_type % 10 if shape_type < 30 else shape_type
    kind = {1: "point", 8: "multipoint", 3: "line", 5: "polygon"}[base]
    z_type = 10 <= shape_type < 20
    m_type = 20 <= shape_type < 30
    if shape is None:
        return struct.pack("<i", 0)
    points = [p for part in shape for p in part]
    has_m = any(not math.isnan(p[3]) for p in points)

    def nodata(m):
        return -1e39 if math.isnan(m) else m

    out = struct.pack("<i", shape_type)
    if kind == "point":
        x, y, z, m = points[0]
        out += struct.pack("<2d", x, y)
        if z_type:
            out += struct.pack("<d", z)
        if m_type or (z_type and has_m):
            out += struct.pack("<d", nodata(m))
        return out
    xs, ys = [p[0] for p in points], [p[1] for p in points]
    out += struct.pack("<4d", min(xs), min(ys), max(xs), max(ys))
    if kind == "multipoint":
        out += struct.pack("<i", len(points))
    else:
        out += struct.pack("<2i", len(shape), len(points))
        start = 0
        for part in shape:
            out += struct.pack("<i", start)
            start += len(part)
    for p in points:
        out += struct.pack("<2d", p[0], p[1])

    def section(values):
        valid = [v for v in values if not math.isnan(v)]
        lo, hi = (min(valid), max(valid)) if valid else (0.0, 0.0)
        return struct.pack("<2d", lo, hi) + b"".join(struct.pack("<d", nodata(v)) for v in values)

    if z_type:
        out += section([p[2] for p in points])
    if m_type or (z_type and has_m):
        out += section([p[3] for p in points])
    return out


def write_shp(path, shape_type, shapes):
    """Writes `path` (.shp) and its .shx from the specification, replacing GDAL's files."""
    import math
    import struct

    records, index = b"", b""
    for number, shape in enumerate(shapes, start=1):
        body = shp_record(shape_type, shape)
        index += struct.pack(">2i", (100 + len(records)) // 2, len(body) // 2)
        records += struct.pack(">2i", number, len(body) // 2) + body
    points = [p for shape in shapes if shape for part in shape for p in part]

    def span(values):
        valid = [v for v in values if not math.isnan(v)]
        return (min(valid), max(valid)) if valid else (0.0, 0.0)

    ranges = (
        span([p[0] for p in points]) + span([p[1] for p in points])
        if points else (0.0,) * 4
    )
    box = (ranges[0], ranges[2], ranges[1], ranges[3])
    z = span([p[2] for p in points]) if 10 <= shape_type < 20 else (0.0, 0.0)
    m = span([p[3] for p in points]) if shape_type >= 11 else (0.0, 0.0)

    def header(length):
        return struct.pack(">i20xi", 9994, length // 2) + struct.pack("<2i", 1000, shape_type) + struct.pack("<8d", *box, *z, *m)

    with open(path, "wb") as handle:
        handle.write(header(100 + len(records)) + records)
    with open(path[:-4] + ".shx", "wb") as handle:
        handle.write(header(100 + len(index)) + index)


def read_zm_wkt(path, layer=None):
    """`(geometries, {"id": ..., "name": ...})` of any vector file as GDAL reports them, in file
    order, M included (pyogrio's Arrow path exports ISO WKB with Z and M)."""
    import pyogrio

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        meta, table = pyogrio.read_arrow(path, layer=layer)
    name = meta["geometry_name"] or "wkb_geometry"
    geoms = [None if b.as_py() is None else shapely.from_wkb(b.as_py()) for b in table.column(name)]
    return geoms, {c: table.column(c).to_pylist() for c in ("id", "name")}


def generate_zm():
    """Z and M fixtures. Z files are written by GeoPandas / pyogrio (GDAL); pyogrio cannot write
    M, so the M and ZM files are built from the ESRI and GeoPackage specifications in this
    file and then checked by reading them back through GDAL (the expectation is GDAL's read)."""
    import math
    import sqlite3
    import struct

    import numpy as np
    import pyogrio

    out = os.path.join(OUT, "zm")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    manifest = {"geopandas": gpd.__version__, "pyogrio": pyogrio.__version__,
                "gdal": pyogrio.__gdal_version__[0:3], "files": {}}

    def frame(wkts, crs="EPSG:4326"):
        geoms = [swkt.loads(w) if w else None for w in wkts]
        return gpd.GeoDataFrame({"id": list(range(1, len(wkts) + 1)),
                                 "name": [chr(97 + i) for i in range(len(wkts))]},
                                geometry=geoms, crs=crs)

    def record(key, path, layer=None, **extra):
        geoms, columns = read_zm_wkt(path, layer)
        manifest["files"][key] = {
            "crs": "EPSG:4326",
            "wkt": wkts_of(geoms),
            "has_z": [False if g is None else bool(shapely.has_z(g)) for g in geoms],
            "has_m": [False if g is None else bool(shapely.has_m(g)) for g in geoms],
            "columns": columns,
            **extra,
        }

    # Z, written by GDAL ---------------------------------------------------------------------
    z_frames = {
        "points_z.shp": ["POINT Z (1 2 3)", "POINT Z (-80.5 25.25 -1.5)", None],
        "lines_z.shp": ["LINESTRING Z (0 0 1, 1 1 2, 2 0 3)", "MULTILINESTRING Z ((0 0 1, 1 1 2), (5 5 3, 6 6 4))"],
        "polygons_z.shp": [
            "POLYGON Z ((0 0 1, 0 4 2, 4 4 3, 4 0 4, 0 0 1), (1 1 5, 2 1 5, 2 2 5, 1 1 5))",
            "MULTIPOLYGON Z (((10 10 7, 10 11 7, 11 11 7, 10 10 7)), ((20 20 8, 20 21 8, 21 21 8, 20 20 8)))",
        ],
        "multipoints_z.shp": ["MULTIPOINT Z ((0 0 1), (1 1 2))", "MULTIPOINT Z ((5 5 3), (6 6 4), (7 7 5))"],
    }
    for name, wkts in z_frames.items():
        frame(wkts).to_file(os.path.join(out, name))
        record(name, os.path.join(out, name), kind="shapefile", shape_type=pyogrio.read_info(os.path.join(out, name))["geometry_type"])

    geojson_rows = [
        "POINT Z (1 2 3)", "POINT (4 5)", None, "LINESTRING Z (0 0 1, 1 1 2)",
        "POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))",
        "GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 1, 1 1 2))",
    ]
    frame(geojson_rows).to_file(os.path.join(out, "z.geojson"), driver="GeoJSON")
    record("z.geojson", os.path.join(out, "z.geojson"), kind="geojson")

    fgb_files = {
        "points_z.fgb": ["POINT Z (1 2 3)", "POINT Z (4 5 6)", "POINT Z (-80.5 25.25 -1.5)"],
        "polygons_z.fgb": ["POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))", "POLYGON Z ((10 10 7, 11 10 7, 11 11 7, 10 10 7))"],
    }
    for name, wkts in fgb_files.items():
        frame(wkts).to_file(os.path.join(out, name), driver="FlatGeobuf")
        record(name, os.path.join(out, name), kind="flatgeobuf")

    gpkg = os.path.join(out, "z.gpkg")
    frame(["POINT Z (1 2 3)", "POINT Z (4 5 6)", None]).to_file(gpkg, layer="pts", driver="GPKG")
    frame(["POINT Z (1 2 3)", "POINT (4 5)"]).to_file(gpkg, layer="mixed", driver="GPKG")
    shrink(gpkg)
    flags = {}
    connection = sqlite3.connect(gpkg)
    for table, name, z, m in connection.execute("SELECT table_name, geometry_type_name, z, m FROM gpkg_geometry_columns"):
        flags[table] = {"geometry_type_name": name, "z": z, "m": m}
    connection.close()
    for layer in ("pts", "mixed"):
        record(f"z.gpkg:{layer}", gpkg, layer, kind="gpkg", flags=flags[layer])

    # GeoParquet and Arrow with Z ------------------------------------------------------------
    zpoints = frame(["POINT Z (1 2 3)", "POINT Z (4 5 6)", None])
    zmixed = frame(["POINT Z (1 2 3)", "POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))", "LINESTRING (0 0, 1 1)", None])
    zpolys = frame(["POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))",
                    "MULTIPOLYGON Z (((10 10 7, 11 10 7, 11 11 7, 10 10 7)), ((20 20 8, 21 20 8, 21 21 8, 20 20 8)))", None])

    def expected_file(key, df, **extra):
        manifest["files"][key] = {
            "crs": "EPSG:4326",
            "wkt": wkts_of(df.geometry),
            "has_z": [False if g is None else bool(shapely.has_z(g)) for g in df.geometry],
            "has_m": [False] * len(df),
            "columns": {"id": df["id"].tolist(), "name": df["name"].tolist()},
            **extra,
        }

    zmixed.to_parquet(os.path.join(out, "z_wkb.parquet"))
    expected_file("z_wkb.parquet", zmixed, kind="parquet",
                  geometry_types=sorted(json.loads(__import__("pyarrow.parquet", fromlist=["x"]).read_metadata(os.path.join(out, "z_wkb.parquet")).metadata[b"geo"])["columns"]["geometry"]["geometry_types"]))
    zpoints.to_parquet(os.path.join(out, "z_geoarrow_points.parquet"), geometry_encoding="geoarrow")
    expected_file("z_geoarrow_points.parquet", zpoints, kind="parquet")
    zpolys.to_parquet(os.path.join(out, "z_geoarrow_polygons.parquet"), geometry_encoding="geoarrow")
    expected_file("z_geoarrow_polygons.parquet", gpd.read_parquet(os.path.join(out, "z_geoarrow_polygons.parquet")), kind="parquet")
    zpoints.to_feather(os.path.join(out, "z_wkb.feather"))
    expected_file("z_wkb.feather", zpoints, kind="feather")
    table = pa.table(zpoints.to_arrow(geometry_encoding="geoarrow"))
    with pa.OSFile(os.path.join(out, "z_geoarrow_points.arrows"), "wb") as sink:
        with pa.ipc.new_stream(sink, table.schema) as writer:
            writer.write_table(table)
    expected_file("z_geoarrow_points.arrows", zpoints, kind="arrows")

    # M and ZM, from the specifications --------------------------------------------------------
    nan = float("nan")

    def pts(*rows):
        return [[(x, y, z, m) for (x, y, z, m) in rows]]

    shapes = {
        "points_m.shp": (21, [[[(1, 2, nan, 10)]], [[(3, 4, nan, 20.5)]], None]),
        "lines_m.shp": (23, [pts((0, 0, nan, 1), (1, 1, nan, 2), (2, 0, nan, 3)),
                             [[(0, 0, nan, 1), (1, 1, nan, 2)], [(5, 5, nan, 3), (6, 6, nan, 4)]]]),
        "polygons_m.shp": (25, [[[(0, 0, nan, 1), (0, 4, nan, 2), (4, 4, nan, 3), (4, 0, nan, 4), (0, 0, nan, 1)],
                                 [(1, 1, nan, 5), (2, 1, nan, 5), (2, 2, nan, 5), (1, 1, nan, 5)]]]),
        "multipoints_m.shp": (28, [pts((0, 0, nan, 1), (1, 1, nan, 2))]),
        "points_zm.shp": (11, [[[(1, 2, 3, 10)]], [[(3, 4, 5, 20.5)]]]),
        "lines_zm.shp": (13, [pts((0, 0, 1, 10), (1, 1, 2, 11), (2, 0, 3, 12)),
                              [[(0, 0, 1, 1), (1, 1, 2, 2)], [(5, 5, 3, 3), (6, 6, 4, 4)]]]),
        "polygons_zm.shp": (15, [[[(0, 0, 1, 5), (0, 4, 2, 6), (4, 4, 3, 7), (4, 0, 4, 8), (0, 0, 1, 5)]]]),
    }
    for name, (shape_type, rows) in shapes.items():
        target = os.path.join(out, name)
        # GDAL makes the .dbf, .prj and .cpg; the .shp and .shx are then replaced.
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            frame(["POINT (0 0)"] * len(rows)).to_file(target)
        write_shp(target, shape_type, rows)
        geoms, _ = read_zm_wkt(target)
        for geom, shape in zip(geoms, rows):
            if shape is None:
                assert geom is None, name
                continue
            want = np.array([p for part in shape for p in part], dtype=float)
            got = shapely.get_coordinates(geom, include_z=True, include_m=True)
            if want.shape != got.shape:
                # GDAL drops the all-NaN axes for 2D+M and Z-only reads
                got = got[:, : want.shape[1]]
            assert np.allclose(got[:, :2], want[:, :2]), (name, got, want)
            assert np.allclose(np.nan_to_num(got[:, -1], nan=-1), np.nan_to_num(want[:, -1], nan=-1)), (name, got, want)
        record(name, target, kind="shapefile")

    # GeoPackage with M: GDAL makes the container, the blobs and flags follow the spec.
    mgpkg = os.path.join(out, "m.gpkg")
    frame(["POINT (1 2)", "POINT (3 4)"]).to_file(mgpkg, layer="pts_m", driver="GPKG")
    frame(["POINT (1 2)", "POINT (3 4)"]).to_file(mgpkg, layer="pts_zm", driver="GPKG")
    connection = sqlite3.connect(mgpkg)
    # the R-tree triggers call these; envelopes are the XY ones written below
    connection.create_function("ST_IsEmpty", 1, lambda blob: 0)
    for fn, offset in [("ST_MinX", 8), ("ST_MaxX", 16), ("ST_MinY", 24), ("ST_MaxY", 32)]:
        connection.create_function(fn, 1, lambda blob, o=offset: struct.unpack_from("<d", blob, o)[0])

    def gp_blob(wkb, x, y):
        return b"GP\x00\x03" + struct.pack("<i", 4326) + struct.pack("<4d", x, x, y, y) + wkb

    for layer, code, rows, flag in [
        ("pts_m", 2001, [(1, 2, None, 10.0), (3, 4, None, 20.5)], (0, 1)),
        ("pts_zm", 3001, [(1, 2, 3.0, 10.0), (3, 4, 5.0, 20.5)], (1, 1)),
    ]:
        geometry_column = connection.execute(
            "SELECT column_name FROM gpkg_geometry_columns WHERE table_name = ?", (layer,)).fetchone()[0]
        for fid, (x, y, z, m) in enumerate(rows, start=1):
            values = [x, y] + ([z] if z is not None else []) + [m]
            wkb = struct.pack("<BI", 1, code) + struct.pack(f"<{len(values)}d", *values)
            connection.execute(f'UPDATE "{layer}" SET "{geometry_column}" = ? WHERE fid = ?', (gp_blob(wkb, x, y), fid))
        connection.execute("UPDATE gpkg_geometry_columns SET z = ?, m = ? WHERE table_name = ?", (*flag, layer))
    connection.commit()
    connection.close()
    shrink(mgpkg)
    for layer in ("pts_m", "pts_zm"):
        geoms, _ = read_zm_wkt(mgpkg, layer)
        assert all(shapely.has_m(g) for g in geoms), layer
        record(f"m.gpkg:{layer}", mgpkg, layer, kind="gpkg")

    with open(os.path.join(out, "manifest.json"), "w") as handle:
        json.dump(manifest, handle, indent=1, sort_keys=True)
        handle.write("\n")


def shrink(path):
    """Smaller SQLite pages keep the committed GeoPackages small; the file stays valid."""
    import sqlite3
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA page_size = 1024")
    connection.execute("VACUUM")
    connection.close()


def info_of(path, layer=None):
    import pyogrio
    info = pyogrio.read_info(path, layer=layer)
    return {
        "features": int(info["features"]),
        "geometry_type": info["geometry_type"],
        "crs": info["crs"],
        "fields": [str(f) for f in info["fields"]],
        "total_bounds": None if info["total_bounds"] is None else [float(v) for v in info["total_bounds"]],
        "driver": info["driver"],
    }


if __name__ == "__main__":
    main()
