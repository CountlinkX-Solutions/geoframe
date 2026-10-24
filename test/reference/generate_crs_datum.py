"""Dev-only generator of the issue #62 fixtures (run with `python -I`).

Writes into test/reference/fixtures/crs_datum/:

* utm_estimate.json   GeoPandas `estimate_utm_crs` over a grid of points and boxes (Norway,
                      Svalbard, zone edges, poles, antimeridian, projected CRSs, other datums).
* native/geoframe_core/src/utm_crs.rs (`utm_table` argument): the EPSG UTM CRS table of the PROJ
                      database that `estimate_utm_crs` filters, in the order pyproj returns it.
* datum_shifts.json   pyproj (no grids available) vs the points GeoFrame must reproduce.
* grids/              tiny synthetic NTv2 grids and pyproj results using the same files.
"""
import json
import os
import struct
import sys
import warnings

import geopandas as gpd
import numpy as np
import pyproj
from pyproj import CRS, Transformer
from pyproj.database import query_utm_crs_info
from shapely.geometry import Point

warnings.filterwarnings("ignore")
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "fixtures", "crs_datum")


def est(crs, box, datum=None):
    s = gpd.GeoSeries([Point(box[0], box[1]), Point(box[2], box[3])], crs=crs)
    try:
        return s.estimate_utm_crs(datum) if datum else s.estimate_utm_crs()
    except Exception as e:  # GeoPandas raises RuntimeError "Unable to determine UTM CRS"
        return None
    # unreachable


def code(c):
    return None if c is None else c.to_string()


def utm_cases():
    cases = []

    def add(crs, box, datum=None):
        r = est(crs, box, datum)
        cases.append([crs, datum, [float(v) for v in box], code(r)])

    # point grid, WGS 84
    for lon in np.arange(-180, 180.1, 7.5):
        for lat in np.arange(-90, 90.1, 5):
            add("EPSG:4326", [lon, lat, lon, lat])
    # Norway 32V and Svalbard 31X..37X regions, half-degree / one-degree
    for lon in np.arange(0, 45.1, 0.75):
        for lat in (55.5, 56, 58, 60, 62, 64, 64.5, 71.5, 72, 74, 76, 78, 80, 82, 84):
            add("EPSG:4326", [lon, lat, lon, lat])
    # zone edges, equator, +-80, 84 and just beyond
    for lon in (-180, -174, -6, 0, 6, 12, 174, 179.9999999, 180):
        for lat in (-90, -80.0000001, -80, -0.0000001, 0, 0.0000001, 80, 83.9999999, 84, 84.0000001, 90):
            add("EPSG:4326", [lon, lat, lon, lat])
    # boxes whose centre differs from either corner
    rng = np.random.default_rng(62)
    for _ in range(300):
        lon0 = rng.uniform(-180, 175); lat0 = rng.uniform(-80, 80)
        add("EPSG:4326", [lon0, lat0, lon0 + rng.uniform(0, 40), min(lat0 + rng.uniform(0, 30), 90)])
    # box that spans the antimeridian as a geographic frame (no wrap in GeoPandas)
    add("EPSG:4326", [170, 10, -170, 20]); add("EPSG:4326", [-170, 10, 170, 20])
    # other datums on a coarse grid
    for datum in ("NAD83", "NAD27", "ETRS89", "ETRS89-NOR [EUREF89]", "WGS 72", "SIRGAS 2000", "ED50", "Tokyo", "NAD83(2011)", "WGS84", "NoSuchDatum"):
        for lon in np.arange(-180, 180.1, 15):
            for lat in np.arange(-80, 84.1, 12):
                add("EPSG:4326", [lon, lat, lon, lat], datum)
    # projected / other geographic sources: the bounds are converted to lon/lat first
    pts = {
        "EPSG:3857": [(-9e6, 2.9e6, -8.8e6, 3.1e6), (1.5e6, 7.5e6, 2.5e6, 8.4e6), (-2e7, -1e7, -1.9e7, -9e6), (1.9e7, 0, 2.003e7, 5e6), (-2.003e7, -1e6, 2.003e7, 1e6)],
        "EPSG:32633": [(300000, 6500000, 700000, 7000000), (166021, 0, 833978, 9329005), (500000, 8700000, 500000, 9300000)],
        "EPSG:32617": [(300000, 2800000, 400000, 3000000), (-100000, 100000, 400000, 600000)],
        "EPSG:27700": [(100000, 100000, 300000, 300000), (400000, 400000, 450000, 900000), (0, 0, 700000, 1300000)],
        "EPSG:3035": [(3000000, 2000000, 4500000, 3500000), (2000000, 1500000, 7000000, 5500000)],
        "EPSG:4269": [(-100, 40, -90, 45)],
        "EPSG:3413": [(-500000, -2000000, 500000, -1000000), (-3000000, -3000000, 3000000, 3000000)],
        "EPSG:3031": [(-500000, -500000, 500000, 500000), (-2000000, -2000000, 2000000, 2000000)],
        "EPSG:3832": [(-1000000, 0, 1000000, 1000000), (14000000, 0, 15000000, 3000000)],
        "EPSG:2193": [(1500000, 5000000, 2000000, 5600000)],
        "EPSG:3994": [(-500000, -4000000, 500000, -3000000)],
        "EPSG:5070": [(-2000000, 2000000, 2000000, 3000000)],
        "EPSG:3395": [(-1.9e7, -1e6, -1.8e7, 1e6), (1.8e7, 5e6, 2.0e7, 6e6)],
        "EPSG:4326": [(-180, -90, 180, 90)],
    }
    for crs, boxes in pts.items():
        for b in boxes:
            add(crs, list(b))
            add(crs, list(b), "NAD83")
    return cases


def generate_utm():
    os.makedirs(OUT, exist_ok=True)
    cases = utm_cases()
    json.dump({"pyproj": __import__("pyproj").__version__, "geopandas": gpd.__version__, "cases": cases},
              open(os.path.join(OUT, "utm_estimate.json"), "w"), separators=(",", ":"))
    print("utm cases", len(cases), "unresolved", sum(1 for c in cases if c[3] is None))


def generate_utm_table():
    """Write the Rust source of the EPSG UTM CRS table (code, datum, west, south, east, north)."""
    rows = query_utm_crs_info()
    lines = []
    for r in rows:
        a = r.area_of_use
        if a is None:
            continue
        datum = r.name.split("/")[0].strip()
        lines.append('    (%s, "%s", %r, %r, %r, %r),' % (r.code, datum.replace('"', '\\"'), a.west, a.south, a.east, a.north))
    path = os.path.join(HERE, "..", "..", "native", "geoframe_core", "src", "utm_crs.rs")
    header = (
        "//! Generated by test/reference/generate_crs_datum.py (`utm_table`) from the PROJ database\n"
        "//! (pyproj %s, PROJ %s): every EPSG projected CRS named \"... / UTM zone ...\", as\n"
        "//! `pyproj.database.query_utm_crs_info()` lists them (ascending code).\n"
        "//! Columns: code, datum (the CRS name before the slash), west, south, east, north of the\n"
        "//! area of use in degrees (west > east when it crosses the antimeridian).\n\n"
        "pub(crate) const UTM_CRS: &[(u16, &str, f64, f64, f64, f64)] = &[\n"
        % (pyproj.__version__, pyproj.proj_version_str)
    )
    open(path, "w").write(header + "\n".join(lines) + "\n];\n")
    return len(lines)


if __name__ == "__main__":
    which = sys.argv[1:] or ["utm"]
    if "utm" in which:
        generate_utm()
    if "utm_table" in which:
        print("utm table rows", generate_utm_table())
