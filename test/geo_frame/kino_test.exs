defmodule GeoFrame.KinoTest do
  use ExUnit.Case, async: true

  import Kino.Test

  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @fixture Path.expand("../fixtures/neighborhoods.parquet", __DIR__)

  setup :configure_livebook_bridge

  setup_all do
    {:ok, gdf: GeoFrame.read_parquet!(@fixture)}
  end

  describe "Kino.Render" do
    test "a GeoDataFrame renders as map, table and inspect tabs", %{gdf: gdf} do
      assert %{type: :tabs, labels: ["Map", "Table", "Inspect"], outputs: outputs} =
               Kino.Render.to_livebook(gdf)

      assert length(outputs) == 3
    end

    test "a GeoSeries renders as map and inspect tabs", %{gdf: gdf} do
      assert %{type: :tabs, labels: ["Map", "Inspect"]} =
               gdf |> GeoDataFrame.geometry() |> Kino.Render.to_livebook()
    end

    test "above the feature limit the map is replaced by a notice", %{gdf: gdf} do
      previous = Application.get_env(:geoframe, :max_map_features)
      Application.put_env(:geoframe, :max_map_features, 2)
      on_exit(fn -> restore_env(previous) end)

      assert %{type: :tabs, labels: ["Map", "Table", "Inspect"], outputs: [map | _]} =
               Kino.Render.to_livebook(gdf)

      assert inspect(map) =~ "too many"
    end
  end

  describe "GeoFrame.Kino.map/2" do
    test "returns a Kino", %{gdf: gdf} do
      assert %Kino.JS.Live{} = GeoFrame.Kino.map(gdf)
    end

    test "accepts a GeoSeries", %{gdf: gdf} do
      assert %Kino.JS.Live{} = GeoFrame.Kino.map(GeoDataFrame.geometry(gdf))
    end

    test "returns a notice above the :max_features option", %{gdf: gdf} do
      assert %Kino.Markdown{} = GeoFrame.Kino.map(gdf, max_features: 2)
    end

    test "handles a series with no drawable geometry" do
      series = GeoSeries.from_wkt!([nil, "POINT EMPTY"])
      assert %Kino.Markdown{} = GeoFrame.Kino.map(series)
    end
  end

  describe "GeoFrame.Kino.maplibre/2" do
    test "adds a GeoJSON source and one layer per geometry kind", %{gdf: gdf} do
      spec = gdf |> GeoFrame.Kino.maplibre() |> MapLibre.to_spec()

      assert %{"type" => "geojson", "data" => %{"type" => "FeatureCollection"}} =
               spec["sources"]["geoframe"]

      ids = spec["layers"] |> Enum.map(& &1["id"])
      assert "geoframe-fill" in ids
      assert "geoframe-outline" in ids
      assert "geoframe-line" in ids
      assert "geoframe-circle" in ids
    end

    test "centers on the data and picks a city-scale zoom", %{gdf: gdf} do
      spec = gdf |> GeoFrame.Kino.maplibre() |> MapLibre.to_spec()
      [lng, lat] = spec["center"]
      assert_in_delta lng, -80.185, 0.02
      assert_in_delta lat, 25.775, 0.02
      assert spec["zoom"] > 10 and spec["zoom"] < 15
    end

    test "a single point gets a fixed street-level zoom" do
      series = GeoSeries.from_wkt!(["POINT (10 20)"])
      spec = series |> GeoFrame.Kino.maplibre() |> MapLibre.to_spec()
      assert spec["center"] == [10.0, 20.0]
      assert spec["zoom"] == 14
    end
  end

  describe "Z geometries" do
    setup do
      series = GeoSeries.from_wkt!(["POINT Z (10 20 5)", "LINESTRING Z (0 0 1, 1 1 2)", nil])

      %{
        series: series,
        gdf: GeoFrame.GeoDataFrame.new(Explorer.DataFrame.new(id: [1, 2, 3]), series)
      }
    end

    test "the map draws the 2D projection", %{series: series} do
      spec = series |> GeoFrame.Kino.maplibre() |> MapLibre.to_spec()
      features = spec["sources"]["geoframe"]["data"]["features"]

      assert Enum.map(features, & &1["geometry"]["coordinates"]) ==
               [[10.0, 20.0], [[0.0, 0.0], [1.0, 1.0]]]
    end

    test "inspect and the table preview keep the Z ordinate", %{series: series, gdf: gdf} do
      assert inspect(series) =~ "POINT Z(10 20 5)"
      assert inspect(gdf) =~ "LINESTRING Z(0 0 1,1 1 2)"

      assert gdf
             |> GeoFrame.GeoDataFrame.preview()
             |> Explorer.DataFrame.to_columns()
             |> Map.fetch!("geometry") ==
               ["POINT Z(10 20 5)", "LINESTRING Z(0 0 1,1 1 2)", nil]

      assert gdf
             |> GeoFrame.GeoDataFrame.to_explorer(encoding: :wkt)
             |> Explorer.DataFrame.to_columns()
             |> Map.fetch!("geometry") ==
               ["POINT Z(10 20 5)", "LINESTRING Z(0 0 1,1 1 2)", nil]
    end
  end

  defp restore_env(nil), do: Application.delete_env(:geoframe, :max_map_features)
  defp restore_env(value), do: Application.put_env(:geoframe, :max_map_features, value)
end
