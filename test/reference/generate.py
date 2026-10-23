"""Dev-only generator for the differential reference fixtures.

Computes GeoPandas 1.2 / Shapely 2.2 (GEOS 3.14) / pyproj 3.8 results for a fixed,
deterministic corpus and writes test/reference/fixtures/*.json. CI never runs this file:
the ExUnit suite (test/geo_frame/reference_test.exs) only reads the committed fixtures.

Run:  ~/.venvs/geopandas-ref/bin/python -I test/reference/generate.py
"""

import json
import math
import os
import sys

import geopandas as gpd
import pyproj
import shapely
from shapely import wkt as swkt

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "fixtures")

# --------------------------------------------------------------------------------------
# Corpus: id -> WKT (None is a null geometry). Lon/lat geometries use the `g_` prefix.
# --------------------------------------------------------------------------------------
CORPUS = {
    # points
    "pt1": "POINT (1 1)",
    "pt2": "POINT (3 4)",
    "pt_edge": "POINT (2 0)",
    "pt_far": "POINT (10 10)",
    "mpt": "MULTIPOINT ((0 0), (2 2), (5 1))",
    # lines
    "line_h": "LINESTRING (0 0, 10 0)",
    "line_diag": "LINESTRING (0 0, 3 4, 6 4)",
    "line_cross": "LINESTRING (2 -2, 2 6)",
    "line_closed": "LINESTRING (0 0, 4 0, 4 4, 0 0)",
    "line_self": "LINESTRING (0 0, 4 4, 4 0, 0 4)",
    "line_dup": "LINESTRING (0 0, 0 0, 2 0, 2 0, 5 0)",
    "mline": "MULTILINESTRING ((0 0, 2 0), (2 0, 4 0), (5 5, 6 6))",
    # polygons
    "sq": "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
    "sq_hole": "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
    "tri": "POLYGON ((0 0, 6 0, 3 5, 0 0))",
    "sq_overlap": "POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))",
    "sq_touch": "POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))",
    "sq_corner": "POLYGON ((4 4, 8 4, 8 8, 4 8, 4 4))",
    "sq_inner": "POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))",
    "sq_far": "POLYGON ((20 20, 22 20, 22 22, 20 22, 20 20))",
    "ell": "POLYGON ((0 0, 6 0, 6 2, 2 2, 2 6, 0 6, 0 0))",
    "sq_cw": "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))",
    "mpoly": "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((5 5, 8 5, 8 8, 5 8, 5 5)))",
    # collection, invalid and degenerate
    "coll": "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 2 2))",
    "bowtie": "POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))",
    "poly_flat": "POLYGON ((0 0, 1 1, 2 2, 0 0))",
    "line_zero": "LINESTRING (1 1, 1 1)",
    # empties and null
    "pt_empty": "POINT EMPTY",
    "line_empty": "LINESTRING EMPTY",
    "poly_empty": "POLYGON EMPTY",
    "coll_empty": "GEOMETRYCOLLECTION EMPTY",
    "null": None,
    # lon/lat near Miami and far apart
    "g_miami": "POINT (-80.1918 25.7617)",
    "g_fll": "POINT (-80.1373 26.1224)",
    "g_london": "POINT (-0.1276 51.5072)",
    "g_nyc": "POINT (-74.006 40.7128)",
    "g_sydney": "POINT (151.2093 -33.8688)",
    "g_line": "LINESTRING (-80.19 25.76, -80.14 26.12, -80.0 26.5)",
    "g_line_long": "LINESTRING (-80.19 25.76, -74.006 40.7128, -0.1276 51.5072)",
    "g_poly": "POLYGON ((-80.3 25.7, -80.1 25.7, -80.1 25.9, -80.3 25.9, -80.3 25.7))",
    "g_poly_hole": (
        "POLYGON ((-81 25, -79 25, -79 27, -81 27, -81 25), "
        "(-80.5 25.5, -80.2 25.5, -80.2 26, -80.5 26, -80.5 25.5))"
    ),
    "g_poly_far": "POLYGON ((10 10, 11 10, 11 11, 10 11, 10 10))",
}


