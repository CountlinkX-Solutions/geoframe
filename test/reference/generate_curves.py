"""Dev-only generator of the curve fixtures (0.3.0, B10c).

GeoArrow and Shapely have no curve types, so GDAL linearizes curves on read (inscribed
vertices, `OGR_ARC_STEPSIZE` = 4 degrees, Z interpolated along the arc). This script builds a
GeoPackage whose rows hold curve geometries (ISO WKB types 8-12), reads it back with
pyogrio 0.13 (GDAL 3.12) and writes `fixtures/curves/curves.json`: per case the input WKB
(ISO, little and big endian, EWKB for some) and the linear WKB GDAL returned.

    ~/.venvs/geopandas-ref/bin/python -I generate_curves.py

GDAL drops M when it linearizes an arc, so M is not part of this reference; GeoFrame
interpolates M along the arc (see docs/parity.md).
"""
import json
import os
import sqlite3
import struct
import tempfile

import pyogrio
import shapely

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "fixtures", "curves")


def wkb(code, body, endian="<"):
    return struct.pack(endian + "BI", 1 if endian == "<" else 0, code) + body


def pts(coords, endian="<"):
    out = struct.pack(endian + "I", len(coords))
    for c in coords:
        out += struct.pack(endian + "d" * len(c), *c)
    return out


def circ(coords, z=False, endian="<"):
    return wkb(1008 if z else 8, pts(coords, endian), endian)


def line(coords, z=False, endian="<"):
    return wkb(1002 if z else 2, pts(coords, endian), endian)


def comp(members, z=False, endian="<"):
    return wkb(1009 if z else 9, struct.pack(endian + "I", len(members)) + b"".join(members), endian)


def cpoly(rings, z=False, endian="<"):
    return wkb(1010 if z else 10, struct.pack(endian + "I", len(rings)) + b"".join(rings), endian)


def coll(code, members, endian="<"):
    return wkb(code, struct.pack(endian + "I", len(members)) + b"".join(members), endian)


def poly(rings, z=False, endian="<"):
    body = struct.pack(endian + "I", len(rings)) + b"".join(pts(r, endian) for r in rings)
    return wkb(1003 if z else 3, body, endian)


def ewkb(iso, srid, z):
    """Rewrite a top-level little-endian ISO geometry as EWKB (flags + SRID)."""
    code = struct.unpack("<I", iso[1:5])[0]
    base = code % 1000
    new = base | 0x20000000 | (0x80000000 if z else 0)
    # Children keep ISO codes (the reader accepts both flavours per geometry).
    return iso[:1] + struct.pack("<II", new, srid) + iso[5:]


# A half circle, a circle, arcs in both directions and above 180 degrees.
HALF = [(0, 0), (1, 1), (2, 0)]
HALF_CW = [(2, 0), (1, 1), (0, 0)]
THREE_QUARTER = [(1, 0), (0, 1), (0.5, -0.9)]
FULL = [(0, 0), (1, 1), (0, 0)]
FULL_BIG = [(100.5, 200.25), (110.5, 200.25), (100.5, 200.25)]
NEAR_LINE = [(0, 0), (1, 1e-6), (2, 0)]
COLLINEAR = [(0, 0), (1, 1), (2, 2)]
TWO_ARCS = [(0, 0), (1, 1), (2, 0), (3, -1), (4, 0)]
SMALL_ARC = [(0, 0), (0.001, 0.0001), (0.002, 0)]
SAME_X = [(0, 0), (-1, 1), (0, 2)]
GEOGRAPHIC = [(-80.19, 25.76), (-80.1, 25.9), (-79.9, 25.8)]

CASES = {
    "circularstring_half": circ(HALF),
    "circularstring_half_clockwise": circ(HALF_CW),
    "circularstring_270": circ(THREE_QUARTER),
    "circularstring_two_arcs": circ(TWO_ARCS),
    "circularstring_same_x": circ(SAME_X),
    "circularstring_small": circ(SMALL_ARC),
    "circularstring_geographic": circ(GEOGRAPHIC),
    "circularstring_full_circle": circ(FULL),
    "circularstring_full_circle_offset": circ(FULL_BIG),
    "circularstring_collinear": circ(COLLINEAR),
    "circularstring_near_collinear": circ(NEAR_LINE),
    "circularstring_empty": wkb(8, struct.pack("<I", 0)),
    "circularstring_z_half": circ([(0, 0, 10), (1, 1, 20), (2, 0, 40)], z=True),
    "circularstring_z_full_circle": circ([(0, 0, 5), (1, 1, 7), (0, 0, 9)], z=True),
    "circularstring_z_collinear": circ([(0, 0, 1), (1, 1, 2), (2, 2, 6)], z=True),
    "circularstring_big_endian": circ(HALF, endian=">"),
    "compoundcurve_mixed": comp(
        [line([(-2, 0), (0, 0)]), circ(HALF), line([(2, 0), (2, -3)])]
    ),
    "compoundcurve_two_arcs": comp([circ(HALF), circ([(2, 0), (3, -1), (4, 0)])]),
    "compoundcurve_z": comp(
        [line([(-2, 0, 1), (0, 0, 2)], z=True), circ([(0, 0, 2), (1, 1, 4), (2, 0, 8)], z=True)],
        z=True,
    ),
    "compoundcurve_line_only": comp([line([(0, 0), (1, 1), (2, 0)])]),
    "curvepolygon_full_circle": cpoly([circ(FULL)]),
    "curvepolygon_compound_ring": cpoly(
        [comp([circ(HALF), line([(2, 0), (0, 0)])])]
    ),
    "curvepolygon_curved_hole": cpoly(
        [
            comp([circ([(-5, 0), (0, 5), (5, 0)]), line([(5, 0), (-5, 0)])]),
            circ([(-1, 0), (0, 0), (-1, 0)]),
        ]
    ),
    "curvepolygon_linear_hole": cpoly(
        [circ([(-5, 0), (5, 0), (-5, 0)]), line([(0, 0), (1, 0), (1, 1), (0, 0)])]
    ),
    "curvepolygon_z": cpoly(
        [comp([circ([(0, 0, 1), (1, 1, 2), (2, 0, 3)], z=True),
               line([(2, 0, 3), (0, 0, 1)], z=True)], z=True)],
        z=True,
    ),
    "multicurve": coll(
        11, [circ(HALF), line([(0, 0), (1, 1)]), comp([circ(HALF), line([(2, 0), (2, 2)])])]
    ),
    "multisurface": coll(
        12,
        [cpoly([circ(FULL)]), poly([[(10, 10), (11, 10), (11, 11), (10, 10)]])],
    ),
    "geometrycollection": coll(
        7, [circ(HALF), cpoly([circ(FULL)]), wkb(1, struct.pack("<dd", 1, 2)), line([(0, 0), (1, 1)])]
    ),
    "ewkb_srid_z": ewkb(circ([(0, 0, 10), (1, 1, 20), (2, 0, 40)], z=True), 4326, True),
    "ewkb_srid_2d": ewkb(circ(HALF), 4326, False),
}
# Cases whose GDAL result is read from the ISO input; EWKB inputs reuse the ISO case result.
EWKB_OF = {"ewkb_srid_z": "circularstring_z_half", "ewkb_srid_2d": "circularstring_half"}


