defmodule GeoFrame.ToCrsParityTest do
  # End-to-end check of the issue #97 fixes against pyproj values (fixture crs_parity.json).
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  @parity Path.expand("../reference/fixtures/crs_datum/crs_parity.json", __DIR__)
          |> File.read!()
          |> Jason.decode!()

  defp point(x, y, crs), do: GeoSeries.from_wkt!(["POINT (#{x} #{y})"], crs: crs)

  defp xy(series) do
    [wkt] = GeoSeries.to_wkt(series)
    [x, y] = Regex.run(~r/\(([^ ]+) ([^)]+)\)/, wkt, capture: :all_but_first)
    {elem(Float.parse(x), 0), elem(Float.parse(y), 0)}
  end

  # Metres between two lon/lat points (spherical).
  defp metres({lon1, lat1}, {lon2, lat2}) do
    rad = :math.pi() / 180

    a =
      :math.pow(:math.sin((lat2 - lat1) * rad / 2), 2) +
        :math.cos(lat1 * rad) * :math.cos(lat2 * rad) *
          :math.pow(:math.sin((lon2 - lon1) * rad / 2), 2)

    2 * 6_371_008.8 * :math.asin(:math.sqrt(a))
  end

  for code <- [
        "5513",
        "2065",
        "4807",
        "4901",
        "4087",
        "2057",
        "3078",
        "2066",
        "5472",
        "8857",
        "27200",
        "4688"
      ] do
    test "to_crs from EPSG:#{code} matches pyproj" do
      entry = @parity["crs"][unquote(code)]

      for [_lon, _lat, x, y, ex, ey] <- entry["pts"] do
        got = point(x, y, "EPSG:#{unquote(code)}") |> GeoSeries.to_crs("EPSG:4326") |> xy()
        assert metres(got, {ex, ey}) < 0.05
      end
    end
  end

  test "grad-unit geographic CRS takes and gives grads" do
    # 2 E 48 N is -0.3739 grad of longitude from Paris and 53.3334 grad of latitude.
    {x, y} = point(2.0, 48.0, "EPSG:4326") |> GeoSeries.to_crs("EPSG:4807") |> xy()
    assert_in_delta x, -0.3738935959, 1.0e-6
    assert_in_delta y, 53.3334059895, 1.0e-6
  end

  test "Krovak southing/westing has positive coordinates in the Czech Republic" do
    {x, y} = point(15.0, 50.0, "EPSG:4326") |> GeoSeries.to_crs("EPSG:5513") |> xy()
    assert x > 1_000_000 and y > 600_000
  end
end