def geom(cid):
    w = CORPUS[cid]
    return None if w is None else swkt.loads(w)


def full(g):
    """Full-precision WKT, or None."""
    if g is None:
        return None
    return shapely.to_wkt(g, rounding_precision=12, trim=True)


def num(v):
    """JSON-safe number: NaN/None -> None, numpy scalars -> python."""
    if v is None:
        return None
    f = float(v)
    if math.isnan(f) or math.isinf(f):
        return None
    return f


def conv(kind, v):
    if v is None:
        return None
    if kind == "bool":
        return bool(v)
    if kind == "int":
        return int(v)
    if kind == "float":
        return num(v)
    if kind == "string":
        return str(v)
    if kind == "geom":
        return full(v)
    if kind == "floats":
        return [num(x) for x in v]
    raise ValueError(kind)


def series(ids, crs=None):
    return gpd.GeoSeries([geom(i) for i in ids], crs=crs)


class Writer:
    def __init__(self, name):
        self.name = name
        self.sets = {}
        self.groups = []

    def add_set(self, name, rows):
        self.sets[name] = [list(r) for r in rows]

    def group(self, op, kind, set_name, fn, label=None, args=None, crs=None, per_row=False):
        """fn(a_series, b_series_or_None) -> iterable of values (or scalar when per_row)."""
        rows = self.sets[set_name]
        a_ids = [r[0] for r in rows]
        b_ids = [r[1] if len(r) > 1 else None for r in rows]
        has_b = any(b is not None for b in b_ids)
        out = []
        errors = 0
        if per_row:
            for a, b in zip(a_ids, b_ids):
                try:
                    out.append(conv(kind, fn(series([a], crs), series([b], crs) if has_b else None)))
                except Exception:
                    out.append(None)
                    errors += 1
        else:
            try:
                vals = list(fn(series(a_ids, crs), series(b_ids, crs) if has_b else None))
                out = [conv(kind, v) for v in vals]
            except Exception:
                for a, b in zip(a_ids, b_ids):
                    try:
                        v = list(fn(series([a], crs), series([b], crs) if has_b else None))[0]
                        out.append(conv(kind, v))
                    except Exception:
                        out.append({"error": True})
                        errors += 1
        self.groups.append(
            {
                "op": op,
                "label": label or op,
                "kind": kind,
                "set": set_name,
                "args": args or {},
                "crs": crs,
                "per_row": per_row,
                "e": out,
            }
        )

    def dump(self, extra=None):
        doc = {"sets": self.sets, "groups": self.groups}
        if extra:
            doc.update(extra)
        path = os.path.join(OUT, self.name + ".json")
        with open(path, "w") as f:
            json.dump(doc, f, sort_keys=True, separators=(",", ":"), allow_nan=False)
            f.write("\n")


def pairs(ids_a, ids_b=None):
    ids_b = ids_a if ids_b is None else ids_b
    return [(a, b) for a in ids_a for b in ids_b]


def singles(ids):
    return [(i,) for i in ids]


NON_EMPTY_2D = [
    k for k, w in CORPUS.items() if w is not None and "EMPTY" not in w and not k.startswith("g_")
]
EMPTIES = ["pt_empty", "line_empty", "poly_empty", "coll_empty", "null"]
GEO = [k for k in CORPUS if k.startswith("g_")]
VALID_POLY = ["sq", "sq_hole", "tri", "sq_overlap", "sq_touch", "sq_corner", "sq_inner",
              "sq_far", "ell", "sq_cw", "mpoly"]

PRED_IDS = ["pt1", "pt2", "pt_edge", "pt_far", "mpt", "line_h", "line_diag", "line_cross",
            "line_closed", "mline", "sq", "sq_hole", "tri", "sq_overlap", "sq_touch",
            "sq_corner", "sq_inner", "sq_far", "ell", "mpoly", "coll", "bowtie"]
