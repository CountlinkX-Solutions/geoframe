defmodule GeoFrame.CrsEsriDatumTest do
  # to_crs with a CRS given as WKT1_ESRI text (a Shapefile .prj: GEOGCS "GCS_...", DATUM "D_...",
  # no identifier). PROJ identifies the datum by its name and shifts it to WGS 84 like the EPSG
  # code does. Values are pyproj 3.8 / PROJ 9.8 results (fixture crs_esri_datum.json).
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  @fixture Path.expand("../reference/fixtures/crs_datum/crs_esri_datum.json", __DIR__)
           |> File.read!()
           |> Jason.decode!()

  # 1e-7 of a degree is about a centimetre
  @tolerance 1.0e-7

  defp xy(series) do
    [wkt] = GeoSeries.to_wkt(series)
    [x, y] = Regex.run(~r/\(([^ ]+) ([^)]+)\)/, wkt, capture: :all_but_first)
    {elem(Float.parse(x), 0), elem(Float.parse(y), 0)}
  end

  for code <- [
        "4277",
        "27700",
        "4230",
        "23030",
        "4301",
        "4618",
        "26717",
        "32038",
        "24200",
        "3102"
      ] do
    test "EPSG:#{code} as WKT1_ESRI text shifts its datum like pyproj" do
      entry = get_in(@fixture, ["crs", unquote(code)])
      wkt = entry["wkt"]

      for [x, y, ex, ey] <- entry["pts"] do
        {gx, gy} =
          GeoSeries.from_wkt!(["POINT (#{x} #{y})"], crs: wkt)
          |> GeoSeries.to_crs("EPSG:4326")
          |> xy()

        assert abs(gx - ex) <= @tolerance and abs(gy - ey) <= @tolerance,
               "#{unquote(code)}: got #{gx} #{gy}, pyproj #{ex} #{ey}"
      end
    end
  end

  test "a WGS 84 compatible datum named in ESRI text is not shifted" do
    entry = get_in(@fixture, ["crs", "4269"])
    [x, y, ex, ey] = hd(entry["pts"])

    {gx, gy} =
      GeoSeries.from_wkt!(["POINT (#{x} #{y})"], crs: entry["wkt"])
      |> GeoSeries.to_crs("EPSG:4326")
      |> xy()

    assert_in_delta gx, ex, @tolerance
    assert_in_delta gy, ey, @tolerance
  end

  @prime_meridian Path.expand(
                    "../reference/fixtures/crs_datum/crs_esri_prime_meridian.json",
                    __DIR__
                  )
                  |> File.read!()
                  |> Jason.decode!()

  for code <- ["4805", "4806", "4807", "4901"] do
    test "EPSG:#{code} as ESRI text with another prime meridian keeps its previous result" do
      e = get_in(@prime_meridian, ["crs", unquote(code)])
      [bx, by] = e["before"]

      {gx, gy} =
        GeoSeries.from_wkt!(["POINT (#{e["x"]} #{e["y"]})"], crs: e["wkt"])
        |> GeoSeries.to_crs("EPSG:4326")
        |> xy()

      assert_in_delta gx, bx, 1.0e-9
      assert_in_delta gy, by, 1.0e-9
    end
  end
end
