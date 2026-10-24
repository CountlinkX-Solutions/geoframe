"""GeoPandas side of the GeoFrame comparison (see notebooks/compare.livemd).

Prints one JSON object {analysis_key: {metric: value}}. The functions below are the very
same code as the python blocks of the notebook; scripts/compare_geopandas.sh checks that
they have not drifted apart.

    ~/.venvs/geopandas-ref/bin/python -I examples/geopandas/compare.py
"""

# --- setup
import json
import os
import tempfile
import urllib.request

import geopandas as gpd
import shapely
from pyproj import Geod

BASE = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson"
EQUAL_AREA = "EPSG:6933"
# South America Albers equal-area conic (ESRI:102033) as WKT, since "ESRI:" codes are not in EPSG.
SOUTH_AMERICA_ALBERS = (
    'PROJCS["South_America_Albers",GEOGCS["WGS 84",DATUM["WGS_1984",'
    'SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],'
    'UNIT["Degree",0.0174532925199433]],PROJECTION["Albers_Conic_Equal_Area"],'
    'PARAMETER["latitude_of_center",-32],PARAMETER["longitude_of_center",-60],'
    'PARAMETER["standard_parallel_1",-5],PARAMETER["standard_parallel_2",-42],'
    'PARAMETER["false_easting",0],PARAMETER["false_northing",0],UNIT["metre",1]]'
)


def fetch(name):
    path = os.path.join(tempfile.gettempdir(), name)
    if not os.path.exists(path):
        urllib.request.urlretrieve(f"{BASE}/{name}", path)
    return path


countries = gpd.read_file(fetch("ne_50m_admin_0_countries.geojson"))
cities = gpd.read_file(fetch("ne_10m_populated_places_simple.geojson"))
rivers = gpd.read_file(fetch("ne_50m_rivers_lake_centerlines.geojson"))
countries_eq = countries.to_crs(EQUAL_AREA)
big_cities = cities[cities["pop_max"] > 1_000_000]


# --- analysis: buffer_dissolve
def buffer_dissolve():
    buffers = big_cities.to_crs(EQUAL_AREA)[["adm0name", "geometry"]]
    buffers = buffers.set_geometry(buffers.buffer(100_000))
    covered = buffers.dissolve(by="adm0name").reset_index()
    covered["area_km2"] = covered.area / 1e6
    top = covered.sort_values(["area_km2", "adm0name"], ascending=[False, True]).head(10)
    return {
        "n_countries": len(covered),
        "total_area_km2": covered["area_km2"].sum(),
        "top10_countries": top["adm0name"].tolist(),
        "top10_area_km2": top["area_km2"].tolist(),
    }


# --- analysis: river_corridor
def river_corridor():
    major = rivers[rivers["scalerank"] <= 4].to_crs(EQUAL_AREA)
    corridor = gpd.GeoDataFrame(geometry=[major.buffer(50_000).union_all()], crs=EQUAL_AREA)
    pieces = gpd.overlay(
        countries_eq[["ADM0_A3", "geometry"]], corridor, how="intersection", keep_geom_type=True
    )
    pieces["inside_km2"] = pieces.area / 1e6
    pieces = pieces.merge(
        countries_eq.assign(area_km2=countries_eq.area / 1e6)[["ADM0_A3", "area_km2"]],
        on="ADM0_A3",
    )
    pieces["share_pct"] = 100 * pieces["inside_km2"] / pieces["area_km2"]
    top = pieces.sort_values(["inside_km2", "ADM0_A3"], ascending=[False, True]).head(5)
    return {
        "n_countries": len(pieces),
        "total_inside_km2": pieces["inside_km2"].sum(),
        "top5_countries": top["ADM0_A3"].tolist(),
        "top5_inside_km2": top["inside_km2"].tolist(),
        "top5_share_pct": top["share_pct"].tolist(),
    }