SETOP_IDS = ["pt1", "pt_edge", "mpt", "line_h", "line_cross", "line_diag", "sq", "sq_hole",
             "tri", "sq_overlap", "sq_touch", "sq_corner", "sq_inner", "ell", "mpoly"]
NULLISH_PAIRS = [(a, b) for a in EMPTIES for b in ["sq", "pt1", "line_h"]] + [
    (a, e) for a in ["sq", "pt1"] for e in EMPTIES
]


# --------------------------------------------------------------------------------------
# measures
# --------------------------------------------------------------------------------------
def build_measures():
    w = Writer("measures")
    allv = NON_EMPTY_2D + EMPTIES
    w.add_set("all", singles(allv))
    w.add_set("all_nonnull", singles([i for i in allv if i != "null"]))
    w.add_set("dist", pairs(["pt1", "pt2", "line_h", "line_diag", "sq", "sq_hole", "sq_far", "mpoly", "coll"]) + NULLISH_PAIRS)
    w.add_set("hd", pairs(["pt1", "line_h", "line_diag", "line_cross", "sq", "tri", "sq_far", "ell", "mpoly"]))
    w.add_set("fd", pairs(["line_h", "line_diag", "line_cross", "line_closed", "line_self", "line_dup"]))
    w.group("area", "float", "all", lambda a, b: a.area)
    w.group("length", "float", "all", lambda a, b: a.length)
    w.group("geom_type", "string", "all", lambda a, b: a.geom_type)
    w.group("x", "float", "all", lambda a, b: [g.x if g is not None and g.geom_type == "Point" and not g.is_empty else None for g in a])
    w.group("y", "float", "all", lambda a, b: [g.y if g is not None and g.geom_type == "Point" and not g.is_empty else None for g in a])
    w.group("count_coordinates", "int", "all", lambda a, b: a.count_coordinates())
    w.group("count_geometries", "int", "all", lambda a, b: a.count_geometries())
    w.group("count_interior_rings", "int", "all", lambda a, b: a.count_interior_rings())
    w.group("total_bounds", "floats", "all_nonnull", lambda a, b: a.total_bounds, per_row=True)
    w.group("bounds", "floats", "all", lambda a, b: a.bounds.values.tolist(), label="bounds")
    w.group("distance", "float", "dist", lambda a, b: a.distance(b, align=False))
    w.group("hausdorff_distance", "float", "hd", lambda a, b: a.hausdorff_distance(b, align=False))
    w.group("hausdorff_distance", "float", "hd", lambda a, b: a.hausdorff_distance(b, densify=0.25, align=False),
            label="hausdorff_distance(densify=0.25)", args={"densify": 0.25})
    w.group("frechet_distance", "float", "fd", lambda a, b: a.frechet_distance(b, align=False))
    w.group("frechet_distance", "float", "fd", lambda a, b: a.frechet_distance(b, densify=0.5, align=False),
            label="frechet_distance(densify=0.5)", args={"densify": 0.5})
    w.dump()


# --------------------------------------------------------------------------------------
# unary predicates
# --------------------------------------------------------------------------------------
def build_unary_predicates():
    w = Writer("unary_predicates")
    allv = NON_EMPTY_2D + EMPTIES
    w.add_set("all", singles(allv))
    w.group("is_valid", "bool", "all", lambda a, b: a.is_valid)
    w.group("is_empty", "bool", "all", lambda a, b: a.is_empty)
    w.group("is_ring", "bool", "all", lambda a, b: a.is_ring)
    w.group("is_closed", "bool", "all", lambda a, b: a.is_closed)
    w.group("is_ccw", "bool", "all", lambda a, b: a.is_ccw)
    w.group("is_valid_reason", "string", "all", lambda a, b: a.is_valid_reason())
    w.group("isna", "bool", "all", lambda a, b: a.isna())
    w.dump()


