"""Dev-only GeoPandas side of the PostGIS cross-check (see crosscheck_postgis.sh)."""
import sys

import geopandas as gpd
from shapely.geometry import Point
from sqlalchemy import create_engine

engine = create_engine(sys.argv[1].replace("postgres://", "postgresql+psycopg://"))
mode = sys.argv[2]
if mode == "read":
    g = gpd.read_postgis("SELECT * FROM gf_xcheck_elixir ORDER BY id", engine, geom_col="geometry")
    assert g.crs.to_epsg() == 4326, g.crs
    assert [None if x is None else x.wkt for x in g.geometry] == [
        "POINT (1 2)", "POINT (3 4)", None, "POINT (5 6)"], list(g.geometry)
    assert list(g["id"]) == [1, 2, 3, 4]
    assert g["score"].isna().tolist() == [False, True, False, False]
    assert g["name"].isna().tolist() == [False, False, True, False]
    m = gpd.read_postgis("SELECT * FROM gf_xcheck_mixed ORDER BY id", engine, geom_col="geometry")
    assert m.crs.to_epsg() == 3857 and len(m) == 2, m.crs
    print("geopandas read of GeoFrame tables: ok")
else:
    g = gpd.GeoDataFrame(
        {"id": [1, 2, 3], "score": [1.5, None, 3.5], "name": ["x", None, "z"]},
        geometry=[Point(1, 2), Point(3, 4), None], crs="EPSG:4326")
    g.to_postgis("gf_xcheck_python", engine, if_exists="replace", index=False)
    print("geopandas wrote gf_xcheck_python")
