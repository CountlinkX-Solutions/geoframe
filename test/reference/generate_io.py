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

    shrink(multi)
    shrink(single)

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
