defmodule GeoFrame.ToCrsTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @r 6_378_137.0

  defp s(rows, opts \\ [crs: "EPSG:4326"]), do: GeoSeries.from_wkt!(rows, opts)

  defp xy(%GeoSeries{} = series) do
    [wkt] = GeoSeries.to_wkt(series)
    [x, y] = ~r/\(([-\d.e+]+) ([-\d.e+]+)\)/ |> Regex.run(wkt, capture: :all_but_first)
    {to_number(x), to_number(y)}
  end

  defp to_number(text) do
    {value, ""} = Float.parse(text)
    value
  end

  defp web_mercator(lon, lat) do
    phi = lat * :math.pi() / 180
    {@r * lon * :math.pi() / 180, @r * :math.log(:math.tan(:math.pi() / 4 + phi / 2))}
  end

  describe "GeoSeries.to_crs/2" do
    test "EPSG:4326 to EPSG:3857 matches the Web Mercator closed form" do
      out = GeoSeries.to_crs(s(["POINT (-80.19 25.76)"]), "EPSG:3857")
      {x, y} = xy(out)
      {ex, ey} = web_mercator(-80.19, 25.76)
      assert_in_delta x, ex, 1.0e-3
      assert_in_delta y, ey, 1.0e-3
      assert GeoSeries.crs(out) == "EPSG:3857"
    end

    test "an integer target is normalised to an EPSG string" do
      out = GeoSeries.to_crs(s(["POINT (-81 0)"]), 32_617)
      assert GeoSeries.crs(out) == "EPSG:32617"
      {x, y} = xy(out)
      assert_in_delta x, 500_000.0, 1.0e-3
      assert_in_delta y, 0.0, 1.0e-3
    end

    test "OGC:CRS84 and lowercase targets are accepted and normalised" do
      assert "OGC:CRS84" = s(["POINT (1 2)"]) |> GeoSeries.to_crs("ogc:crs84") |> GeoSeries.crs()
      assert "EPSG:3857" = s(["POINT (1 2)"]) |> GeoSeries.to_crs("epsg:3857") |> GeoSeries.crs()
    end

    test "the same CRS returns an unchanged copy" do
      series = s(["POINT (1 2)"])
      same = GeoSeries.to_crs(series, "EPSG:4326")
      assert GeoSeries.to_wkt(same) == GeoSeries.to_wkt(series)
      assert GeoSeries.crs(same) == "EPSG:4326"
    end

    test "null rows stay null and a failing row becomes null" do
      out = GeoSeries.to_crs(s(["POINT (-80 25)", nil, "POINT (0 90)"]), "EPSG:3857")
      assert [wkt, nil, nil] = GeoSeries.to_wkt(out)
      assert is_binary(wkt)
    end

    test "polygon area in metres follows the degree-to-metre factor" do
      square = s(["POLYGON ((0 0, 0.01 0, 0.01 0.01, 0 0.01, 0 0))"])
      [area] = square |> GeoSeries.to_crs(3857) |> GeoSeries.area() |> Series.to_list()
      expected = 0.01 * 0.01 * :math.pow(@r * :math.pi() / 180, 2)
      assert_in_delta area / expected, 1.0, 0.01
    end

    test "a naive source raises" do
      assert_raise ArgumentError, ~r/naive geometries; set_crs first/, fn ->
        GeoSeries.to_crs(s(["POINT (1 1)"], []), "EPSG:3857")
      end
    end

    test "unknown and unsupported targets raise with a clear message" do
      assert_raise ArgumentError, ~r/unknown EPSG code 1\b/, fn ->
        GeoSeries.to_crs(s(["POINT (1 1)"]), "EPSG:1")
      end

      assert_raise ArgumentError, ~r/banana/, fn ->
        GeoSeries.to_crs(s(["POINT (1 1)"]), "banana")
      end
    end

    test "a PROJJSON source with an id is supported, one without is not" do
      with_id = ~s({"type":"GeographicCRS","name":"WGS 84","id":{"authority":"EPSG","code":4326}})
      out = GeoSeries.to_crs(s(["POINT (-80.19 25.76)"], crs: with_id), 3857)
      {x, _} = xy(out)
      assert_in_delta x, elem(web_mercator(-80.19, 25.76), 0), 1.0e-3

      assert_raise ArgumentError, ~r/PROJJSON/, fn ->
        GeoSeries.to_crs(s(["POINT (1 1)"], crs: ~s({"type":"GeographicCRS","name":"x"})), 3857)
      end
    end
  end

  describe "round trips" do
    property "4326 -> 3857 -> 4326 recovers lon/lat within 1e-9 degrees" do
      check all(
              lon <- float(min: -179.0, max: 179.0),
              lat <- float(min: -80.0, max: 80.0)
            ) do
        back =
          s(["POINT (#{lon} #{lat})"])
          |> GeoSeries.to_crs(3857)
          |> GeoSeries.to_crs(4326)

        {x, y} = xy(back)
        assert_in_delta x, lon, 1.0e-9
        assert_in_delta y, lat, 1.0e-9
      end
    end

    property "4326 -> 32617 -> 4326 recovers lon/lat inside UTM zone 17" do
      check all(
              lon <- float(min: -83.0, max: -79.0),
              lat <- float(min: -10.0, max: 60.0)
            ) do
        back =
          s(["POINT (#{lon} #{lat})"])
          |> GeoSeries.to_crs(32_617)
          |> GeoSeries.to_crs("EPSG:4326")

        {x, y} = xy(back)
        assert_in_delta x, lon, 1.0e-9
        assert_in_delta y, lat, 1.0e-9
      end
    end
  end

  describe "estimate_utm_crs/1" do
    test "Miami is UTM 17N and Sydney is UTM 56S" do
      assert GeoSeries.estimate_utm_crs(s(["POINT (-80.3 25.6)", "POINT (-80.1 25.9)"])) ==
               "EPSG:32617"

      assert GeoSeries.estimate_utm_crs(s(["POINT (151.2 -33.87)"])) == "EPSG:32756"
    end

    test "a naive series raises" do
      assert_raise ArgumentError, ~r/naive/, fn ->
        GeoSeries.estimate_utm_crs(s(["POINT (1 1)"], []))
      end
    end
  end

  describe "GeoDataFrame" do
    setup do
      gdf =
        GeoDataFrame.new(
          Explorer.DataFrame.new(name: ["a"]),
          s(["POLYGON ((-80.3 25.7, -80.2 25.7, -80.2 25.8, -80.3 25.8, -80.3 25.7))"])
        )

      %{gdf: gdf}
    end

    test "to_crs keeps attributes and reprojects the geometry", %{gdf: gdf} do
      out = GeoDataFrame.to_crs(gdf, 3857)
      assert GeoDataFrame.crs(out) == "EPSG:3857"
      assert out.data["name"] |> Series.to_list() == ["a"]
      assert GeoDataFrame.crs(gdf) == "EPSG:4326"
    end

    test "estimate_utm_crs then to_crs gives areas in square metres", %{gdf: gdf} do
      utm = GeoDataFrame.estimate_utm_crs(gdf)
      assert utm == "EPSG:32617"
      [area] = gdf |> GeoDataFrame.to_crs(utm) |> GeoDataFrame.area() |> Series.to_list()
      # 0.1 x 0.1 degrees near 25.75 N is about 11.1 km x 10.0 km.
      assert_in_delta area, 1.11e8, 0.02 * 1.11e8
    end
  end
end
