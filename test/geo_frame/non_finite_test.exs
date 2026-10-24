defmodule GeoFrame.NonFiniteTest do
  use ExUnit.Case, async: true

  # enif_make_double cannot encode NaN or infinity, so a measure that overflows used to raise
  # ArgumentError at the NIF boundary. The values now cross as Explorer's float atoms, the
  # same ones `Explorer.Series.to_list/1` returns for a f64 series holding them.

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @huge 1.7e308

  defp series(wkts), do: GeoSeries.from_wkt!(wkts)

  test "area overflowing to infinity is :infinity, as GeoPandas gives inf" do
    s = series(["POLYGON ((0 0, 1e200 0, 1e200 1e200, 0 1e200, 0 0))", "POINT (1 1)", nil])

    assert s |> GeoSeries.area() |> Series.to_list() == [:infinity, 0.0, nil]
  end

  test "length overflowing to infinity is :infinity" do
    s = series(["LINESTRING (0 0, #{@huge} #{@huge})", "LINESTRING (0 0, 3 4)"])

    assert s |> GeoSeries.length() |> Series.to_list() == [:infinity, 5.0]
  end

  test "geodesic measures never raise" do
    s = series(["POLYGON ((0 0, 1 0, 1 1, 0 0))"]) |> GeoSeries.set_crs("EPSG:4326")

    assert [a] = s |> GeoSeries.area_geodesic() |> Series.to_list()
    assert is_float(a)
  end

  test "hausdorff and frechet distance overflowing to infinity are :infinity" do
    a = series(["LINESTRING (0 0, 1 0)"])
    b = series(["LINESTRING (#{@huge} #{@huge}, #{@huge} 0)"])

    assert a |> GeoSeries.hausdorff_distance(b) |> Series.to_list() == [:infinity]
    assert a |> GeoSeries.frechet_distance(b) |> Series.to_list() == [:infinity]
  end

  test "planar distance overflowing to infinity is :infinity" do
    a = series(["POINT (0 0)"])
    b = series(["POINT (#{@huge} #{@huge})"])

    assert a |> GeoSeries.distance(b) |> Series.to_list() == [:infinity]
  end

  test "NaN, from infinity minus infinity, crosses as :nan" do
    s = series(["POLYGON ((0 0, 1e999 0, 1e999 1e999, 0 1e999, 0 0))"])

    assert s |> GeoSeries.area() |> Series.to_list() == [:nan]
    assert s |> GeoSeries.length() |> Series.to_list() == [:infinity]
  end

  test "infinite coordinates survive x, y, bounds and total_bounds" do
    s = series(["POINT (1e999 -1e308)", "POINT (1e999 2)", "POINT (1 2)"])

    assert s |> GeoSeries.x() |> Series.to_list() == [:infinity, :infinity, 1.0]
    assert s |> GeoSeries.y() |> Series.to_list() == [-1.0e308, 2.0, 2.0]

    bounds = GeoSeries.bounds(series(["POINT (1e999 -1e308)"]))
    assert Series.to_list(bounds["minx"]) == [:infinity]
    assert Series.to_list(bounds["maxy"]) == [-1.0e308]

    assert {:infinity, -1.0e308, :infinity, -1.0e308} =
             GeoSeries.total_bounds(series(["POINT (1e999 -1e308)"]))
  end
end