# --------------------------------------------------------------------------------------
# binary predicates
# --------------------------------------------------------------------------------------
def build_binary_predicates():
    w = Writer("binary_predicates")
    w.add_set("pred", pairs(PRED_IDS) + NULLISH_PAIRS)
    for op in ["intersects", "contains", "within", "touches", "crosses", "overlaps",
               "disjoint", "covers", "covered_by", "contains_properly"]:
        w.group(op, "bool", "pred", lambda a, b, op=op: getattr(a, op)(b, align=False))
    w.group("geom_equals", "bool", "pred", lambda a, b: a.geom_equals(b, align=False))
    w.group("geom_equals_exact", "bool", "pred", lambda a, b: a.geom_equals_exact(b, 1e-9, align=False),
            label="geom_equals_exact(1e-9)", args={"tolerance": 1e-9})
    w.group("geom_equals_identical", "bool", "pred", lambda a, b: a.geom_equals_identical(b, align=False))
    w.group("relate", "string", "pred", lambda a, b: a.relate(b, align=False))
    w.group("relate_pattern", "bool", "pred", lambda a, b: a.relate_pattern(b, "T*****FF*", align=False),
            label="relate_pattern(T*****FF*)", args={"pattern": "T*****FF*"})
    w.group("dwithin", "bool", "pred", lambda a, b: a.dwithin(b, 2.0, align=False),
            label="dwithin(2.0)", args={"distance": 2.0})
    w.dump()


# --------------------------------------------------------------------------------------
# constructive
# --------------------------------------------------------------------------------------
def build_constructive():
    w = Writer("constructive")
    allv = NON_EMPTY_2D + EMPTIES
    w.add_set("all", singles(allv))
    buf = ["pt1", "mpt", "line_h", "line_diag", "line_closed", "sq", "sq_hole", "tri", "ell", "mpoly"]
    w.add_set("buf", singles(buf))
    w.add_set("lines", singles(["line_h", "line_diag", "line_cross", "line_closed", "line_dup", "mline", "sq", "pt1", "g_line" if False else "line_self"]))
    w.add_set("polys", singles(["sq", "sq_hole", "sq_cw", "tri", "ell", "mpoly", "bowtie", "line_h", "pt1"]))
    for op in ["centroid", "convex_hull", "envelope", "boundary", "exterior", "extract_unique_points", "reverse"]:
        fn = {
            "centroid": lambda a, b: a.centroid,
            "convex_hull": lambda a, b: a.convex_hull,
            "envelope": lambda a, b: a.envelope,
            "boundary": lambda a, b: a.boundary,
            "exterior": lambda a, b: gpd.GeoSeries(shapely.get_exterior_ring(a.values), crs=None),
            "extract_unique_points": lambda a, b: a.extract_unique_points(),
            "reverse": lambda a, b: a.reverse(),
        }[op]
        w.group(op, "geom", "all", fn)
    w.group("representative_point", "geom", "all", lambda a, b: a.representative_point())
    w.group("make_valid", "geom", "all", lambda a, b: a.make_valid())
    w.group("interiors", "geom", "polys",
            lambda a, b: [
                None if g is None or g.geom_type != "Polygon" else
                shapely.multilinestrings([shapely.get_interior_ring(g, i) for i in range(shapely.get_num_interior_rings(g))])
                for g in a
            ],
            label="interiors")
    for d in [1.0, 0.5, -0.5]:
        w.group("buffer", "geom", "buf", lambda a, b, d=d: a.buffer(d), label=f"buffer({d})", args={"distance": d})
    w.group("buffer", "geom", "buf", lambda a, b: a.buffer(1.0, cap_style="flat", join_style="mitre"),
            label="buffer(1.0, flat, mitre)", args={"distance": 1.0, "cap_style": "flat", "join_style": "mitre"})
    w.group("buffer", "geom", "buf", lambda a, b: a.buffer(1.0, cap_style="square", join_style="bevel"),
            label="buffer(1.0, square, bevel)", args={"distance": 1.0, "cap_style": "square", "join_style": "bevel"})
    w.group("buffer", "geom", "buf", lambda a, b: a.buffer(1.0, quad_segs=4),
            label="buffer(1.0, quad_segs=4)", args={"distance": 1.0, "quad_segs": 4})
    w.add_set("simp", singles(["line_diag", "line_dup", "sq", "sq_hole", "ell", "tri", "mpoly", "line_self"]))
    w.group("simplify", "geom", "simp", lambda a, b: a.simplify(1.0, preserve_topology=False),
            label="simplify(1.0, preserve_topology=false)", args={"tolerance": 1.0, "preserve_topology": False})
    w.group("simplify", "geom", "simp", lambda a, b: a.simplify(0.5),
            label="simplify(0.5)", args={"tolerance": 0.5})
    w.add_set("seg", singles(["line_h", "line_diag", "sq", "sq_hole", "mline", "pt1"]))
    w.group("segmentize", "geom", "seg", lambda a, b: a.segmentize(2.5), label="segmentize(2.5)", args={"max_segment_length": 2.5})
    w.add_set("rr", singles(["line_dup", "line_diag", "sq", "mpt", "line_zero"]))
    w.group("remove_repeated_points", "geom", "rr", lambda a, b: a.remove_repeated_points(0.0), label="remove_repeated_points(0)", args={"tolerance": 0.0})
    w.group("remove_repeated_points", "geom", "rr", lambda a, b: a.remove_repeated_points(0.5), label="remove_repeated_points(0.5)", args={"tolerance": 0.5})
    w.group("orient_polygons", "geom", "polys", lambda a, b: a.orient_polygons(exterior_cw=False), label="orient_polygons(ccw)", args={"exterior_cw": False})
    w.group("orient_polygons", "geom", "polys", lambda a, b: a.orient_polygons(exterior_cw=True), label="orient_polygons(cw)", args={"exterior_cw": True})
    w.add_set("lm", singles(["mline", "line_h", "line_diag", "line_closed", "sq"]))
    w.group("line_merge", "geom", "lm", lambda a, b: a.line_merge(), label="line_merge")
    w.group("get_geometry", "geom", "all", lambda a, b: a.get_geometry(0), label="get_geometry(0)", args={"n": 0})
    w.add_set("sl", pairs(["pt1", "pt2", "line_h", "line_diag", "sq", "sq_far", "tri"]))
    w.group("shortest_line", "geom", "sl", lambda a, b: gpd.GeoSeries(shapely.shortest_line(a.values, b.values)))
    w.dump()


