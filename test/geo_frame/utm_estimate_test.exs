defmodule GeoFrame.UtmEstimateTest do
  # The case grid lives in native/geoframe_core/src/reproject_tests.rs (7 000 cases against
  # GeoPandas); this file covers the Elixir surface.
  use ExUnit.Case, async: true

  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp s(wkt, crs \\ "EPSG:4326"), do: GeoSeries.from_wkt!(wkt, crs: crs)

  test "Norway and Svalbard follow pyproj: plain 6-degree zones" do
    assert GeoSeries.estimate_utm_crs(s(["POINT (5 60)"])) == "EPSG:32631"
    assert GeoSeries.estimate_utm_crs(s(["POINT (10 60)"])) == "EPSG:32632"
    assert GeoSeries.estimate_utm_crs(s(["POINT (35 78)"])) == "EPSG:32636"
  end

  test "datum_name picks another UTM family" do
    miami = s(["POINT (-80.2 25.7)"])
    assert GeoSeries.estimate_utm_crs(miami, datum_name: "WGS 84") == "EPSG:32617"
    assert GeoSeries.estimate_utm_crs(miami, datum_name: "NAD83") == "EPSG:26917"
    assert GeoSeries.estimate_utm_crs(miami, datum_name: "NAD27") == "EPSG:26717"
  end

  test "a datum without a UTM CRS over the centre raises, as GeoPandas does" do
    assert_raise ArgumentError, ~r/Unable to determine UTM CRS.*ETRS89/, fn ->
      GeoSeries.estimate_utm_crs(s(["POINT (-80 25)"]), datum_name: "ETRS89")
    end

    assert_raise ArgumentError, ~r/Unable to determine UTM CRS/, fn ->
      GeoSeries.estimate_utm_crs(s(["POINT (0 85)"]))
    end
  end

  test "unknown options raise" do
    assert_raise ArgumentError, ~r/unknown option\(s\) \[:datum\]/, fn ->
      GeoSeries.estimate_utm_crs(s(["POINT (0 0)"]), datum: "NAD83")
    end
  end

  test "projected sources use the centre of the converted bounds" do
    wide = s(["POINT (-20000000 -1000000)", "POINT (20000000 1000000)"], "EPSG:3857")
    assert GeoSeries.estimate_utm_crs(wide) == "EPSG:32630"

    pdc = s(["POINT (3000000 0)", "POINT (3700000 1000000)"], "EPSG:3832")
    assert GeoSeries.estimate_utm_crs(pdc) == "EPSG:32601"
  end

  test "GeoDataFrame forwards the options" do
    gdf = GeoDataFrame.new(Explorer.DataFrame.new(id: [1]), s(["POINT (-80.2 25.7)"]))
    assert GeoDataFrame.estimate_utm_crs(gdf) == "EPSG:32617"
    assert GeoDataFrame.estimate_utm_crs(gdf, datum_name: "NAD83") == "EPSG:26917"
  end
end
