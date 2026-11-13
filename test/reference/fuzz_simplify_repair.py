"""Fuzz `simplify(preserve_topology=False)` against Shapely where Douglas-Peucker leaves an
invalid polygon (GEOS repairs it with buffer(0)).

    python fuzz_simplify_repair.py gen  WORKDIR [N] [SEED]   # writes cases.tsv
    GF_FUZZ_IN=WORKDIR/cases.tsv GF_FUZZ_OUT=WORKDIR/out.tsv \\
      cargo test -p geoframe_core buffer_zero_tests::fuzz_simplify_repair -- --ignored --exact
    python fuzz_simplify_repair.py check WORKDIR              # compares out.tsv with Shapely
"""
import math
import random
import sys

import shapely
from shapely import wkt


def ring(pts):
    return pts + [pts[0]]


def fmt(v):
    return repr(float(v))


def poly_wkt(shell, holes=()):
    def r(c):
        return "(" + ", ".join(f"{fmt(x)} {fmt(y)}" for x, y in c) + ")"
    return "POLYGON (" + ", ".join([r(shell)] + [r(h) for h in holes]) + ")"


def star(rng, cx, cy, n, rmin, rmax):
    pts = []
    for i in range(n):
        a = 2 * math.pi * i / n
        r = rng.uniform(rmin, rmax)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return ring(pts)


def walk(rng, n, span):
    return ring([(rng.uniform(0, span), rng.uniform(0, span)) for _ in range(n)])


def lattice(rng, n, span):
    return ring([(float(rng.randint(0, span)), float(rng.randint(0, span))) for _ in range(n)])


def rough_polygon(rng):
    kind = rng.choice(["star", "star", "walk", "lattice", "holes"])
    if kind == "star":
        return poly_wkt(star(rng, 0, 0, rng.randint(8, 60), 3, 10))
    if kind == "walk":
        return poly_wkt(walk(rng, rng.randint(4, 12), 10))
    if kind == "lattice":
        return poly_wkt(lattice(rng, rng.randint(4, 9), 8))
    shell = star(rng, 0, 0, rng.randint(10, 40), 6, 10)
    holes = [star(rng, rng.uniform(-3, 3), rng.uniform(-3, 3), rng.randint(5, 14), 1, 4)
             for _ in range(rng.randint(1, 3))]
    return poly_wkt(shell, holes)


def rough_multipolygon(rng):
    parts = []
    # Now and then many lattice squares: touching and overlapping parts, many subgraphs.
    many = rng.random() < 0.2
    for _ in range(rng.randint(18, 30) if many else rng.randint(2, 4)):
        if many:
            x, y = float(rng.randint(0, 12)), float(rng.randint(0, 12))
            side = float(rng.randint(1, 3))
            parts.append(ring([(x, y), (x + side, y), (x + side, y + side), (x, y + side)]))
            continue
        if rng.random() < 0.5:
            parts.append(star(rng, rng.uniform(-4, 4), rng.uniform(-4, 4), rng.randint(6, 30), 2, 7))
        else:
            x, y = float(rng.randint(0, 4)), float(rng.randint(0, 4))
            w, h = float(rng.randint(1, 4)), float(rng.randint(1, 4))
            parts.append(ring([(x, y), (x + w, y), (x + w, y + h), (x, y + h)]))
    body = ", ".join("((" + ", ".join(f"{fmt(x)} {fmt(y)}" for x, y in p) + "))" for p in parts)
    return "MULTIPOLYGON (" + body + ")"


def gen(work, n, seed):
    rng = random.Random(seed)
    with open(f"{work}/cases.tsv", "w") as f:
        for i in range(n):
            w = rough_multipolygon(rng) if i % 3 == 0 else rough_polygon(rng)
            tol = rng.choice([0.0, 0.1, 0.5, 1.0, 1.5, 2.0, 3.0])
            f.write(f"{w}\t{tol}\n")


def check(work):
    cases = [l.rstrip("\n").split("\t") for l in open(f"{work}/cases.tsv")]
    outs = [l.rstrip("\n").split("\t", 1) for l in open(f"{work}/out.tsv")]
    stats = {"cases": 0, "repaired": 0, "agree_geom": 0, "differ_geom": 0, "exact": 0,
             "area_ok": 0, "unrepaired_differ": 0}
    differ = []
    for (w, tol), (rep, got) in zip(cases, outs):
        want = shapely.simplify(wkt.loads(w), float(tol), preserve_topology=False)
        got_g = wkt.loads(got) if got else wkt.loads("POLYGON EMPTY")
        stats["cases"] += 1
        repaired = rep == "1"
        stats["repaired"] += repaired
        same = shapely.normalize(want).equals_exact(shapely.normalize(got_g), 0)
        if repaired:
            stats["agree_geom"] += same
            stats["differ_geom"] += not same
            stats["exact"] += want.equals_exact(got_g, 0)
            stats["area_ok"] += math.isclose(want.area, got_g.area, rel_tol=1e-9, abs_tol=1e-9)
            if not same:
                differ.append((w, tol, want.wkt, got))
        elif not same:
            stats["unrepaired_differ"] += 1
            differ.append((w, tol, want.wkt, got))
    print(stats)
    for d in differ[:5]:
        print("DIFF", d)


if __name__ == "__main__":
    if sys.argv[1] == "gen":
        gen(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 3000,
            int(sys.argv[4]) if len(sys.argv) > 4 else 1)
    else:
        check(sys.argv[2])