# --------------------------------------------------------------------------------------
# set operations
# --------------------------------------------------------------------------------------
def build_set_ops():
    w = Writer("set_ops")
    w.add_set("ops", pairs(SETOP_IDS))
    for op in ["intersection", "union", "difference", "symmetric_difference"]:
        w.group(op, "geom", "ops", lambda a, b, op=op: getattr(a, op)(b, align=False))
    w.add_set("union_all", [(i,) for i in ["sq", "sq_overlap", "sq_touch", "sq_hole"]])
    w.dump({"union_all": {"ids": ["sq", "sq_overlap", "sq_touch", "sq_hole"],
                          "e": full(series(["sq", "sq_overlap", "sq_touch", "sq_hole"]).union_all())}})


# --------------------------------------------------------------------------------------
# affine
# --------------------------------------------------------------------------------------
def build_affine():
    w = Writer("affine")
    ids = ["pt1", "mpt", "line_diag", "line_closed", "sq", "sq_hole", "tri", "ell", "mpoly", "coll"]
    w.add_set("a", singles(ids))
    w.group("translate", "geom", "a", lambda a, b: a.translate(2.5, -3.0), label="translate(2.5,-3)", args={"x": 2.5, "y": -3.0})
    w.group("scale", "geom", "a", lambda a, b: a.scale(2.0, 0.5), label="scale(2,0.5,center)", args={"x": 2.0, "y": 0.5, "origin": "center"})
    w.group("scale", "geom", "a", lambda a, b: a.scale(2.0, 0.5, origin=(1, 1)), label="scale(2,0.5,(1,1))", args={"x": 2.0, "y": 0.5, "origin": [1, 1]})
    w.group("scale", "geom", "a", lambda a, b: a.scale(-1.0, 3.0, origin="centroid"), label="scale(-1,3,centroid)", args={"x": -1.0, "y": 3.0, "origin": "centroid"})
    w.group("rotate", "geom", "a", lambda a, b: a.rotate(30), label="rotate(30,center)", args={"angle": 30, "origin": "center"})
    w.group("rotate", "geom", "a", lambda a, b: a.rotate(-75.5, origin=(0, 0)), label="rotate(-75.5,(0,0))", args={"angle": -75.5, "origin": [0, 0]})
    w.group("rotate", "geom", "a", lambda a, b: a.rotate(90, origin="centroid"), label="rotate(90,centroid)", args={"angle": 90, "origin": "centroid"})
    w.group("skew", "geom", "a", lambda a, b: a.skew(20, 10), label="skew(20,10,center)", args={"xs": 20, "ys": 10, "origin": "center"})
    w.group("skew", "geom", "a", lambda a, b: a.skew(15, 0, origin=(0, 0)), label="skew(15,0,(0,0))", args={"xs": 15, "ys": 0, "origin": [0, 0]})
    m = [1.5, 0.25, -0.5, 2.0, 3.0, -1.0]
    w.group("affine_transform", "geom", "a", lambda a, b: a.affine_transform(m), label="affine_transform", args={"matrix": m})
    w.dump()


