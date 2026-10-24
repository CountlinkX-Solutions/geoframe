defmodule GeoFrame.UnknownOptionsTest do
  use ExUnit.Case, async: true

  # A typo such as `predicat:` used to be ignored, silently running the default join.
  # Every option-taking function checks its keys against the ones it supports.

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp series, do: GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)"])

  defp frame do
    GeoDataFrame.new(DataFrame.new(id: [1, 2]), series())
  end

  defp assert_unknown(fun, key, supported) do
    error = assert_raise ArgumentError, fun
    assert error.message =~ "unknown option(s) [#{inspect(key)}]"
    assert error.message =~ "supported options: #{inspect(supported)}"
  end

  test "sjoin rejects a misspelt predicate option" do
    assert_unknown(
      fn -> GeoFrame.sjoin(frame(), frame(), predicat: :within) end,
      :predicat,
      [:how, :predicate, :distance, :lsuffix, :rsuffix]
    )
  end

  test "sjoin_nearest rejects unknown options, also the predicate-join ones" do
    supported = [:how, :max_distance, :distance_col, :exclusive, :lsuffix, :rsuffix]

    assert_unknown(
      fn -> GeoFrame.sjoin_nearest(frame(), frame(), max_dist: 1) end,
      :max_dist,
      supported
    )

    assert_unknown(
      fn -> GeoFrame.sjoin_nearest(frame(), frame(), predicate: :within) end,
      :predicate,
      supported
    )
  end

  test "sindex_query and sindex_nearest reject unknown options" do
    assert_unknown(
      fn -> GeoSeries.sindex_query(series(), series(), :dwithin, distnace: 1) end,
      :distnace,
      [:distance]
    )

    assert_unknown(
      fn -> GeoSeries.sindex_nearest(series(), series(), exclusiv: true) end,
      :exclusiv,
      [:max_distance, :exclusive]
    )
  end

  test "dissolve, to_explorer, new and the WKB/WKT constructors reject unknown options" do
    assert_unknown(
      fn -> GeoFrame.dissolve(frame(), by: "id", sorted: false) end,
      :sorted,
      [:by, :aggfunc, :dropna, :sort]
    )

    assert_unknown(
      fn -> GeoDataFrame.to_explorer(frame(), encodng: :wkt) end,
      :encodng,
      [:encoding]
    )

    assert_unknown(
      fn -> GeoDataFrame.new(DataFrame.new(id: [1, 2]), series(), geometry_colum: "g") end,
      :geometry_colum,
      [:geometry_column]
    )

    df = DataFrame.new(geom: ["POINT (1 1)"])

    assert_unknown(
      fn -> GeoDataFrame.from_wkt!(df, "geom", crss: 4326) end,
      :crss,
      [:crs, :geometry_column]
    )
  end

  test "measures and affine transforms reject unknown options" do
    assert_unknown(fn -> GeoSeries.area(series(), metod: :geodesic) end, :metod, [:method])

    assert_unknown(fn -> GeoSeries.distance(series(), series(), mthod: :geodesic) end, :mthod, [
      :method
    ])

    assert_unknown(fn -> GeoSeries.translate(series(), 1, 1, zof: 1) end, :zof, [:zoff])
  end

  test "supported options still work" do
    assert %GeoDataFrame{} = GeoFrame.sjoin(frame(), frame(), predicate: :intersects, how: :left)
    assert {[0, 1], [0, 1]} = GeoSeries.sindex_query(series(), series())
    assert %GeoDataFrame{} = GeoFrame.sjoin_nearest(frame(), frame(), max_distance: 1.0)
  end
end
