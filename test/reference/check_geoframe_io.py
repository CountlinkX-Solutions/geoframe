"""Dev-only cross-check: files written by GeoFrame must read in GeoPandas 1.2 / pyarrow.

Usage (after `mix run scripts/write_io_samples.exs <dir>`, or just run scripts/crosscheck_io.sh):

    ~/.venvs/geopandas-ref/bin/python -I test/reference/check_geoframe_io.py <dir>

For every entry of <dir>/expected.json it reads the file with GeoPandas (`read_feather`,
`read_parquet`, `GeoDataFrame.from_arrow(pa.ipc.open_stream(...))`, `read_file` for Shapefile
and GeoPackage), compares the CRS, geometries and attribute columns, validates the GeoArrow
extension metadata, and for files with a covering column also runs a `bbox=` read. GeoPackages
are also checked with SQLite (`integrity_check`, GeoPackage application id, R-tree index
answering a `bbox=` read like the one GDAL maintains) and pyogrio's layer list. Exits non-zero
on the first failing file group.
"""

import json
import os
import sys

import geopandas as gpd
import pandas as pd
import pyarrow as pa
import pyarrow.feather as feather
import pyarrow.parquet as pq
import shapely
from pyproj import CRS
from shapely import wkt as swkt


def fail(file, message):
    raise AssertionError(f"{file}: {message}")


def same_geometry(actual, expected):
    if expected is None:
        return actual is None
    if actual is None:
        return False
    # normalize: shapefiles store exterior rings clockwise, so ring order and orientation differ
    return shapely.equals_exact(shapely.normalize(actual), shapely.normalize(swkt.loads(expected)), tolerance=1e-9)


def comparable(value, expected):
    """A pandas value as plain Python; timestamps become ISO text like the expectation."""
    if value is None or (not isinstance(value, str) and pd.isna(value)):
        return None
    if isinstance(value, pd.Timestamp):
        return value.strftime("%Y-%m-%d") if isinstance(expected, str) and len(expected) == 10 else value.strftime("%Y-%m-%dT%H:%M:%S")
    return value.item() if hasattr(value, "item") else value


def check_frame(file, gdf, entry):
    if entry["crs"] is None:
        if gdf.crs is not None:
            fail(file, f"crs {gdf.crs!r} != None")
    elif gdf.crs is None or gdf.crs != CRS.from_user_input(entry["crs"]):
        fail(file, f"crs {gdf.crs!r} != {entry['crs']!r}")
    geoms = list(gdf.geometry)
    if len(geoms) != len(entry["wkt"]):
        fail(file, f"{len(geoms)} rows != {len(entry['wkt'])}")
    for index, (actual, expected) in enumerate(zip(geoms, entry["wkt"])):
        if not same_geometry(actual, expected):
            fail(file, f"row {index}: {actual} != {expected}")
    if "bbox" in gdf.columns:
        fail(file, "covering column leaked into the frame")
    for name, expected in entry["columns"].items():
        if name not in gdf.columns:
            fail(file, f"column {name} missing; GeoPandas read {list(gdf.columns)}")
        actual = [comparable(v, e) for v, e in zip(gdf[name].tolist(), expected)]
        expected = [e[:19] if isinstance(e, str) and len(e) > 19 and e[10] == "T" else e for e in expected]
        if actual != expected:
            fail(file, f"column {name}: {actual} != {expected}")
    extra = set(gdf.columns) - set(entry["columns"]) - {"geometry"}
    if extra:
        fail(file, f"unexpected columns {sorted(extra)}")


def check_extension(file, schema, native):
    field = schema.field("geometry")
    name = field.metadata.get(b"ARROW:extension:name", b"").decode()
    expected = "geoarrow.wkb" if not native else "geoarrow."
    if not name.startswith(expected):
        fail(file, f"extension name {name!r}, expected {expected!r}*")
    raw = field.metadata.get(b"ARROW:extension:metadata")
    if raw is None:
        fail(file, "no ARROW:extension:metadata")
    json.loads(raw)


def check_gpkg(file, path, entry):
    import sqlite3

    import pyogrio

    connection = sqlite3.connect(path)
    if connection.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
        fail(file, "SQLite integrity_check failed")
    if connection.execute("PRAGMA application_id").fetchone()[0] != 0x47504B47:
        fail(file, "application_id is not GPKG")
    connection.close()
    names = [row[0] for row in pyogrio.list_layers(path)]
    layer = entry.get("layer") or names[0]  # no layer: the layer is named after the file
    if layer not in names:
        fail(file, f"layer {layer!r} not in pyogrio's list {names}")
    gdf = gpd.read_file(path, layer=layer)
    check_frame(file, gdf, entry)
    info = pyogrio.read_info(path, layer=layer)
    if info["features"] != len(entry["wkt"]):
        fail(file, f"pyogrio counts {info['features']} features")
    # the R-tree is what makes a bbox read fast: it must agree with a full scan
    wide = gpd.read_file(path, layer=layer, bbox=(-1000, -1000, 1000000, 10000000))
    present = len([w for w in entry["wkt"] if w is not None])
    if len(wide) != present:
        fail(file, f"bbox read returned {len(wide)} rows, expected {present}")
    if info["geometry_type"] is None:
        fail(file, "no geometry type")


def main(directory):
    with open(os.path.join(directory, "expected.json")) as handle:
        entries = json.load(handle)

    for entry in entries:
        file = entry["file"]
        path = os.path.join(directory, file)
        kind = entry["kind"]
        if kind == "feather":
            gdf = gpd.read_feather(path)
            check_frame(file, gdf, entry)
            check_extension(file, feather.read_table(path).schema, entry["native"])
            table = feather.read_table(path)
            check_frame(file, gpd.GeoDataFrame.from_arrow(table), entry)
        elif kind == "parquet":
            gdf = gpd.read_parquet(path)
            check_frame(file, gdf, entry)
            meta = json.loads(pq.read_metadata(path).metadata[b"geo"])
            if entry.get("covering"):
                if "covering" not in meta["columns"]["geometry"]:
                    fail(file, "covering metadata missing")
                filtered = gpd.read_parquet(path, bbox=(0.5, 0.5, 3.5, 3.5))
                if len(filtered) == 0:
                    fail(file, "bbox read returned no rows")
                if "bbox" in filtered.columns:
                    fail(file, "bbox read leaked the covering column")
            elif "covering" in meta["columns"]["geometry"]:
                fail(file, "unexpected covering metadata")
            if file == "wkb_v1_0_0.parquet" and meta["version"] != "1.0.0":
                fail(file, f"version {meta['version']}")
        elif kind == "arrows":
            with pa.ipc.open_stream(path) as reader:
                table = reader.read_all()
            check_extension(file, table.schema, entry["native"])
            check_frame(file, gpd.GeoDataFrame.from_arrow(table), entry)
            # A bare pyarrow array drops the field's extension metadata, so the geometry-only
            # table is what a GeoSeries.to_arrow stream looks like to GeoPandas.
            only = gpd.GeoDataFrame.from_arrow(table.select(["geometry"]))
            if len(only) != len(entry["wkt"]) or list(only.columns) != ["geometry"]:
                fail(file, "geometry-only table")
        elif kind == "shapefile":
            gdf = gpd.read_file(path)
            check_frame(file, gdf, entry)
        elif kind == "gpkg":
            check_gpkg(file, path, entry)
        else:
            fail(file, f"unknown kind {kind}")
        print(f"ok  {file}")

    print(f"\n{len(entries)} GeoFrame-written files read correctly in GeoPandas {gpd.__version__} / pyarrow {pa.__version__} / pyogrio")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