# --------------------------------------------------------------------------------------
# linear referencing
# --------------------------------------------------------------------------------------
def build_linear_referencing():
    w = Writer("linear_referencing")
    lines = ["line_h", "line_diag", "line_cross", "line_closed", "line_dup", "mline", "line_self"]
    w.add_set("lines", singles(lines))
    for d in [2.5, 0.0, 100.0, -1.0]:
        w.group("interpolate", "geom", "lines", lambda a, b, d=d: a.interpolate(d), label=f"interpolate({d})", args={"distance": d})
    for d in [0.3, 1.0]:
        w.group("interpolate", "geom", "lines", lambda a, b, d=d: a.interpolate(d, normalized=True), label=f"interpolate({d},normalized)", args={"distance": d, "normalized": True})
    w.add_set("project", pairs(["line_h", "line_diag", "line_closed", "line_dup", "mline"], ["pt1", "pt2", "pt_edge", "pt_far"]))
    w.group("project", "float", "project", lambda a, b: a.project(b, align=False))
    w.group("project", "float", "project", lambda a, b: a.project(b, normalized=True, align=False), label="project(normalized)", args={"normalized": True})
    w.dump()


# --------------------------------------------------------------------------------------
# CRS
# --------------------------------------------------------------------------------------
def build_crs():
    w = Writer("crs")
    srcs = ["g_miami", "g_fll", "g_london", "g_nyc", "g_sydney", "g_line", "g_line_long", "g_poly", "g_poly_hole", "g_poly_far"]
    w.add_set("geo", singles(srcs))
    targets = ["EPSG:3857", "EPSG:32617", "EPSG:5070", "EPSG:2236"]
    extra = {}
    for t in targets:
        w.group("to_crs", "geom", "geo", lambda a, b, t=t: a.to_crs(t), label=f"to_crs(4326->{t})", args={"target": t}, crs="EPSG:4326")
        # inverse: project the Miami-area geometries first
        ids = ["g_miami", "g_fll", "g_line", "g_poly", "g_poly_hole"]
        inv = []
        for i in ids:
            cid = f"{i}@{t.split(':')[1]}"
            extra[cid] = full(series([i], "EPSG:4326").to_crs(t).iloc[0])
            inv.append(cid)
        setname = f"inv_{t.split(':')[1]}"
        w.add_set(setname, singles(inv))
        CORPUS.update({k: v for k, v in extra.items()})
        w.group("to_crs", "geom", setname, lambda a, b: a.to_crs("EPSG:4326"), label=f"to_crs({t}->4326)", args={"target": "EPSG:4326"}, crs=t)
    w.add_set("utm", singles(["g_miami", "g_london", "g_sydney", "g_nyc", "g_poly_far"]))
    w.group("estimate_utm_crs", "string", "utm", lambda a, b: a.estimate_utm_crs().to_string(), crs="EPSG:4326", per_row=True)
    w.dump()


