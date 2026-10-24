"""Dev-only generator of the secondary-geometry verb fixtures (issue #93).

GeoPandas 1.2 carries every geometry-dtype column other than the active one through
`sjoin`, `sjoin_nearest`, `overlay`, `clip` and `dissolve` as an ordinary column. This script
runs those verbs on frames that have secondary geometry columns and records the result in
test/reference/fixtures/multi_verbs.json, which test/geo_frame/multi_geometry_verbs_test.exs
compares with GeoFrame (CI needs no Python).

Run:  ~/.venvs/geopandas-ref/bin/python -I test/reference/generate_multi_verbs.py
"""

import json
import os
import warnings

import geopandas as gpd
import pandas as pd
from shapely import wkt as swkt

warnings.simplefilter("ignore")
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "fixtures", "multi_verbs.json")


def pt(x, y):
    return f"POINT({x} {y})"


def line(*xy):
    return "LINESTRING(" + ",".join(f"{x} {y}" for x, y in xy) + ")"


# A frame is {"attrs": [[name, values]], "geoms": [(name, crs, [wkt|None])]}, the active column
# first; attributes are pairs because JSON objects are unordered.
def frame(attrs, geoms):
    return {"attrs": [[k, v] for k, v in attrs.items()], "geoms": [{"name": n, "crs": c, "wkt": w} for n, c, w in geoms]}


def to_gdf(f):
    series = {}
    for g in f["geoms"]:
        series[g["name"]] = gpd.GeoSeries(
            [None if w is None else swkt.loads(w) for w in g["wkt"]],
            crs=g["crs"],
        )
    first = f["geoms"][0]["name"]
    df = pd.DataFrame(dict(f["attrs"]))
    df[first] = series[first]
    gdf = gpd.GeoDataFrame(df, geometry=first)
    for name, s in series.items():
        if name != first:
            gdf[name] = s
    return gdf


def py(value):
    if value is None or (not isinstance(value, str) and pd.isna(value)):
        return None
    if hasattr(value, "item"):
        return value.item()
    return value


def result(gdf):
    """Attributes in column order, every geometry column with its CRS, and the pandas order."""
    attrs, geoms = {}, {}
    for col in gdf.columns:
        s = gdf[col]
        if str(s.dtype) == "geometry":
            geoms[col] = {
                "crs": None if s.crs is None else s.crs.to_string(),
                "wkt": [None if g is None else g.wkt for g in s],
            }
        else:
            attrs[col] = [py(v) for v in s.astype(object)]
    return {"columns": list(gdf.columns), "active": gdf.geometry.name, "attrs": attrs, "geoms": geoms}


SQ = lambda x, y, s=2: f"POLYGON(({x} {y},{x + s} {y},{x + s} {y + s},{x} {y + s},{x} {y}))"

# polygons with two secondary columns: `pt` (EPSG:3857, one null) and `alt` (no CRS)
A = frame(
    {"id": [1, 2, 3], "name": ["a", "b", "c"]},
    [
        ("geometry", "EPSG:4326", [SQ(0, 0), SQ(2, 0), SQ(10, 10)]),
        ("pt", "EPSG:3857", [pt(100, 100), None, pt(300, 300)]),
        ("alt", None, [line((0, 0), (1, 1)), line((2, 2), (3, 3)), None]),
    ],
)
# polygons whose `pt` collides with A's, plus a private `other`
B = frame(
    {"rid": [10, 20, 30], "name": ["x", "y", "z"]},
    [
        ("geometry", "EPSG:4326", [SQ(1, 1), SQ(3, 1), SQ(20, 20)]),
        ("pt", "EPSG:3857", [pt(-1, -1), pt(-2, -2), pt(-3, -3)]),
        ("other", "EPSG:4269", [pt(7, 7), None, pt(9, 9)]),
    ],
)
# points: the left side of the joins
P = frame(
    {"pid": [1, 2, 3, 4]},
    [
        ("geometry", "EPSG:4326", [pt(0.5, 0.5), pt(2.5, 0.5), pt(15, 15), pt(1.5, 1.5)]),
        ("home", "EPSG:3857", [pt(5, 5), pt(6, 6), None, pt(8, 8)]),
    ],
)
# lines and points for the nearest join (unique distances plus a tie)
N = frame(
    {"nid": [1, 2, 3]},
    [
        ("geometry", "EPSG:4326", [pt(0, 0), pt(10, 0), pt(10, 10)]),
        ("tag", "EPSG:3857", [pt(1, 1), pt(2, 2), None]),
    ],
)
Q = frame(
    {"qid": [1, 2, 3]},
    [
        ("geometry", "EPSG:4326", [pt(1, 0), pt(9, 0), pt(5, 0)]),
        ("mark", "EPSG:3857", [pt(11, 11), pt(12, 12), pt(13, 13)]),
    ],
)
# dissolve input: groups with null secondary values
D = frame(
    {"grp": ["u", "u", "v", "v", "w"], "n": [1, 2, 3, 4, 5]},
    [
        ("geometry", "EPSG:4326", [SQ(0, 0), SQ(1, 0), SQ(5, 5), SQ(6, 5), SQ(9, 9)]),
        ("pt", "EPSG:3857", [None, pt(2, 2), pt(3, 3), pt(4, 4), None]),
        ("alt", None, [pt(1, 1), pt(1, 2), None, None, None]),
    ],
)