# --- analysis: cities_per_country
def cities_per_country():
    joined = gpd.sjoin(
        cities[["name", "pop_max", "geometry"]],
        countries[["ADM0_A3", "geometry"]],
        how="inner",
        predicate="within",
    )
    stats = (
        joined.groupby("ADM0_A3")
        .agg(n_cities=("name", "size"), pop=("pop_max", "sum"), max_pop=("pop_max", "max"))
        .reset_index()
    )
    area = (countries_eq.area / 1e6).rename("area_km2")
    stats = stats.merge(
        countries_eq[["ADM0_A3"]].join(area), on="ADM0_A3"
    )
    stats["density_per_km2"] = stats["pop"] / stats["area_km2"]
    by_count = stats.sort_values(["n_cities", "ADM0_A3"], ascending=[False, True]).head(10)
    by_density = stats.sort_values(["density_per_km2", "ADM0_A3"], ascending=[False, True]).head(5)
    return {
        "n_joined": len(joined),
        "n_countries": len(stats),
        "total_pop": int(stats["pop"].sum()),
        "top10_by_cities": by_count["ADM0_A3"].tolist(),
        "top10_city_counts": by_count["n_cities"].tolist(),
        "top10_max_pop": by_count["max_pop"].tolist(),
        "top5_by_density": by_density["ADM0_A3"].tolist(),
        "top5_density_per_km2": by_density["density_per_km2"].tolist(),
    }


# --- analysis: nearest_river
def nearest_river():
    cities_eq = big_cities.to_crs(EQUAL_AREA)[["name", "adm0name", "geometry"]]
    streams = rivers[["name", "geometry"]].rename(columns={"name": "river"}).to_crs(EQUAL_AREA)
    near = gpd.sjoin_nearest(cities_eq, streams, how="left", distance_col="dist_m")
    near["km"] = near["dist_m"] / 1000
    # A city equally close to two rivers matches both: keep the first, as the distance is equal.
    near = near[~near.index.duplicated()].copy()
    near["city"] = near["name"] + ", " + near["adm0name"]
    far = near.sort_values(["km", "city"], ascending=[False, True]).head(5)
    return {
        "n_cities": len(near),
        "median_km": near["km"].median(),
        "mean_km": near["km"].mean(),
        "max_km": near["km"].max(),
        "farthest5_cities": far["city"].tolist(),
        "farthest5_km": far["km"].tolist(),
    }


# --- analysis: river_length_south_america
def river_length_south_america():
    south_america = countries[countries["CONTINENT"] == "South America"]
    south_america = south_america[["ADM0_A3", "geometry"]].to_crs(SOUTH_AMERICA_ALBERS)
    streams = rivers.cx[-95:-30, -60:15][["geometry"]].to_crs(SOUTH_AMERICA_ALBERS)
    pieces = gpd.overlay(streams, south_america, how="intersection", keep_geom_type=True)
    pieces["km"] = pieces.length / 1000
    per_country = pieces.groupby("ADM0_A3")["km"].sum().reset_index()
    top = per_country.sort_values(["km", "ADM0_A3"], ascending=[False, True]).head(5)
    return {
        "n_countries": len(per_country),
        "total_km": per_country["km"].sum(),
        "top5_countries": top["ADM0_A3"].tolist(),
        "top5_km": top["km"].tolist(),
    }


# --- analysis: continents
def continents():
    merged = countries_eq[["CONTINENT", "geometry"]].dissolve(by="CONTINENT").sort_index()
    centre = merged.centroid
    # Simplification is measured on the countries, so that it does not depend on the union.
    parts = countries_eq[["CONTINENT"]].assign(
        vertices=countries_eq.geometry.apply(shapely.get_num_coordinates),
        simplified=countries_eq.simplify(50_000, preserve_topology=False).apply(
            shapely.get_num_coordinates
        ),
    )
    totals = parts.groupby("CONTINENT")[["vertices", "simplified"]].sum().sort_index()
    return {
        "continents": merged.index.tolist(),
        "area_km2": (merged.area / 1e6).tolist(),
        "centroid_x_km": (centre.x / 1000).tolist(),
        "centroid_y_km": (centre.y / 1000).tolist(),
        "hull_area_km2": (merged.convex_hull.area / 1e6).tolist(),
        "dissolved_vertices": merged.geometry.apply(shapely.get_num_coordinates).tolist(),
        "country_vertices": totals["vertices"].tolist(),
        "country_vertices_simplified_50km": totals["simplified"].tolist(),
    }