# --------------------------------------------------------------------------------------
# geodesic (pyproj.Geod, WGS84)
# --------------------------------------------------------------------------------------
def build_geodesic():
    w = Writer("geodesic")
    geod = pyproj.Geod(ellps="WGS84")

    def ring_area(ring):
        return abs(geod.geometry_area_perimeter(shapely.Polygon(ring))[0])

    def polygon_area(g):
        return ring_area(g.exterior) - sum(ring_area(r) for r in g.interiors)

    def polygon_perimeter(g):
        return geod.geometry_length(g.exterior) + sum(geod.geometry_length(r) for r in g.interiors)

    def area(a, b):
        return [polygon_area(g) for g in a]

    def perimeter(a, b):
        return [geod.geometry_length(g) if g.geom_type != "Polygon" else polygon_perimeter(g) for g in a]

    w.add_set("polys", singles(["g_poly", "g_poly_hole", "g_poly_far"]))
    w.add_set("lines", singles(["g_line", "g_line_long"]))
    w.add_set("pts", pairs(["g_miami", "g_fll", "g_london", "g_nyc", "g_sydney"]))
    w.group("area_geodesic", "float", "polys", area, crs="EPSG:4326")
    w.group("length_geodesic", "float", "lines", perimeter, crs="EPSG:4326")
    w.group("length_geodesic", "float", "polys", perimeter, label="length_geodesic(polygon)", crs="EPSG:4326")

    def dist(a, b):
        return [geod.inv(p.x, p.y, q.x, q.y)[2] for p, q in zip(a, b)]

    w.group("distance_geodesic", "float", "pts", dist, crs="EPSG:4326")
    w.dump()


# --------------------------------------------------------------------------------------
# IO: WKB hex and GeoJSON
# --------------------------------------------------------------------------------------
def build_io():
    w = Writer("io")
    ids = [i for i in NON_EMPTY_2D if i not in ("poly_flat",)] + ["pt_empty", "null"]
    w.add_set("all", singles(ids))
    w.group("to_wkb_hex", "string", "all", lambda a, b: [None if g is None else shapely.to_wkb(g, hex=True, byte_order=1) for g in a])
    w.group("to_wkt_roundtrip", "geom", "all", lambda a, b: a)
    w.group("geojson", "string", "all",
            lambda a, b: [None if g is None or g.is_empty else json.dumps(shapely.geometry.mapping(g), sort_keys=True) for g in a])
    w.dump()


# --------------------------------------------------------------------------------------
# frames: sjoin / sjoin_nearest / dissolve
# --------------------------------------------------------------------------------------
def frames():
    left = [
        {"lid": "L1", "grp": "a", "v": 1.0, "g": "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"},
        {"lid": "L2", "grp": "a", "v": 2.0, "g": "POLYGON ((3 3, 7 3, 7 7, 3 7, 3 3))"},
        {"lid": "L3", "grp": "b", "v": 3.0, "g": "POLYGON ((10 10, 12 10, 12 12, 10 12, 10 10))"},
        {"lid": "L4", "grp": "b", "v": 4.5, "g": "POLYGON ((20 0, 22 0, 22 2, 20 2, 20 0))"},
        {"lid": "L5", "grp": "c", "v": 5.0, "g": "POINT (30 30)"},
    ]
    right = [
        {"rid": "R1", "g": "POINT (1 1)"},
        {"rid": "R2", "g": "POINT (3.5 3.5)"},
        {"rid": "R3", "g": "POINT (11 11)"},
        {"rid": "R4", "g": "POINT (50 50)"},
        {"rid": "R5", "g": "POLYGON ((6 6, 9 6, 9 9, 6 9, 6 6))"},
        {"rid": "R6", "g": "POINT (29 30)"},
        {"rid": "R7", "g": "POINT (31 30)"},
    ]
    return left, right


