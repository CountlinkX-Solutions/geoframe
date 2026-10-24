"""Dev-only EPSG sweep of `to_crs` against pyproj (issue #62 / #97; run with `python -I`).

For every non-deprecated geographic 2D and projected EPSG CRS, takes three points inside its
area of use, converts them to the CRS with pyproj, then to EPSG:4326 with pyproj (no grids,
`always_xy`) and with GeoFrame (`cargo build --release --example crs_sweep` in
native/geoframe_core). Prints the distribution of the deviation and writes the per-code results.

    python -I test/reference/sweep_crs.py OUT.json        # run
    python -I test/reference/sweep_crs.py --summary OUT.json

With CRS_SWEEP_FORM=projjson (or wkt2) GeoFrame is given each CRS as its PROJJSON (WKT2) definition
without the top-level identifier instead of `EPSG:<code>`, which tests the definition converter
(issue #100) on the whole registry.
"""
import json
import math
import os
import re
import subprocess
import sys
import warnings

import numpy as np
from pyproj import Transformer
from pyproj.database import query_crs_info
from pyproj.enums import PJType

warnings.filterwarnings("ignore")
HERE = os.path.dirname(os.path.abspath(__file__))
EXE = os.environ.get('CRS_SWEEP_EXE') or os.path.join(HERE, '..', '..', 'target', 'release', 'examples', 'crs_sweep')


def metres(lon1, lat1, lon2, lat2):
    dlat = math.radians(lat2 - lat1)
    dlon = math.radians(((lon2 - lon1 + 180) % 360) - 180)
    a = math.sin(dlat / 2) ** 2 + math.cos(math.radians(lat1)) * math.cos(math.radians(lat2)) * math.sin(dlon / 2) ** 2
    return 2 * 6371008.8 * math.asin(min(1.0, math.sqrt(a)))


def cases():
    infos = query_crs_info("EPSG", pj_types=[PJType.GEOGRAPHIC_2D_CRS, PJType.PROJECTED_CRS])
    for info in infos:
        if info.deprecated or info.area_of_use is None:
            continue
        w, s, e, n = info.area_of_use.bounds
        if w > e:
            e += 360
        pts = []
        for fx, fy in ((0.5, 0.5), (0.25, 0.3), (0.75, 0.7)):
            lon = w + fx * (e - w)
            lon = ((lon + 180) % 360) - 180
            pts.append((lon, s + fy * (n - s)))
        yield info, pts


def pyproj_rows():
    rows = []
    for info, pts in cases():
        code = info.code
        label = f"EPSG:{code}"
        try:
            fwd = Transformer.from_crs("EPSG:4326", label, always_xy=True)
            back = Transformer.from_crs(label, "EPSG:4326", always_xy=True)
        except Exception:
            continue
        for lon, lat in pts:
            try:
                x, y = fwd.transform(lon, lat)
                tx, ty = back.transform(x, y)
            except Exception:
                continue
            if not all(math.isfinite(v) for v in (x, y, tx, ty)):
                continue
            rows.append({"code": int(code), "name": info.name, "x": x, "y": y, "truth": [tx, ty]})
    return rows


def definition(code, form):
    """The CRS as GeoFrame is given it: its label, or a definition with no identifier (spaces are
    written %20 so that the line stays three whitespace-separated words)."""
    if form == "code":
        return f"EPSG:{code}"
    import re
    from pyproj import CRS
    crs = CRS.from_epsg(code)
    if form == "projjson":
        pj = crs.to_json_dict()
        pj.pop("id", None)
        text = json.dumps(pj, separators=(",", ":"))
    else:
        text = re.sub(r',ID\["EPSG",\d+\]\]$', "]", crs.to_wkt())
    return re.sub(r"\s", "%20", text)


def main(out_path):
    # The pyproj side takes minutes: CRS_SWEEP_TRUTH names a file that caches it between runs.
    cache = os.environ.get("CRS_SWEEP_TRUTH")
    if cache and os.path.exists(cache):
        rows = json.load(open(cache))
    else:
        rows = pyproj_rows()
        if cache:
            json.dump(rows, open(cache, "w"))
    form = os.environ.get("CRS_SWEEP_FORM", "code")
    jobs = [f"{definition(r['code'], form)} {r['x']!r} {r['y']!r}" for r in rows]
    proc = subprocess.run([EXE], input="\n".join(jobs) + "\n", capture_output=True, text=True)
    lines = proc.stdout.splitlines()
    assert len(lines) == len(rows), (len(lines), len(rows), proc.stderr[-500:])
    for row, line in zip(rows, lines):
        if line.startswith("ERR"):
            row["err"] = line[4:]
        else:
            gx, gy = map(float, line.split())
            row["got"] = [gx, gy]
            row["dev"] = metres(gx, gy, *row["truth"])
    with open(out_path, "w") as f:
        json.dump(rows, f)
    summarize(rows)


def summarize(rows):
    by = {}
    for row in rows:
        by.setdefault(row["code"], []).append(row)
    summary = {"codes": len(by), "fail": 0, "le5": 0, "le50": 0, "km": 0, "mid": 0}
    worst = []
    errs = {}
    for code, rs in by.items():
        if any("err" in r for r in rs) and not any("dev" in r for r in rs):
            summary["fail"] += 1
            errs.setdefault(re.sub(r"(EPSG:|code )\d+", "", rs[0]["err"])[:70], []).append(code)
            continue
        d = max(r["dev"] for r in rs if "dev" in r)
        worst.append((d, code, rs[0]["name"]))
        if d <= 5:
            summary["le5"] += 1
        elif d <= 50:
            summary["le50"] += 1
        elif d >= 1000:
            summary["km"] += 1
        else:
            summary["mid"] += 1
    compared = summary["codes"] - summary["fail"]
    print(json.dumps(summary))
    print(f"compared {compared}: <=5 m {summary['le5']}, 5-50 m {summary['le50']}, 50 m-1 km {summary['mid']}, >=1 km {summary['km']}, failed {summary['fail']}")
    for msg, codes in sorted(errs.items(), key=lambda kv: -len(kv[1])):
        print(f"  fail {len(codes):4d} {msg}  e.g. {codes[:6]}")
    worst.sort(reverse=True)
    for d, code, name in worst[:40]:
        print(f"  {d:12.1f} m  EPSG:{code} {name}")


if __name__ == "__main__":
    if sys.argv[1] == "--summary":
        summarize(json.load(open(sys.argv[2])))
    else:
        main(sys.argv[1])
