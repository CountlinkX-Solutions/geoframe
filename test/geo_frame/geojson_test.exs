defmodule GeoFrame.GeoJSONTest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  describe "GeoSeries.to_geojson/1" do
    test "returns one geometry map per row, nil for null and unrepresentable rows" do
      series =
        GeoSeries.from_wkt!([
          "POINT (1 2)",
          nil,
          "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))",
          "POINT EMPTY"
        ])

      assert [point, nil, polygon, nil] = GeoSeries.to_geojson(series)
      assert point == %{"type" => "Point", "coordinates" => [1.0, 2.0]}
      assert %{"type" => "Polygon", "coordinates" => [ring]} = polygon
      assert length(ring) == 5
    end

    test "an empty series gives an empty list" do
      assert GeoSeries.to_geojson(GeoSeries.from_wkt!([])) == []
    end
  end

  describe "GeoDataFrame.to_geojson/1" do
    setup do
      data = DataFrame.new(name: ["a", "b", "c"], population: [10, nil, 30])

      gdf =
        GeoDataFrame.new(
          data,
          GeoSeries.from_wkt!(["POINT (1 2)", "LINESTRING (0 0, 3 4)", nil])
        )

      {:ok, gdf: gdf}
    end

    test "is a FeatureCollection with attributes as properties", %{gdf: gdf} do
      assert %{"type" => "FeatureCollection", "features" => features} =
               GeoDataFrame.to_geojson(gdf)

      assert length(features) == 3

      assert Enum.at(features, 0) == %{
               "type" => "Feature",
               "geometry" => %{"type" => "Point", "coordinates" => [1.0, 2.0]},
               "properties" => %{"name" => "a", "population" => 10}
             }
    end

    test "keeps null attributes and null geometries", %{gdf: gdf} do
      %{"features" => [_, second, third]} = GeoDataFrame.to_geojson(gdf)
      assert second["properties"] == %{"name" => "b", "population" => nil}
      assert third["geometry"] == nil
      assert third["type"] == "Feature"
    end

    test "round-trips through a JSON encoder", %{gdf: gdf} do
      assert gdf |> GeoDataFrame.to_geojson() |> Jason.encode!() |> Jason.decode!() ==
               GeoDataFrame.to_geojson(gdf)
    end

    test "works on a frame without attribute columns" do
      gdf = GeoDataFrame.new(DataFrame.new([]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      assert %{"features" => [%{"properties" => %{}}]} = GeoDataFrame.to_geojson(gdf)
    end
  end
end