def gdf_of(rows, crs=None):
    import pandas as pd

    df = pd.DataFrame([{k: v for k, v in r.items() if k != "g"} for r in rows])
    return gpd.GeoDataFrame(df, geometry=[swkt.loads(r["g"]) for r in rows], crs=crs)


def build_frames():
    left, right = frames()
    lg, rg = gdf_of(left), gdf_of(right)
    out = {"left": left, "right": right, "sjoin": [], "sjoin_nearest": [], "dissolve": []}

    def pairs_of(df):
        return sorted(
            [[r["lid"], None if r["rid"] != r["rid"] else r["rid"]] for _, r in df.iterrows()],
            key=lambda p: (p[0] or "", p[1] or ""),
        )

    # right-join frames keep rid always; left ids may be NaN
    def pairs_right(df):
        return sorted(
            [[None if r["lid"] != r["lid"] else r["lid"], r["rid"]] for _, r in df.iterrows()],
            key=lambda p: (p[0] or "", p[1] or ""),
        )

    for pred in ["intersects", "within", "contains", "touches"]:
        for how in ["inner", "left", "right"]:
            j = gpd.sjoin(lg, rg, how=how, predicate=pred)
            out["sjoin"].append({"predicate": pred, "how": how, "distance": None,
                                 "e": pairs_right(j) if how == "right" else pairs_of(j)})
    for how in ["inner", "left", "right"]:
        j = gpd.sjoin(lg, rg, how=how, predicate="dwithin", distance=2.0)
        out["sjoin"].append({"predicate": "dwithin", "how": how, "distance": 2.0,
                             "e": pairs_right(j) if how == "right" else pairs_of(j)})

    def nearest_rows(j, right_how=False):
        rows = []
        for _, r in j.iterrows():
            lid = None if r["lid"] != r["lid"] else r["lid"]
            rid = None if r["rid"] != r["rid"] else r["rid"]
            d = None if r["dist"] != r["dist"] else float(r["dist"])
            rows.append([lid, rid, d])
        return sorted(rows, key=lambda p: (p[0] or "", p[1] or ""))

    for how in ["inner", "left", "right"]:
        for kw, args in [({}, {}), ({"max_distance": 5.0}, {"max_distance": 5.0})]:
            j = gpd.sjoin_nearest(lg, rg, how=how, distance_col="dist", **kw)
            out["sjoin_nearest"].append({"how": how, "args": args, "e": nearest_rows(j)})

    for by, agg in [("grp", "sum"), ("grp", "mean"), (None, "sum")]:
        cols = gdf_of(left)[["grp", "v", "geometry"]]
        d = cols.dissolve(by=by, aggfunc=agg)
        rows = []
        for idx, r in d.reset_index().iterrows() if by else d.iterrows():
            rows.append({"key": r["grp"] if by else None, "v": num(r["v"]), "g": full(r["geometry"])})
        out["dissolve"].append({"by": by, "aggfunc": agg, "e": rows})
    path = os.path.join(OUT, "frames.json")
    with open(path, "w") as f:
        json.dump(out, f, sort_keys=True, separators=(",", ":"), allow_nan=False)
        f.write("\n")


def main():
    os.makedirs(OUT, exist_ok=True)
    build_measures()
    build_unary_predicates()
    build_binary_predicates()
    build_constructive()
    build_set_ops()
    build_affine()
    build_linear_referencing()
    build_crs()
    build_geodesic()
    build_io()
    build_frames()
    corpus = dict(CORPUS)
    meta = {
        "geopandas": gpd.__version__,
        "shapely": shapely.__version__,
        "geos": shapely.geos_version_string,
        "pyproj": pyproj.__version__,
        "proj": pyproj.proj_version_str,
    }
    with open(os.path.join(OUT, "corpus.json"), "w") as f:
        json.dump({"meta": meta, "corpus": corpus}, f, sort_keys=True, separators=(",", ":"))
        f.write("\n")


if __name__ == "__main__":
    sys.exit(main())
