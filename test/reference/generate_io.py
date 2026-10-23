"""Dev-only generator for the GeoPandas-written I/O fixtures.

Writes small Feather, GeoParquet and Arrow IPC stream files with GeoPandas 1.2 / pyarrow
into test/reference/fixtures/io/ plus manifest.json (the expected CRS, WKT and attributes of
each file). CI never runs this file: test/geo_frame/parity_g1_test.exs reads the committed
files and compares them with the manifest.

Run:  ~/.venvs/geopandas-ref/bin/python -I test/reference/generate_io.py
"""

import json
import os

import geopandas as gpd
import pandas as pd
import pyarrow as pa
import shapely
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


if __name__ == "__main__":
    main()