# --- analysis: capitals_geodesic
def capitals_geodesic():
    wanted = ["Ottawa", "London", "Cairo", "Tokyo", "Brasília", "Canberra", "Moscow", "Nairobi"]
    capitals = cities[(cities["adm0cap"] == 1) & cities["name"].isin(wanted)]
    capitals = capitals.sort_values("name").reset_index(drop=True)
    geod = Geod(ellps="WGS84")
    km, pairs = [], []
    for i in range(len(capitals)):
        for j in range(i + 1, len(capitals)):
            a, b = capitals.geometry[i], capitals.geometry[j]
            km.append(geod.inv(a.x, a.y, b.x, b.y)[2] / 1000)
            pairs.append(f"{capitals['name'][i]} - {capitals['name'][j]}")
    longest = max(range(len(km)), key=lambda k: km[k])
    return {
        "capitals": capitals["name"].tolist(),
        "pair_km": km,
        "total_km": sum(km),
        "longest_pair": pairs[longest],
    }


# --- analysis: neighbours
def neighbours():
    def touching(code):
        target = countries.loc[countries["ADM0_A3"] == code, "geometry"].iloc[0]
        hits = countries[countries.touches(target)]
        return sorted(hits["NAME"])

    equator = shapely.LineString([(-180, 0), (180, 0)])
    crossed = countries[countries.intersects(equator)]
    return {
        "germany_neighbours": touching("DEU"),
        "brazil_neighbours": touching("BRA"),
        "equator_countries": sorted(crossed["NAME"]),
    }


# --- analysis: predicates
def predicates():
    points = cities[["name", "geometry"]]
    polygons = countries[["ADM0_A3", "geometry"]]
    rows = {
        p: len(gpd.sjoin(points, polygons, how="inner", predicate=p))
        for p in ["intersects", "within", "touches"]
    }
    unmatched = gpd.sjoin(points, polygons, how="left", predicate="within")["ADM0_A3"].isna()
    inside = countries.representative_point().within(countries.geometry)
    return {
        "intersects_rows": rows["intersects"],
        "within_rows": rows["within"],
        "touches_rows": rows["touches"],
        "cities_in_no_country": int(unmatched.sum()),
        "invalid_countries": int((~countries.is_valid).sum()),
        "representative_points_inside": int(inside.sum()),
    }


# --- analysis: voronoi
def voronoi():
    south_america = countries_eq[countries_eq["CONTINENT"] == "South America"]
    land = south_america.union_all()
    capitals = cities[(cities["adm0cap"] == 1) & (cities["adm0_a3"].isin(south_america["ADM0_A3"]))]
    capitals = capitals.to_crs(EQUAL_AREA).sort_values("name").reset_index(drop=True)
    diagram = capitals.geometry.voronoi_polygons(extend_to=shapely.box(*land.bounds))
    cells = gpd.GeoDataFrame(geometry=diagram.reset_index(drop=True), crs=EQUAL_AREA)
    owned = gpd.sjoin(cells, capitals[["name", "geometry"]], predicate="contains")
    owned["area_km2"] = owned.geometry.intersection(land).area / 1e6
    owned = owned.sort_values("name")
    return {
        "capitals": owned["name"].tolist(),
        "cell_area_km2": owned["area_km2"].tolist(),
        "total_area_km2": owned["area_km2"].sum(),
    }


# --- main
if __name__ == "__main__":
    analyses = [
        buffer_dissolve,
        river_corridor,
        cities_per_country,
        nearest_river,
        river_length_south_america,
        continents,
        capitals_geodesic,
        neighbours,
        predicates,
        voronoi,
    ]
    print(json.dumps({f.__name__: f() for f in analyses}, indent=1))