cases = []


def add(op, args, inputs, fn):
    gdfs = [to_gdf(f) for f in inputs]
    out = fn(*gdfs)
    cases.append(
        {
            "op": op,
            "args": args,
            "inputs": inputs,
            "expected": result(out.reset_index(drop=True) if "by" not in args else out.reset_index()),
        }
    )


for how in ["inner", "left", "right"]:
    add("sjoin", {"how": how}, [P, A], lambda l, r, how=how: gpd.sjoin(l, r, how=how))
    add("sjoin", {"how": how}, [A, B], lambda l, r, how=how: gpd.sjoin(l, r, how=how))
add(
    "sjoin",
    {"how": "inner", "lsuffix": "L", "rsuffix": "R"},
    [A, B],
    lambda l, r: gpd.sjoin(l, r, how="inner", lsuffix="L", rsuffix="R"),
)
add(
    "sjoin",
    {"how": "inner", "predicate": "within"},
    [P, A],
    lambda l, r: gpd.sjoin(l, r, how="inner", predicate="within"),
)
for how in ["inner", "left", "right"]:
    add(
        "sjoin_nearest",
        {"how": how, "distance_col": "d"},
        [N, Q],
        lambda l, r, how=how: gpd.sjoin_nearest(l, r, how=how, distance_col="d"),
    )
add(
    "sjoin_nearest",
    {"how": "inner", "max_distance": 2.0},
    [P, B],
    lambda l, r: gpd.sjoin_nearest(l, r, how="inner", max_distance=2.0),
)
for how in ["intersection", "union", "identity", "symmetric_difference", "difference"]:
    add(
        "overlay",
        {"how": how, "keep_geom_type": False},
        [A, B],
        lambda l, r, how=how: gpd.overlay(l, r, how=how, keep_geom_type=False),
    )
add("clip", {"mask": [0.5, 0.5, 3, 3]}, [A], lambda g: gpd.clip(g, (0.5, 0.5, 3, 3)).sort_index())
add(
    "clip",
    {"mask": SQ(1, 0, 5)},
    [A],
    lambda g: gpd.clip(g, swkt.loads(SQ(1, 0, 5))).sort_index(),
)
add("dissolve", {"by": "grp"}, [D], lambda g: g.dissolve(by="grp"))
add("dissolve", {"by": "grp", "aggfunc": "last"}, [D], lambda g: g.dissolve(by="grp", aggfunc="last"))
add("dissolve", {"by": "grp", "aggfunc": "count"}, [D], lambda g: g.dissolve(by="grp", aggfunc="count"))
add(
    "dissolve",
    {"by": "grp", "aggfunc": {"n": "sum", "pt": "last"}},
    [D],
    lambda g: g.dissolve(by="grp", aggfunc={"n": "sum", "pt": "last"}),
)
add("dissolve", {}, [D], lambda g: g.dissolve())
add("dissolve", {"aggfunc": {"alt": "first"}}, [D], lambda g: g.dissolve(aggfunc={"alt": "first"}))

with open(OUT, "w") as f:
    json.dump(
        {"geopandas": gpd.__version__, "cases": cases},
        f,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )
    f.write("\n")
print(len(cases), "cases")
