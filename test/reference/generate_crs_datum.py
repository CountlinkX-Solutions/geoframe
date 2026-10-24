"""Dev-only generator of the issue #62 fixtures (run with `python -I`).

Writes into test/reference/fixtures/crs_datum/:

* utm_estimate.json   GeoPandas `estimate_utm_crs` over a grid of points and boxes (Norway,
                      Svalbard, zone edges, poles, antimeridian, projected CRSs, other datums).
* native/geoframe_core/src/datum_grids.rs (`grid_table` argument): which PROJ-data grid files shift
                      each geographic CRS to a WGS 84 compatible datum (from proj.db).
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


# Targets a grid may shift to while still counting as "to WGS 84" (PROJ's own chain from these
# to WGS 84 is a null/ballpark transformation of about 1-2 m).
GRID_TARGETS = (4326, 4258, 4269, 4283, 7844, 4171, 4167, 4617, 4612, 6668)
GRID_EXTENSIONS = (".gsb",)


def generate_grid_table():
    import sqlite3
    from pyproj.datadir import get_data_dir

    db = sqlite3.connect(os.path.join(get_data_dir(), "proj.db"))
    rows = db.execute(
        """select g.source_crs_code, g.accuracy, g.grid_name, a.proj_grid_name, a.old_proj_grid_name,
                  a.proj_grid_format, a.proj_method, a.inverse_direction
           from grid_transformation g
           left join grid_alternatives a on a.original_grid_name = g.grid_name
           where g.deprecated = 0 and g.source_crs_auth_name = 'EPSG' and g.target_crs_auth_name = 'EPSG'
             and (g.method_name in ('NTv2', 'NADCON', 'NTv1')
                  or (a.proj_method = 'hgridshift' and a.proj_grid_format = 'GTiff'))
             and g.target_crs_code in (%s)
             and g.source_crs_code not in (%s)
             and exists (select 1 from geodetic_crs c where c.auth_name = 'EPSG'
                         and c.code = g.source_crs_code and c.type = 'geographic 2D')
           order by g.source_crs_code, g.accuracy, g.grid_name"""
        % (",".join(str(t) for t in GRID_TARGETS), ",".join(str(t) for t in GRID_TARGETS))
    ).fetchall()
    table = {}
    for src, acc, grid, tif, old, fmt, method, inverse in rows:
        if inverse:
            continue
        names = []
        if tif and fmt == "GTiff" and method == "hgridshift":
            names.append(tif)
        for candidate in (grid, old):
            if candidate and candidate.lower().endswith(GRID_EXTENSIONS) and " " not in candidate:
                names.append(candidate)
        for n in names:
            if n not in table.setdefault(src, []):
                table[src].append(n)
    lines = []
    for src in sorted(table):
        lines.append("    (%d, &[%s])," % (src, ", ".join('"%s"' % n for n in table[src])))
    path = os.path.join(HERE, "..", "..", "native", "geoframe_core", "src", "datum_grids.rs")
    header = (
        "//! Generated by test/reference/generate_crs_datum.py (`grid_table`) from the PROJ database\n"
        "//! (pyproj %s, PROJ %s): the horizontal grid files (NTv2 `.gsb`, and the GeoTIFF `.tif`\n"
        "//! PROJ-data ships for NTv2 and NADCON grids) that shift a geographic 2D EPSG CRS to WGS 84\n"
        "//! or a datum PROJ treats as equal to it, best accuracy first.\n\n"
        "pub(crate) const DATUM_GRIDS: &[(u32, &[&str])] = &[\n"
        % (pyproj.__version__, pyproj.proj_version_str)
    )
    open(path, "w").write(header + "\n".join(lines) + "\n];\n")
    return len(lines), sum(len(v) for v in table.values())


# -- datum shifts without grids ------------------------------------------------------------------

SHIFT_CRS = {
    # geographic and a projected CRS per datum (the projected ones exercise the same shift)
    4267: "NAD27", 26714: "NAD27 / UTM 14N", 26717: "NAD27 / UTM 17N", 4269: "NAD83", 26914: "NAD83 / UTM 14N", 4272: "NZGD49",
    4277: "OSGB36", 27700: "British National Grid", 4230: "ED50", 23032: "ED50 / UTM 32N",
    4314: "DHDN", 31467: "DHDN / GK3", 4301: "Tokyo", 4258: "ETRS89", 4312: "MGI", 4283: "GDA94",
    4284: "Pulkovo 1942", 4618: "SAD69", 4148: "Hartebeesthoek94", 4326: "WGS 84",
}


# NAD27's area of use is the whole continent and the Pacific (it crosses the antimeridian):
# sample the regions PROJ has a different NAD27 operation for.
EXPLICIT_POINTS = {
    4267: [(-120, 38), (-80, 35), (-100, 40), (-150, 62), (-100, 55), (-102, 23), (-85, 30), (-110, 45)],
}


def shift_cases():
    from pyproj.database import query_crs_info
    areas = {int(i.code): i.area_of_use for i in query_crs_info(auth_name="EPSG", allow_deprecated=False) if i.area_of_use}
    out = {}
    for code in SHIFT_CRS:
        a = areas[code]
        fwd = Transformer.from_crs(4326, code, always_xy=True)
        inv = Transformer.from_crs(code, 4326, always_xy=True)
        pts = []
        where = EXPLICIT_POINTS.get(code)
        if where is None:
            width = (a.east - a.west) % 360
            where = [
                (a.west + width * fx, a.south + (a.north - a.south) * fy)
                for fx in (0.5, 0.2, 0.8, 0.35, 0.65)
                for fy in (0.5, 0.25, 0.75)
            ]
        for lon, lat in where:
            x, y = fwd.transform(lon, lat)
            ex, ey = inv.transform(x, y)
            pts.append([x, y, ex, ey])
        out[str(code)] = {"name": SHIFT_CRS[code], "pts": pts}
    return out


# -- synthetic NTv2 / GeoTIFF grids ----------------------------------------------------------------

GRID_WEST, GRID_EAST, GRID_SOUTH, GRID_NORTH, GRID_STEP = 135.0, 141.0, 33.0, 37.0, 0.25


def field(lon, lat):
    """(north, east) shift in arc-seconds of the synthetic grid: smooth, not bilinear."""
    import math
    north = 12.0 + 0.8 * math.sin(lon * 0.9) + 0.5 * math.cos(lat)
    east = 10.5 + 0.6 * math.cos(lon * 0.7) + 0.4 * math.sin(lat * 1.1)
    return north, east


def grid_axes():
    ncols = int(round((GRID_EAST - GRID_WEST) / GRID_STEP)) + 1
    nrows = int(round((GRID_NORTH - GRID_SOUTH) / GRID_STEP)) + 1
    return ncols, nrows


def write_ntv2(path):
    ncols, nrows = grid_axes()

    def rec(key, value, kind):
        key = key.ljust(8).encode()
        if kind == "i":
            return key + struct.pack("<i4x", value)
        if kind == "d":
            return key + struct.pack("<d", value)
        return key + value.ljust(8).encode()

    head = b"".join([
        rec("NUM_OREC", 11, "i"), rec("NUM_SREC", 11, "i"), rec("NUM_FILE", 1, "i"),
        rec("GS_TYPE", "SECONDS", "s"), rec("VERSION", "NTv2.0", "s"), rec("SYSTEM_F", "Bessel", "s"),
        rec("SYSTEM_T", "WGS84", "s"), rec("MAJOR_F", 6377397.155, "d"), rec("MINOR_F", 6356078.963, "d"),
        rec("MAJOR_T", 6378137.0, "d"), rec("MINOR_T", 6356752.314, "d"),
    ])
    sub = b"".join([
        rec("SUB_NAME", "SYNTH", "s"), rec("PARENT", "NONE", "s"), rec("CREATED", "20260101", "s"),
        rec("UPDATED", "20260101", "s"), rec("S_LAT", GRID_SOUTH * 3600, "d"), rec("N_LAT", GRID_NORTH * 3600, "d"),
        rec("E_LONG", -GRID_EAST * 3600, "d"), rec("W_LONG", -GRID_WEST * 3600, "d"),
        rec("LAT_INC", GRID_STEP * 3600, "d"), rec("LONG_INC", GRID_STEP * 3600, "d"),
        rec("GS_COUNT", ncols * nrows, "i"),
    ])
    body = bytearray()
    for r in range(nrows):
        lat = GRID_SOUTH + r * GRID_STEP
        for c in range(ncols):  # east to west (longitude positive west increases)
            lon = GRID_EAST - c * GRID_STEP
            north, east = field(lon, lat)
            body += struct.pack("<4f", north, -east, 0.01, 0.01)  # lon shift is positive WEST
    trailer = b"END     " + b"\0" * 8
    open(path, "wb").write(head + sub + bytes(body) + trailer)


def write_geotiff(path):
    """Two float32 samples per pixel (latitude_offset, longitude_offset), arc-seconds, north first."""
    ncols, nrows = grid_axes()
    data = bytearray()
    for r in range(nrows):  # top row = northern row
        lat = GRID_NORTH - r * GRID_STEP
        for c in range(ncols):
            lon = GRID_WEST + c * GRID_STEP
            north, east = field(lon, lat)
            data += struct.pack("<2f", north, east)
    meta = (
        '<GDALMetadata><Item name="TYPE">HORIZONTAL_OFFSET</Item>'
        '<Item name="DESCRIPTION" sample="0" role="description">latitude_offset</Item>'
        '<Item name="UNITTYPE" sample="0" role="unittype">arc-second</Item>'
        '<Item name="DESCRIPTION" sample="1" role="description">longitude_offset</Item>'
        '<Item name="UNITTYPE" sample="1" role="unittype">arc-second</Item>'
        '<Item name="positive_value" sample="1">east</Item></GDALMetadata>'
    ).encode() + b"\0"
    # GeoKeyDirectory: GTModelType = geographic (2), GTRasterType = PixelIsPoint (2)
    geokeys = struct.pack("<12H", 1, 1, 0, 2, 1024, 0, 1, 2, 1025, 0, 1, 2)
    scale = struct.pack("<3d", GRID_STEP, GRID_STEP, 0.0)
    tie = struct.pack("<6d", 0.0, 0.0, 0.0, GRID_WEST, GRID_NORTH, 0.0)
    # tag -> (type, count, payload); payloads over 4 bytes are stored after the pixel data
    values = {
        258: (3, 2, struct.pack("<2H", 32, 32)),
        339: (3, 2, struct.pack("<2H", 3, 3)),
        33550: (12, 3, scale),
        33922: (12, 6, tie),
        34735: (3, len(geokeys) // 2, geokeys),
        42112: (2, len(meta), meta),
    }
    inline = {  # value fits in the entry
        256: (4, 1, struct.pack("<I", ncols)), 257: (4, 1, struct.pack("<I", nrows)),
        259: (3, 1, struct.pack("<H2x", 1)), 262: (3, 1, struct.pack("<H2x", 1)),
        277: (3, 1, struct.pack("<H2x", 2)), 278: (4, 1, struct.pack("<I", nrows)),
        279: (4, 1, struct.pack("<I", len(data))), 284: (3, 1, struct.pack("<H2x", 1)),
    }
    n_entries = len(values) + len(inline) + 1  # + StripOffsets
    data_off = 8 + 2 + n_entries * 12 + 4
    extra = bytearray()
    entries = {273: struct.pack("<HHII", 273, 4, 1, data_off)}
    for tag, (typ, cnt, raw) in inline.items():
        entries[tag] = struct.pack("<HHI", tag, typ, cnt) + raw
    for tag, (typ, cnt, raw) in values.items():
        if len(raw) <= 4:  # fits in the entry itself
            entries[tag] = struct.pack("<HHI", tag, typ, cnt) + raw.ljust(4, b"\0")
            continue
        offset = data_off + len(data) + len(extra)
        entries[tag] = struct.pack("<HHII", tag, typ, cnt, offset)
        extra += raw + (b"\0" if len(raw) % 2 else b"")
    ifd = struct.pack("<H", n_entries) + b"".join(entries[t] for t in sorted(entries)) + struct.pack("<I", 0)
    open(path, "wb").write(b"II*\0" + struct.pack("<I", 8) + ifd + bytes(data) + bytes(extra))


TOKYO_FALLBACK = "+proj=longlat +ellps=bessel +towgs84=-146.414,507.337,680.507,0,0,0,0 +no_defs"


def grid_cases(gsb, tif):
    rng = np.random.default_rng(6202)
    pts = []
    for _ in range(40):
        pts.append((float(rng.uniform(GRID_WEST + 0.3, GRID_EAST - 0.3)), float(rng.uniform(GRID_SOUTH + 0.3, GRID_NORTH - 0.3))))
    pts += [(GRID_WEST, GRID_SOUTH), (GRID_EAST, GRID_NORTH), (GRID_WEST + 0.125, 35.0), (138.0, GRID_NORTH - 0.01)]
    outside = [(130.0, 35.0), (142.5, 35.0), (138.0, 31.0), (138.0, 39.0), (143.0, 36.0)]
    out = {"grid_west": GRID_WEST, "grid_east": GRID_EAST, "grid_south": GRID_SOUTH, "grid_north": GRID_NORTH,
           "inside": [], "outside": []}
    def pipeline(path):
        crs = CRS.from_proj4("+proj=longlat +ellps=bessel +nadgrids=%s +no_defs" % path)
        return Transformer.from_crs(crs, 4326, always_xy=True), Transformer.from_crs(4326, crs, always_xy=True)
    fb = CRS.from_proj4(TOKYO_FALLBACK)
    fb_fwd, fb_inv = Transformer.from_crs(fb, 4326, always_xy=True), Transformer.from_crs(4326, fb, always_xy=True)
    gsb_f, gsb_i = pipeline(gsb)
    tif_f, tif_i = pipeline(tif)
    for lon, lat in pts:
        f_gsb = gsb_f.transform(lon, lat); f_tif = tif_f.transform(lon, lat)
        i_gsb = gsb_i.transform(lon, lat)
        out["inside"].append({"x": lon, "y": lat, "gsb": list(f_gsb), "tif": list(f_tif), "inv_gsb": list(i_gsb)})
    for lon, lat in outside:
        out["outside"].append({"x": lon, "y": lat, "fallback": list(fb_fwd.transform(lon, lat)), "inv_fallback": list(fb_inv.transform(lon, lat))})
    return out


def generate_datum():
    os.makedirs(os.path.join(OUT, "grids"), exist_ok=True)
    json.dump({"pyproj": pyproj.__version__, "proj": pyproj.proj_version_str, "crs": shift_cases()},
              open(os.path.join(OUT, "datum_shifts.json"), "w"), separators=(",", ":"))
    gsb = os.path.join(OUT, "grids", "synthetic_tokyo.gsb")
    tif = os.path.join(OUT, "grids", "synthetic_tokyo.tif")
    write_ntv2(gsb)
    write_geotiff(tif)
    json.dump(grid_cases(gsb, tif), open(os.path.join(OUT, "grids", "cases.json"), "w"), separators=(",", ":"))
    print("datum shifts and grids written")


if __name__ == "__main__":
    which = sys.argv[1:] or ["utm"]
    if "utm" in which:
        generate_utm()
    if "datum" in which:
        generate_datum()
    if "grid_table" in which:
        print("grid table: CRSs, grid names", generate_grid_table())
    if "utm_table" in which:
        print("utm table rows", generate_utm_table())
