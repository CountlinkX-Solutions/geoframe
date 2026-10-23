"""Random polygonal coverages with GeoPandas' answers, for the Rust fuzz tests in
`native/geoframe_core/src/coverage_tests.rs` (run it with `~/.venvs/geopandas-ref/bin/python -I`).

    fuzz_coverage.py validate SEED COUNT OUT.json
    fuzz_coverage.py simplify SEED COUNT OUT.json

Each case holds the polygons (`rows`), a `gap` width, `valid` (`is_valid_coverage`) and
`edges` (`invalid_coverage_edges`, one WKT per row). The `simplify` cases hold the rows, a
`tol`erance, `boundary` (`simplify_boundary`) and the `expected` rows of `simplify_coverage`.
"""
import json, random, sys
import geopandas as gpd
from shapely import wkt, Polygon, MultiPoint
from shapely.geometry import box
mode_name = sys.argv[1]
random.seed(int(sys.argv[2]))
N=int(sys.argv[3])
def hull(npts, g=0.5, lim=6):
    while True:
        pts=[(random.randint(0,int(lim/g))*g, random.randint(0,int(lim/g))*g) for _ in range(npts)]
        h=MultiPoint(pts).convex_hull
        if h.geom_type=="Polygon" and h.area>0: return h
def cut(poly):
    # split a polygon by a line into 2 valid coverage pieces, sharing a node-matched edge
    from shapely import ops, LineString
    minx,miny,maxx,maxy=poly.bounds
    for _ in range(20):
        if random.random()<0.5:
            x=random.choice([minx+(maxx-minx)*f for f in (0.3,0.5,0.7)])
            line=LineString([(x,miny-1),(x,maxy+1)])
        else:
            a=(random.uniform(minx,maxx),miny-1); b=(random.uniform(minx,maxx),maxy+1)
            line=LineString([a,b])
        parts=ops.split(poly,line).geoms
        if len(parts)==2: return list(parts)
    return [poly]
def perturb(polys):
    polys=list(polys)
    r=random.random()
    i=random.randrange(len(polys))
    p=polys[i]
    if p.geom_type!="Polygon": return polys
    if r<0.2:
        # shift by a small offset (gap or overlap)
        from shapely import affinity
        d=random.choice([0.05,0.1,0.2,0.4,-0.1,-0.3])
        polys[i]=affinity.translate(p,d,0) if random.random()<.5 else affinity.translate(p,0,d)
    elif r<0.4:
        # add a midpoint vertex on a random segment (hanging vertex)
        c=list(p.exterior.coords)[:-1]
        k=random.randrange(len(c)); a=c[k]; b=c[(k+1)%len(c)]
        m=((a[0]+b[0])/2,(a[1]+b[1])/2)
        c.insert(k+1,m)
        polys[i]=Polygon(c)
    elif r<0.5:
        polys[i]=Polygon(list(p.exterior.coords)[::-1])
    elif r<0.6:
        polys.append(polys[i])
    elif r<0.7:
        polys[i]=p.buffer(-0.15,join_style=2)
    return polys
def scramble(poly):
    """The same polygon with every ring started elsewhere and, at random, reversed."""
    def ring(r):
        c = list(r.coords)[:-1]
        k = random.randrange(len(c))
        c = c[k:] + c[:k]
        if random.random() < 0.5:
            c.reverse()
        return c
    return Polygon(ring(poly.exterior), [ring(h) for h in poly.interiors])


