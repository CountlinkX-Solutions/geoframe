# Dev-only: PostGIS interop with GeoPandas. Usage:
#   mix run scripts/crosscheck_postgis.exs write   (GeoFrame writes gf_xcheck_elixir)
#   mix run scripts/crosscheck_postgis.exs read    (GeoFrame reads gf_xcheck_python, written by GeoPandas)
alias Explorer.DataFrame
alias GeoFrame.{GeoDataFrame, GeoSeries}

uri = URI.parse(System.fetch_env!("GEOFRAME_POSTGIS_URL"))
[user, pass] = String.split(uri.userinfo, ":", parts: 2)

{:ok, conn} =
  Postgrex.start_link(
    hostname: uri.host,
    port: uri.port,
    username: user,
    password: pass,
    database: String.trim_leading(uri.path, "/"),
    types: GeoFrame.Postgrex.Types
  )

case System.argv() do
  ["write"] ->
    geometry =
      GeoSeries.from_wkt!(
        ["POINT (1 2)", "POINT (3 4)", nil, "POINT (5 6)"],
        crs: "EPSG:4326"
      )

    data =
      DataFrame.new(
        id: [1, 2, 3, 4],
        score: [1.5, nil, 3.5, 4.5],
        name: ["a", "b", nil, "d"],
        flag: [true, false, nil, true]
      )

    gdf = GeoDataFrame.new(data, geometry, geometry_column: "geometry")
    :ok = GeoFrame.to_postgis(gdf, conn, "gf_xcheck_elixir", if_exists: :replace)

    polys =
      GeoSeries.from_wkt!(
        ["POLYGON ((0 0, 1 0, 1 1, 0 0))", "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 0)))"],
        crs: "EPSG:3857"
      )

    :ok =
      GeoFrame.to_postgis(
        GeoDataFrame.new(DataFrame.new(id: [1, 2]), polys),
        conn,
        "gf_xcheck_mixed", if_exists: :replace)

  ["read"] ->
    gdf =
      GeoFrame.read_postgis!(conn, "SELECT * FROM gf_xcheck_python ORDER BY id",
        geom_col: "geometry"
      )

    IO.inspect(GeoDataFrame.crs(gdf), label: "crs")
    IO.inspect(GeoSeries.to_wkt(gdf.geometry), label: "wkt")
    IO.inspect(DataFrame.to_columns(gdf.data), label: "data")
end