def gp(blob):
    return b"GP" + bytes([0, 1]) + struct.pack("<i", 4326) + blob


def build(path, rows):
    c = sqlite3.connect(path)
    c.executescript(
        """
PRAGMA application_id=1196444487; PRAGMA user_version=10300;
CREATE TABLE gpkg_spatial_ref_sys(srs_name TEXT,srs_id INTEGER PRIMARY KEY,organization TEXT,organization_coordsys_id INTEGER,definition TEXT,description TEXT);
INSERT INTO gpkg_spatial_ref_sys VALUES('u',0,'NONE',0,'undefined',NULL),('u',-1,'NONE',-1,'undefined',NULL),('WGS 84',4326,'EPSG',4326,'GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]',NULL);
CREATE TABLE gpkg_contents(table_name TEXT PRIMARY KEY,data_type TEXT,identifier TEXT,description TEXT,last_change DATETIME DEFAULT '2020-01-01T00:00:00.000Z',min_x REAL,min_y REAL,max_x REAL,max_y REAL,srs_id INTEGER);
CREATE TABLE gpkg_geometry_columns(table_name TEXT,column_name TEXT,geometry_type_name TEXT,srs_id INTEGER,z TINYINT,m TINYINT);
CREATE TABLE curves(fid INTEGER PRIMARY KEY AUTOINCREMENT, geom BLOB, name TEXT);
INSERT INTO gpkg_contents(table_name,data_type,identifier,srs_id) VALUES('curves','features','curves',4326);
INSERT INTO gpkg_geometry_columns VALUES('curves','geom','GEOMETRY',4326,2,0);
"""
    )
    for name, blob in rows:
        c.execute("insert into curves(geom,name) values(?,?)", (gp(blob), name))
    c.commit()
    c.close()


def main():
    os.makedirs(OUT, exist_ok=True)
    gpkg = os.path.join(OUT, "curves.gpkg")
    if os.path.exists(gpkg):
        os.remove(gpkg)
    build(gpkg, [(n, b) for n, b in CASES.items() if n not in EWKB_OF])
    table = pyogrio.read_dataframe(gpkg)
    cases = []
    for name, geom in zip(table["name"], table.geometry):
        cases.append(
            {
                "name": name,
                "input": CASES[name].hex(),
                "expected": None
                if geom is None
                else shapely.to_wkb(geom, hex=True, output_dimension=3, byte_order=1, flavor="iso"),
                "type": None if geom is None else geom.geom_type,
            }
        )
    by_name = {c["name"]: c for c in cases}
    # pyogrio only linearizes a curve at the top level; GDAL's own getLinearGeometry (used when
    # a geometry collection is linearized) strokes each member, so the expected result is the
    # collection assembled from the members' linear results.
    gc = by_name["geometrycollection"]
    parts = [
        by_name["circularstring_half"]["expected"],
        by_name["curvepolygon_full_circle"]["expected"],
        wkb(1, struct.pack("<dd", 1, 2)).hex(),
        line([(0, 0), (1, 1)]).hex(),
    ]
    gc["expected"] = (struct.pack("<BII", 1, 7, len(parts)).hex()) + "".join(parts)
    gc["type"] = "GeometryCollection"
    for name, source in EWKB_OF.items():
        cases.append({**by_name[source], "name": name, "input": CASES[name].hex()})
    with open(os.path.join(OUT, "curves.json"), "w") as f:
        json.dump({"gdal": pyogrio.__gdal_version_string__, "cases": cases}, f, indent=1, sort_keys=True)
    print(len(cases), "cases,", os.path.getsize(gpkg), "bytes gpkg")


if __name__ == "__main__":
    main()
