"""Random polygonal coverages with GeoPandas' answers, for the Rust fuzz tests in
`native/geoframe_core/src/coverage_tests.rs` (run it with `~/.venvs/geopandas-ref/bin/python -I`).

    fuzz_coverage.py validate SEED COUNT OUT.json

Each case holds the polygons (`rows`), a `gap` width, `valid` (`is_valid_coverage`) and
`edges` (`invalid_coverage_edges`, one WKT per row).
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