def simplify_cases():
    import shapely
    from shapely import LineString, MultiLineString, MultiPolygon, unary_union, polygonize
    cases = []
    while len(cases) < N:
        nx, ny = random.randint(2, 4), random.randint(2, 4)
        step = random.choice([1.0, 2.0])
        jitter = random.choice([0.0, 0.15, 0.3]) * step
        amp = random.choice([0.12, 0.3])
        pts = {(i, j): (i*step + random.uniform(-jitter, jitter), j*step + random.uniform(-jitter, jitter))
               for i in range(nx+1) for j in range(ny+1)}
        # border points stay on the box so the outer boundary is straight-ish
        def wiggle(a, b, k):
            out = [a]
            for t in range(1, k+1):
                f = t/(k+1)
                dx, dy = b[0]-a[0], b[1]-a[1]
                L = (dx*dx+dy*dy) ** .5 or 1
                off = random.uniform(-amp, amp) * step
                out.append((a[0]+f*dx - dy/L*off, a[1]+f*dy + dx/L*off))
            out.append(b)
            return out
        lines = []
        for i in range(nx+1):
            for j in range(ny):
                lines.append(LineString(wiggle(pts[(i, j)], pts[(i, j+1)], random.randint(0, 8))))
        for j in range(ny+1):
            for i in range(nx):
                lines.append(LineString(wiggle(pts[(i, j)], pts[(i+1, j)], random.randint(0, 8))))
        if random.random() < 0.3:
            # an island: a closed ring inside a cell
            cx, cy = random.randrange(nx)*step + step/2, random.randrange(ny)*step + step/2
            r = step*0.2
            lines.append(LineString([(cx-r, cy-r), (cx+r, cy-r*0.8), (cx+r*0.7, cy+r), (cx-r, cy+r*0.9), (cx-r, cy-r)]))
        if random.random() < 0.3 and lines:
            # drop an edge so two cells merge into one polygon
            lines.pop(random.randrange(len(lines)))
        noded = unary_union(MultiLineString(lines))
        polys = list(polygonize(list(noded.geoms)).geoms)
        if len(polys) < 2:
            continue
        random.shuffle(polys)
        rows = [scramble(p) for p in polys] if random.random() < 0.5 else polys
        if random.random() < 0.3 and len(rows) >= 4:
            rows = [MultiPolygon([rows[0], rows[1]])] + rows[2:]   # a multi-part row (touching parts allowed)
        s = gpd.GeoSeries(rows)
        for tol in random.sample([0.0, 0.02, 0.05, 0.1, 0.2, 0.4, 0.8, 1.5, 3.0], 3):
            for boundary in (True, False):
                try:
                    out = s.simplify_coverage(tol, simplify_boundary=boundary)
                except Exception:
                    continue
                cases.append({"rows": [g.wkt for g in rows], "tol": tol, "boundary": boundary,
                              "expected": [g.wkt for g in out]})
    return cases


if mode_name == "simplify":
    cases = simplify_cases()
    json.dump(cases, open(sys.argv[4], "w"))
    print(len(cases), "cases")
    sys.exit(0)

cases=[]
while len(cases)<N:
    mode=random.random()
    if mode<0.3:
        polys=[hull(random.randint(3,6)) for _ in range(random.randint(2,3))]
    elif mode<0.7:
        base=hull(random.randint(4,7),g=0.5)
        parts=cut(base)
        if random.random()<0.4 and len(parts)==2:
            parts=cut(parts[0])+[parts[1]]
        polys=perturb(parts)
        if random.random()<.3: polys=perturb(polys)
    else:
        # grid of cells with gaps
        w=random.choice([1,2]); polys=[]
        for ix in range(random.randint(1,3)):
            for iy in range(random.randint(1,2)):
                x0=ix*w+random.choice([0,0,0,0.1,0.25]); y0=iy*w+random.choice([0,0,0,0.1,-0.2])
                x1=(ix+1)*w+random.choice([0,0,0.1,0.3]); y1=(iy+1)*w
                polys.append(box(x0,y0,x1,y1))
        random.shuffle(polys)
        polys=perturb(polys)
    if random.random()<0.5:
        # holes and multipolygons
        big=box(0,0,6,6); h=hull(random.randint(3,5),lim=4); h=h.buffer(0.0)
        from shapely import affinity
        h=affinity.translate(h,1,1)
        if random.random()<0.5:
            holed=big.difference(h)
            polys=[holed,h] if random.random()<0.7 else [holed,affinity.translate(h,random.choice([0.1,0.3,-0.2]),0)]
        else:
            parts=[box(0,0,2,2),box(3,3,5,5)]
            from shapely import MultiPolygon
            polys=[MultiPolygon(parts),box(2,0,3,3),box(2,2.5,3.5,3.2)]
        random.shuffle(polys)
    polys=[p for p in polys if not p.is_empty and p.geom_type in ("Polygon","MultiPolygon")]
    if len(polys)<2: continue
    for gap in random.sample([0,0,0.1,0.3,0.6,1.5],2):
        s=gpd.GeoSeries(polys)
        cases.append({"rows":[p.wkt for p in polys],"gap":gap,"valid":bool(s.is_valid_coverage(gap_width=gap)),
          "edges":[g.wkt for g in s.invalid_coverage_edges(gap_width=gap)]})
json.dump(cases,open(sys.argv[4],"w"))
print(len(cases), sum(not c["valid"] for c in cases), "invalid")
