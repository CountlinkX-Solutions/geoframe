"""Dev-only generator of the issue #62 fixtures (run with `python -I`).

Writes into test/reference/fixtures/crs_datum/:

* utm_estimate.json   GeoPandas `estimate_utm_crs` over a grid of points and boxes (Norway,
                      Svalbard, zone edges, poles, antimeridian, projected CRSs, other datums).
* native/geoframe_core/src/datum_grids.rs (`grid_table` argument): which PROJ-data grid files shift
                      each geographic CRS to a WGS 84 compatible datum (from proj.db).
* native/geoframe_core/src/utm_crs.rs (`utm_table` argument): the EPSG UTM CRS table of the PROJ
                      database that `estimate_utm_crs` filters, in the order pyproj returns it.
* datum_shifts.json   pyproj (no grids available) vs the points GeoFrame must reproduce.
* crs_parity.json     pyproj results for the CRSs of issue #97 (axis directions, grads and prime
                      meridians, ellipsoidal projections, extra projections, Helmert by area).
* crs_convert.json    CRS definitions as PROJJSON, WKT1 and WKT2 text (no identifier) with pyproj's
                      results, for the definition converter (issue #100).
* crs_esri_datum.json (`esri_wkt` argument): WKT1_ESRI text of common CRSs (GEOGCS "GCS_...",
                      DATUM "D_...", no identifier) with pyproj's results: PROJ identifies the datum
                      by name and shifts it to WGS 84 like the code would.
* native/geoframe_core/src/crs_extra.bin (`extra_table`): EPSG definitions missing from the
                      `crs-definitions` table, as compressed PROJJSON.
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


SKIP_STEPS = ("molodensky", "hgridshift", "vgridshift", "deformation", "xyzgridshift")


def _helmert_step(kv, inverse):
    """(matrix, translation) of one Helmert / Molodensky-Badekas step on geocentric coordinates
    (position vector convention, small-angle rotation), or None."""
    import numpy as np
    if "t_epoch" in kv or "dx" in kv:
        return None
    if "convention" not in kv and any(k in kv for k in ("rx", "ry", "rz")):
        return None
    sign = -1.0 if kv.get("convention") == "coordinate_frame" else 1.0
    g = lambda k, f=1.0: float(kv.get(k, 0.0)) * f
    t = np.array([g("x"), g("y"), g("z")])
    rad = np.array([g("rx", sign), g("ry", sign), g("rz", sign)]) * np.pi / 180 / 3600
    if kv.get("exact"):
        # PROJ's `helmert exact`: the exact rotation matrix of the coordinate frame angles (those of
        # the position vector convention, negated), not the small-angle one. The verified formula
        # (pyproj `from_pipeline`, Saba: 0.00 mm).
        cr, sr, cp, sp, cy, sy = (f(a) for a in -rad for f in (np.cos, np.sin))
        rotation = np.array([[cp * cy, cr * sy + sr * sp * cy, sr * sy - cr * sp * cy],
                             [-cp * sy, cr * cy - sr * sp * sy, sr * cy + cr * sp * sy],
                             [sp, -sr * cp, cr * cp]])
    else:
        rotation = np.array([[1, -rad[2], rad[1]], [rad[2], 1, -rad[0]], [-rad[1], rad[0], 1]])
    mat = (1 + g("s") * 1e-6) * rotation
    if "px" in kv:  # Molodensky-Badekas: X' = P + T + M (X - P)
        pivot = np.array([g("px"), g("py"), g("pz")])
        t = pivot + t - mat @ pivot
    if inverse:
        inv = np.linalg.inv(mat)
        mat, t = inv, -inv @ t
    return mat, t


def _as_params(mat, t):
    """The `+towgs84` set (translations, rotations in arc-seconds, scale in ppm) of an affine map
    whose rotation is small: exact for one step, to ~1e-11 relative for a composition."""
    import numpy as np
    s = (mat[0, 0] + mat[1, 1] + mat[2, 2]) / 3.0
    rx, ry, rz = (mat[2, 1] - mat[1, 2]) / 2 / s, (mat[0, 2] - mat[2, 0]) / 2 / s, (mat[1, 0] - mat[0, 1]) / 2 / s
    r = np.array([rx, ry, rz]) * 180 / np.pi * 3600
    return [float(v) for v in (*t, *r, (s - 1) * 1e6)]


def parse_op(definition):
    """The operation a PROJ pipeline holds, as (kind, params): (0, +towgs84 set) for one or more
    Helmert / Molodensky-Badekas steps (composed into one), (1, [dlat, dlon, 0, 0, 0, 0, 0]) for a
    `geogoffset` (arc-seconds), (2, [dlat, dlon, tx, ty, tz, 0, 0]) for a geogoffset followed by
    a translation on the GRS80 ellipsoid (the Greek datum) and (4, +towgs84 set) for a single
    `helmert exact` step (the exact rotation matrix, where the rotations are too large for the
    engine's small-angle `+towgs84`: Saba, 2e-3 rad); else None. A null operation (`proj=noop`:
    the coordinates are kept, the ellipsoid change is ignored) is a zero geogoffset."""
    import re
    import numpy as np
    d = re.sub(r"\s+", " ", definition)
    if any(k in d for k in SKIP_STEPS):
        return None
    if d.strip().startswith("proj=noop"):
        return (1, [0.0] * 7)
    exact = False
    exact_set = None
    geocentric = False
    mat, t = np.eye(3), np.zeros(3)
    offset = None
    helmerts = 0
    for step in d.split(" step ")[1:]:
        inverse = step.startswith("inv ")
        step = step[4:] if inverse else step
        m = re.match(r"proj=(\w+) ?(.*)", step)
        if not m:
            return None
        name, rest = m.groups()
        kv = dict(tok.split("=", 1) for tok in rest.split() if "=" in tok)
        if "exact" in rest.split():
            kv["exact"] = "1"
            exact = True
        if name == "geogoffset":
            if offset is not None or helmerts or inverse:
                return None
            offset = (float(kv.get("dlat", 0.0)), float(kv.get("dlon", 0.0)))
        elif name in ("helmert", "molobadekas"):
            one = _helmert_step(kv, inverse)
            if one is None:
                return None
            mat, t = one[0] @ mat, one[0] @ t + one[1]
            helmerts += 1
            sign = -1.0 if kv.get("convention") == "coordinate_frame" else 1.0
            exact_set = [float(kv.get(k, 0.0)) * f for k, f in (("x", 1), ("y", 1), ("z", 1), ("rx", sign), ("ry", sign), ("rz", sign), ("s", 1))]
            exact_set = None if inverse or "px" in kv else exact_set
        elif name == "cart":
            geocentric = True
        elif name not in ("unitconvert", "push", "pop", "longlat", "noop"):
            return None
    if offset is None:
        if not helmerts:
            # geocentric on the two ellipsoids with no step between (GSK-2011): a zero shift
            return (0, [0.0] * 7) if geocentric else None
        if exact:
            return (4, exact_set) if helmerts == 1 and exact_set else None
        return (0, _as_params(mat, t))
    if not helmerts:
        return (1, [offset[0], offset[1], 0.0, 0.0, 0.0, 0.0, 0.0])
    params = _as_params(mat, t)
    if any(abs(v) > 1e-9 for v in params[3:]):
        return None
    return (2, [offset[0], offset[1], *params[:3], 0.0, 0.0])


def is_offshore(area_name):
    """PROJ prefers an operation whose area is not an offshore one, whatever the accuracies: the
    name says "offshore" and does not say "onshore" before it ("Norway - offshore north of 62°N.
    Also Svalbard - onshore and offshore." is offshore, "Nigeria - onshore and offshore." is not).
    Measured against `Transformer.get_last_used_operation` on ~5 500 sampled points: 256
    differences, against 259 when "onshore" anywhere makes the area not offshore, and the
    onshore Finnish and Norwegian Lapland (Tromso, Rovaniemi) go the PROJ way."""
    n = (area_name or "").lower()
    offshore, onshore = n.find("offshore"), n.find("onshore")
    return offshore >= 0 and (onshore < 0 or offshore < onshore)


def _unlisted_operation(info):
    """The operation `Transformer.from_crs(code, 4326)` runs at the centre of the area of use when
    it is not a ballpark one, in the shape of `helmert_ops` entries (else none)."""
    from pyproj import Transformer
    area = info.area_of_use
    if area is None:
        return []
    w, s, e, n = area.bounds
    lon = ((w + ((e + 360 if w > e else e) - w) / 2 + 180) % 360) - 180
    try:
        tr = Transformer.from_crs(int(info.code), 4326, always_xy=True)
        tr.transform(lon, (s + n) / 2)
        used = tr.get_last_used_operation()
    except Exception:
        return []
    if used is None or used.area_of_use is None or used.description.startswith("Ballpark"):
        return []
    op = parse_op(used.definition)
    if op is None:
        return []
    bw, bs, be, bn = used.area_of_use.bounds
    acc = used.accuracy if used.accuracy is not None and used.accuracy >= 0 else 1e9
    size = (be - bw) % 360 * (bn - bs) if bw > be else (be - bw) * (bn - bs)
    # the only operation of the list: PROJ applies it everywhere, whatever its area of use
    return [(False, acc, size, 0, [-180.0, -90.0, 180.0, 90.0], op)]


def helmert_ops():
    """Per geographic 2D EPSG CRS: the operations to WGS 84 PROJ can use without grids, in the
    order GeoFrame tries them: operations not for an offshore-only area first, then known accuracy
    first and best first, then smaller area, then PROJ's own order. Measured against pyproj this
    rule picks PROJ's operation in ~99 % of the points of a random sample of the area of use of
    every CRS with two or more operations."""
    from pyproj.database import query_crs_info
    from pyproj.enums import PJType
    from pyproj.transformer import TransformerGroup
    out = {}
    skipped = 0
    for info in query_crs_info("EPSG", pj_types=[PJType.GEOGRAPHIC_2D_CRS]):
        if info.deprecated or int(info.code) == 4326:
            continue
        try:
            group = TransformerGroup(int(info.code), 4326, always_xy=True, allow_ballpark=True)
        except Exception:
            continue
        ops = []
        ballpark_only = True
        listed_ballpark = any(t.description.startswith(("Ballpark", "Transformation from")) for t in group.transformers)
        for idx, t in enumerate(group.transformers):
            if t.area_of_use is None or t.description.startswith(("Ballpark", "Transformation from")):
                continue
            ballpark_only = False
            op = parse_op(t.definition)
            if op is None:
                skipped += 1
                continue
            w, s, e, n = t.area_of_use.bounds
            acc = t.accuracy if t.accuracy is not None and t.accuracy >= 0 else 1e9
            area = (e - w) % 360 * (n - s) if w > e else (e - w) * (n - s)
            ops.append((is_offshore(t.area_of_use.name), acc, area, idx, [w, s, e, n], op))
        if ballpark_only and group.unavailable_operations:
            # PROJ's own `Transformer` may still pick an operation the group does not list when the
            # direct one needs a missing grid (RD/83 and PD/83: Helmert to ETRS89, then a null step).
            ops += _unlisted_operation(info)
            ballpark_only = not ops
        ops.sort(key=lambda o: o[:4])
        world = [-180.0, -90.0, 180.0, 90.0]
        if len(ops) == 1 and not listed_ballpark:
            # a lone operation applies everywhere: its own area is the world
            ops[0] = ops[0][:4] + (world, ops[0][5])
        elif ops and not listed_ballpark and not any(o[4] == world for o in ops):
            # PROJ's list has no ballpark entry: a point outside every area of use gets its first
            # operation (Minna at 40E 10N, NZGD49 anywhere). Kind + 16 marks this catch-all, which
            # only a geographic CRS takes (PROJ keeps the operations that meet the area of a
            # projected one).
            first = group.transformers[0]
            op = parse_op(first.definition) if first.area_of_use is not None else None
            if op is not None:
                ops.append((False, 1e10, 1e10, 0, world, (op[0] + 16, op[1])))
        if ops or ballpark_only:
            # ballpark only: PROJ keeps the coordinates, so the list is empty (no shift anywhere)
            out[int(info.code)] = [(o[4], o[5]) for o in ops]
    return out, skipped


def generate_helmert_table():
    """Write native/geoframe_core/src/datum_helmert.rs."""
    table, skipped = helmert_ops()
    lists = []
    index = {}
    rows = []
    for code in sorted(table):
        key = repr(table[code])
        if key not in index:
            index[key] = len(lists)
            lists.append(table[code])
        rows.append((code, index[key]))
    def fmt(v):
        t = ("%.9f" % v).rstrip("0")
        return t + "0" if t.endswith(".") else t

    lines = []
    for i, ops in enumerate(lists):
        body = ", ".join(
            "([%s], [%s], %d)" % (", ".join(fmt(b) for b in box), ", ".join(fmt(v) for v in params), kind)
            for box, (kind, params) in ops
        )
        lines.append("    &[%s], // %d" % (body, i))
    header = (
        "//! Generated by test/reference/generate_crs_datum.py (`helmert_table`) from the PROJ database\n"
        "//! (pyproj %s, PROJ %s): the Helmert operations PROJ can run without grids from each geographic\n"
        "//! 2D EPSG CRS to WGS 84, as `TransformerGroup` lists them, each with its area of use\n"
        "//! (`[west, south, east, north]`, degrees; west > east across the antimeridian) and parameters.\n"
        "//! Kind 0 is a `+towgs84` set (position vector convention: metres, arc-seconds, ppm; several\n"
        "//! Helmert steps are composed into one). Kind 1 is a `geogoffset` ([dlat, dlon] arc-seconds) and\n"
        "//! kind 2 a `geogoffset` followed by a translation [tx, ty, tz] metres on the GRS80 ellipsoid.\n"
        "//! A null operation (`noop`: the coordinates are kept, whatever the ellipsoids) is a zero offset.\n"
        "//! Kind 4 is one exact Helmert step ([tx, ty, tz, rx, ry, rz, scale] as a `+towgs84` set, applied with\n"
        "//! the exact rotation matrix: rotations too large for the small-angle formula).\n"
        "//! Operations are in the order they are tried: not offshore first, known accuracy first,\n"
        "//! best accuracy first, smaller area first, PROJ's order. A last operation with the whole world as\n"
        "//! its area and 16 added to its kind is PROJ's first operation, for the CRSs whose PROJ list has no\n"
        "//! ballpark entry: what a point outside every area of use gets (a geographic CRS only).\n"
        "//! `DATUM_HELMERT` maps a CRS code to an index of `HELMERT_LISTS`.\n\n"
        "pub(crate) type HelmertOp = ([f64; 4], [f64; 7], u8);\n\n"
        % (pyproj.__version__, pyproj.proj_version_str)
    ).replace("\\n", "\n")
    path = os.path.join(HERE, "..", "..", "native", "geoframe_core", "src", "datum_helmert.rs")
    text = header + "pub(crate) static HELMERT_LISTS: &[&[HelmertOp]] = &[\n" + "\n".join(lines) + "\n];\n\n"
    text += "pub(crate) static DATUM_HELMERT: &[(u32, u16)] = &[\n" + "\n".join("    (%d, %d)," % r for r in rows) + "\n];\n"
    open(path, "w").write(text)
    return len(rows), len(lists), sum(len(l) for l in lists), skipped


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


# -- issue #97 parity cases --------------------------------------------------------------------

PARITY_CRS = [
    # axis directions, grad units with prime meridians, ellipsoidal equidistant cylindrical
    5513, 2065, 5514, 4807, 4901, 4810, 4816, 4821, 4811, 4087,
    # projections proj4rs lacks: omerc A/B, cass, poly, eqearth, nzmg, aeqd (feature)
    # Helmert operation chosen by area of use
    4688, 3303, 4156, 4267, 4230, 4618,
    2057, 3078, 3375, 3468, 6394, 2066, 3068, 3377, 5472, 5880, 8857, 8858, 27200, 3295, 3993,
]


def parity_cases():
    from pyproj.database import query_crs_info
    areas = {int(i.code): i.area_of_use for i in query_crs_info(auth_name="EPSG", allow_deprecated=False) if i.area_of_use}
    out = {}
    for code in PARITY_CRS:
        a = areas[code]
        fwd = Transformer.from_crs(4326, code, always_xy=True)
        inv = Transformer.from_crs(code, 4326, always_xy=True)
        width = (a.east - a.west) % 360 or 360
        pts = []
        for fx, fy in ((0.5, 0.5), (0.2, 0.3), (0.8, 0.7), (0.35, 0.8), (0.65, 0.2)):
            lon = a.west + width * fx
            lon = ((lon + 180) % 360) - 180
            lat = a.south + (a.north - a.south) * fy
            x, y = fwd.transform(lon, lat)
            ex, ey = inv.transform(x, y)
            pts.append([lon, lat, x, y, ex, ey])
        out[str(code)] = {"name": CRS(code).name, "geographic": CRS(code).is_geographic, "pts": pts}
    return out


def generate_parity():
    json.dump({"pyproj": pyproj.__version__, "proj": pyproj.proj_version_str, "crs": parity_cases()},
              open(os.path.join(OUT, "crs_parity.json"), "w"), separators=(",", ":"))
    print("crs parity written")


# -- definitions given as text (issue #100) --------------------------------------------------------

# One CRS per conversion method, plus the datum features the converter must read (prime meridian,
# grads, feet, axis directions, geographic and geocentric CRS). Codes outside the bundled table
# (5515, 8441, 6244, 3139 ...) are included on purpose.
CONVERT_CRS = [
    2000, 2046, 2062, 2138, 2056, 2065, 2172, 2264, 2229, 3000, 3031, 3035, 3078, 3388, 3408,
    3857, 3993, 4087, 5041, 5472, 5515, 5224, 8441, 6244, 3139, 6201, 6933, 8857, 9549, 27200,
    27701, 3295, 4807, 4326, 4269, 4230, 4979, 5514, 3410, 32633, 3067, 2193,
]


def convert_cases():
    from pyproj.database import query_crs_info
    areas = {int(i.code): i.area_of_use for i in query_crs_info(auth_name="EPSG", allow_deprecated=False) if i.area_of_use}
    out = {}
    for code in CONVERT_CRS:
        crs = CRS(code)
        a = areas.get(code) or crs.area_of_use
        west, south, east, north = a.west, a.south, a.east, a.north
        fwd = Transformer.from_crs(4326, crs, always_xy=True)
        inv = Transformer.from_crs(crs, 4326, always_xy=True)
        width = (east - west) % 360 or 360
        pts = []
        for fx, fy in ((0.5, 0.5), (0.25, 0.3), (0.75, 0.7)):
            lon = ((west + width * fx + 180) % 360) - 180
            lat = south + (north - south) * fy
            x, y, *_ = fwd.transform(lon, lat, 0.0)
            ex, ey = inv.transform(x, y)
            pts.append([lon, lat, x, y, ex, ey])
        pj = crs.to_json_dict()
        pj.pop("id", None)
        entry = {"name": crs.name, "geographic": crs.is_geographic, "projjson": pj, "pts": pts}
        # WKT text for a subset, so every flavour is exercised without bloating the fixture
        if code in WKT_CRS:
            # without the identifier of the whole CRS (the last node), so it is read, not looked up
            import re
            try:
                entry["wkt1"] = re.sub(r',AUTHORITY\["EPSG","\d+"\]\]$', "]", crs.to_wkt(pyproj.enums.WktVersion.WKT1_GDAL))
            except Exception:
                pass  # WKT1 cannot express the method (Modified Krovak)
            entry["wkt2"] = re.sub(r',ID\["EPSG",\d+\]\]$', "]", crs.to_wkt(pyproj.enums.WktVersion.WKT2_2019))
        out[str(code)] = entry
    return out


WKT_CRS = [2000, 2062, 3031, 3078, 3035, 5041, 4807, 4326, 32633, 3857, 2264, 2056, 5515]


def generate_convert():
    json.dump({"pyproj": pyproj.__version__, "proj": pyproj.proj_version_str, "crs": convert_cases()},
              open(os.path.join(OUT, "crs_convert.json"), "w"), separators=(",", ":"))
    print("crs convert written")


def generate_name_table():
    """Write native/geoframe_core/src/crs_names.rs: the geographic 2D EPSG CRSs that have a Helmert
    list or grids, by normalised name, so a definition without an identifier finds its datum
    shifts the way PROJ identifies a CRS by name."""
    import re
    src = os.path.join(HERE, "..", "..", "native", "geoframe_core", "src")
    codes = set(int(c) for c in re.findall(r"^    \((\d+), ", open(os.path.join(src, "datum_helmert.rs")).read(), re.M))
    codes |= set(int(c) for c in re.findall(r"^    \((\d+), &\[", open(os.path.join(src, "datum_grids.rs")).read(), re.M))
    from pyproj.database import query_crs_info
    from pyproj.enums import PJType
    rows = {}
    for info in query_crs_info("EPSG", pj_types=[PJType.GEOGRAPHIC_2D_CRS]):
        if info.deprecated or int(info.code) not in codes:
            continue
        key = info.name.lower().replace("_", " ")
        rows.setdefault(key, int(info.code))
    lines = ['    ("%s", %d),' % (k.replace("\\", "\\\\").replace('"', '\\"'), rows[k]) for k in sorted(rows)]
    header = (
        "//! Generated by test/reference/generate_crs_datum.py (`name_table`) from the PROJ database\n"
        "//! (pyproj %s, PROJ %s): the non-deprecated geographic 2D EPSG CRSs that have Helmert operations\n"
        "//! or grids (`datum_helmert.rs`, `datum_grids.rs`), by lower-case name with `_` as a space.\n\n"
        "pub(crate) const CRS_NAMES: &[(&str, u32)] = &[\n" % (pyproj.__version__, pyproj.proj_version_str)
    )
    open(os.path.join(src, "crs_names.rs"), "w").write(header + "\n".join(lines) + "\n];\n")
    return len(lines)


# -- ESRI WKT1 text: the datum identified by its name ---------------------------------------------

# Common CRSs whose .prj text (WKT1_ESRI) names its datum `D_...` without an authority. The first
# group has a datum shift in PROJ (Helmert by area; no grids are available to pyproj here), the
# last ones agree already (WGS 84 compatible datums or a ballpark), as a regression guard.
ESRI_WKT_CRS = [
    4267, 26717, 32038, 26912, 4277, 27700, 4230, 23030, 4301, 4618, 4284, 4289, 31370,
    4242, 24200, 4169, 3102, 4313, 2154, 4269, 26917, 4258, 25832, 4674, 4283, 28355,
]


def esri_wkt_cases():
    import re
    out = {}
    for code in ESRI_WKT_CRS:
        crs = CRS(code)
        a = crs.area_of_use
        wkt = crs.to_wkt("WKT1_ESRI")
        # without the identifier of the whole CRS, if any, so it is read, not looked up
        wkt = re.sub(r',AUTHORITY\["[A-Z]+","[A-Za-z0-9_]+"\]\]$', "]", wkt)
        fwd = Transformer.from_crs(4326, crs, always_xy=True)
        inv = Transformer.from_crs(wkt, 4326, always_xy=True)
        pts = []
        east = a.east + 360 if a.west > a.east else a.east
        # the middle of the area of use, where PROJ's choice among the operations is not in doubt
        for fx, fy in ((0.5, 0.5), (0.4, 0.45), (0.6, 0.55)):
            lon = ((a.west + (east - a.west) * fx + 180) % 360) - 180
            lat = a.south + (a.north - a.south) * fy
            x, y, *_ = fwd.transform(lon, lat, 0.0)
            ex, ey = inv.transform(x, y)
            pts.append([x, y, ex, ey])
        out[str(code)] = {"name": crs.name, "geographic": crs.is_geographic, "wkt": wkt, "pts": pts}
    return out


def generate_esri_wkt():
    json.dump({"pyproj": pyproj.__version__, "proj": pyproj.proj_version_str, "crs": esri_wkt_cases()},
              open(os.path.join(OUT, "crs_esri_datum.json"), "w"), separators=(",", ":"))
    print("crs esri datum written")




def generate_datum_name_table():
    """Write native/geoframe_core/src/crs_datum_names.rs: the datum names (EPSG name, EPSG and ESRI
    aliases: `D_North_American_1927`, `European_Datum_1950`) of the geographic 2D EPSG CRSs of
    `crs_names.rs`, keyed by lower-case name with `_` as a space and by the longitude of the prime
    meridian in degrees (Greenwich 0, Paris 2.33722917, Ferro -17.6667...), so a definition with no
    identifier finds the CRS of its datum the way PROJ identifies the datum of a WKT1 text by its
    name, and `D_MGI` on the Ferro meridian is MGI (Ferro) and not MGI. A (name, meridian) pair that
    two datums share is left out. The last column says whether GeoFrame's by-code answer for that CRS
    is within 5 cm of pyproj in the by-code sweep (`CRS_SWEEP_OUT`: the output of `sweep_crs.py`) and
    so is the WKT1_ESRI text of that geographic CRS (`CRS_ESRI_SWEEP_OUT`: a
    list of JSON files of rows `{code, ex, ey, g}`, pyproj's and GeoFrame's answer for the text, from
    a probe run with every flag true; PROJ identifies Monte Mario (Rome) by its name as the Greenwich
    datum, which its code does not). Every flag is true when neither is given. A text on another
    meridian is answered through that CRS only when its flag is true; a projected CRS on it is answered
    through it either way (its by-code and text sweeps agree with pyproj, MGI (Ferro) Austria)."""
    import re
    import sqlite3
    from pyproj import CRS
    src = os.path.join(HERE, "..", "..", "native", "geoframe_core", "src")
    codes = set(int(c) for c in re.findall(r'^    \("[^"]*", (\d+)\),', open(os.path.join(src, "crs_names.rs")).read(), re.M))
    bad = set()
    sweep = os.environ.get("CRS_SWEEP_OUT")
    if sweep:
        for row in json.load(open(sweep)):
            if row.get("dev", 0) > 0.05 and CRS.from_epsg(row["code"]).is_geographic:
                bad.add(row["code"])
    for path in filter(None, os.environ.get("CRS_ESRI_SWEEP_OUT", "").split(":")):
        from pyproj import Geod
        geod = Geod(ellps="WGS84")
        for row in json.load(open(path)):
            if row.get("g") is None:
                continue
            if geod.inv(row["ex"], row["ey"], *row["g"])[2] > 0.05:
                crs = CRS.from_epsg(row["code"])
                if crs.is_geographic:
                    bad.add(row["code"])
    db = sqlite3.connect(os.path.join(pyproj.datadir.get_data_dir(), "proj.db"))
    official, aliases = {}, {}
    for code, datum in db.execute(
        "select code, datum_code from geodetic_crs where auth_name='EPSG' and type='geographic 2D' "
        "and datum_auth_name='EPSG' and deprecated=0"
    ):
        if int(code) not in codes:
            continue
        meridian = CRS.from_epsg(int(code)).prime_meridian
        pm = round(meridian.longitude * meridian.unit_conversion_factor * 180 / 3.141592653589793, 6)
        name = db.execute("select name from geodetic_datum where auth_name='EPSG' and code=?", (datum,)).fetchone()[0]
        official.setdefault((name.lower().replace("_", " "), pm), set()).add(int(code))
        for (alt,) in db.execute(
            "select alt_name from alias_name where table_name='geodetic_datum' and auth_name='EPSG' and code=?", (datum,)
        ):
            aliases.setdefault((alt.lower().replace("_", " "), pm), set()).add(int(code))
    rows = {k: min(v) for k, v in official.items() if len(v) == 1}
    for k, v in aliases.items():
        # an official name wins over an alias; an alias two datums share says nothing
        if k not in official and len(v) == 1:
            rows[k] = min(v)
    lines = [
        '    ("%s", %r, %d, %s),' % (k[0].replace("\\", "\\\\").replace('"', '\\"'), k[1] + 0.0, rows[k], "false" if rows[k] in bad else "true")
        for k in sorted(rows)
    ]
    header = (
        "//! Generated by test/reference/generate_crs_datum.py (`datum_name_table`) from the PROJ database\n"
        "//! (pyproj %s, PROJ %s): the datum names and aliases (EPSG and ESRI `D_...`) of the geographic CRSs\n"
        "//! of `crs_names.rs`, lower-case with `_` as a space, with the longitude of the prime meridian in\n"
        "//! degrees, to the EPSG code of that CRS and whether its by-code answer agrees with pyproj.\n\n"
        "pub(crate) const DATUM_NAMES: &[(&str, f64, u32, bool)] = &[\n" % (pyproj.__version__, pyproj.proj_version_str)
    )
    open(os.path.join(src, "crs_datum_names.rs"), "w").write(header + "\n".join(lines) + "\n];\n")
    return len(lines)


EXTRA_TYPES = ("GEOGRAPHIC_2D_CRS", "GEOGRAPHIC_3D_CRS", "GEODETIC_CRS", "PROJECTED_CRS", "COMPOUND_CRS")


def table_codes():
    """The EPSG codes of the bundled `crs-definitions` table (read from its source)."""
    import glob
    import re
    path = glob.glob(os.path.expanduser("~/.cargo/registry/src/*/crs-definitions-0.6.0/src/from_code.rs"))[0]
    codes = set(int(m) for m in re.findall(r"^\s*(\d+) => EPSG", open(path).read(), re.M))
    # entries the table leaves without a proj string are served from the supplementary table
    empty = set(int(m) for m in re.findall(r'^EPSG_\d+\|(\d+)\|""\|', open(path.replace("from_code.rs", "defs.rs")).read(), re.M))
    return codes - empty


def extra_definitions():
    """PROJJSON of every non-deprecated EPSG geographic, geocentric, projected and compound CRS
    the bundled table lacks, minified, without the text fields (scope, area, remarks, bbox) that
    do not matter to a transformation."""
    from pyproj.database import query_crs_info
    from pyproj.enums import PJType
    have = table_codes()
    out = {}
    for name in EXTRA_TYPES:
        for info in query_crs_info("EPSG", pj_types=[getattr(PJType, name)]):
            code = int(info.code)
            if info.deprecated or code in have or code in out:
                continue
            try:
                crs = CRS.from_epsg(code)
                if crs.is_vertical and not crs.is_compound:
                    continue
                if crs.is_geographic and crs.is_derived:
                    continue
                pj = crs.to_json_dict()
            except Exception:
                continue

            def strip(node):
                if isinstance(node, dict):
                    return {k: strip(v) for k, v in node.items() if k not in ("$schema", "scope", "area", "bbox", "remarks", "usages")}
                if isinstance(node, list):
                    return [strip(v) for v in node]
                return node
            out[code] = strip(pj)
    return out


def generate_extra_table():
    """Write native/geoframe_core/src/crs_extra.bin: `code<TAB>PROJJSON` lines, deflate-compressed."""
    import zlib
    defs = extra_definitions()
    text = "".join("%d\t%s\n" % (c, json.dumps(defs[c], separators=(",", ":"), ensure_ascii=False)) for c in sorted(defs))
    raw = text.encode("utf-8")
    packed = zlib.compress(raw, 9)
    path = os.path.join(HERE, "..", "..", "native", "geoframe_core", "src", "crs_extra.bin")
    open(path, "wb").write(packed)
    return len(defs), len(raw), len(packed)


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
    fb = CRS.from_epsg(4301)  # no grid installed: the operation PROJ picks by area of use
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
    if "parity" in which:
        generate_parity()
    if "convert" in which:
        generate_convert()
    if "extra_table" in which:
        print("extra table: CRSs, bytes, compressed", generate_extra_table())
    if "name_table" in which:
        print("name table rows", generate_name_table())
    if "datum_name_table" in which:
        print("datum name table rows", generate_datum_name_table())
    if "esri_wkt" in which:
        generate_esri_wkt()
    if "helmert_table" in which:
        print("helmert table: CRSs, lists, ops, skipped ops", generate_helmert_table())
    if "grid_table" in which:
        print("grid table: CRSs, grid names", generate_grid_table())
    if "utm_table" in which:
        print("utm table rows", generate_utm_table())
