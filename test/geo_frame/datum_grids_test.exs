defmodule GeoFrame.DatumGridsTest do
  # Grid directories and registrations are process-wide: this module runs alone.
  use ExUnit.Case, async: false

  alias GeoFrame.GeoSeries
  alias GeoFrame.Grids

  @fixtures Path.expand("../reference/fixtures/crs_datum", __DIR__)
  @cases Path.join([@fixtures, "grids", "cases.json"]) |> File.read!() |> Jason.decode!()
  @shifts Path.join(@fixtures, "datum_shifts.json") |> File.read!() |> Jason.decode!()

  setup do
    before = Grids.dirs()

    on_exit(fn ->
      Grids.unregister("EPSG:4301")
      Grids.set_dirs(before)
    end)
  end

  defp point(x, y, crs), do: GeoSeries.from_wkt!(["POINT (#{x} #{y})"], crs: crs)

  defp xy(series) do
    [wkt] = GeoSeries.to_wkt(series)
    [x, y] = Regex.run(~r/\(([^ ]+) ([^)]+)\)/, wkt, capture: :all_but_first)
    {elem(Float.parse(x), 0), elem(Float.parse(y), 0)}
  end

  # Distance in metres, spherical (accurate to 0.5 %).
  defp metres({lon1, lat1}, {lon2, lat2}) do
    rad = :math.pi() / 180
    dlat = (lat2 - lat1) * rad
    dlon = (lon2 - lon1) * rad

    a =
      :math.pow(:math.sin(dlat / 2), 2) +
        :math.cos(lat1 * rad) * :math.cos(lat2 * rad) * :math.pow(:math.sin(dlon / 2), 2)

    2 * 6_371_008.8 * :math.asin(:math.sqrt(a))
  end

  test "NAD27 and NZGD49 convert without grid files (they used to raise)" do
    {lon, lat} = point(-100, 40, "EPSG:4267") |> GeoSeries.to_crs("EPSG:4326") |> xy()
    assert metres({lon, lat}, {-100, 40}) > 10
    assert %GeoSeries{} = point(172, -42, "EPSG:4272") |> GeoSeries.to_crs("EPSG:4326")
    assert %GeoSeries{} = point(500_000, 4_400_000, "EPSG:26714") |> GeoSeries.to_crs("EPSG:4326")
  end

  test "every datum of the fixture stays within the documented ballpark of pyproj" do
    # pyproj without grids; the tolerance table is in native/geoframe_core/src/datum_tests.rs
    for {code, %{"pts" => pts}} <- @shifts["crs"], [x, y, lon, lat] <- pts do
      moved = point(x, y, "EPSG:#{code}") |> GeoSeries.to_crs("EPSG:4326") |> xy()
      assert metres(moved, {lon, lat}) < 60, "EPSG:#{code} at #{x} #{y}"
    end
  end

  test "grid directories: status, NTv2 and GeoTIFF grids, fallback outside them" do
    Grids.register("EPSG:4301", ["synthetic_tokyo.gsb"])
    assert Grids.status("EPSG:4301") == [%{name: "synthetic_tokyo.gsb", path: nil}]
    helmert = point(138, 35, "EPSG:4301") |> GeoSeries.to_crs("EPSG:4326") |> xy()

    Grids.set_dirs([Path.join(@fixtures, "grids")])
    assert Grids.dirs() == [Path.join(@fixtures, "grids")]
    [%{path: path}] = Grids.status("EPSG:4301")
    assert path == Path.join([@fixtures, "grids", "synthetic_tokyo.gsb"])
    gridded = point(138, 35, "EPSG:4301") |> GeoSeries.to_crs("EPSG:4326") |> xy()
    assert metres(helmert, gridded) > 100

    for {format, name} <- [{"gsb", "synthetic_tokyo.gsb"}, {"tif", "synthetic_tokyo.tif"}] do
      Grids.register("EPSG:4301", [name])

      for %{"x" => x, "y" => y} = c <- @cases["inside"] do
        [lon, lat] = c[format]
        moved = point(x, y, "EPSG:4301") |> GeoSeries.to_crs("EPSG:4326") |> xy()
        assert metres(moved, {lon, lat}) < 0.001, "#{format} #{x} #{y}"
      end

      for %{"x" => x, "y" => y, "fallback" => [lon, lat]} <- @cases["outside"] do
        moved = point(x, y, "EPSG:4301") |> GeoSeries.to_crs("EPSG:4326") |> xy()
        assert metres(moved, {lon, lat}) < 0.001, "#{format} outside #{x} #{y}"
      end
    end
  end

  test "register takes a projected CRS on the datum and unregister restores the table" do
    assert :ok = Grids.register("EPSG:27700", [])
    assert Grids.status("EPSG:4277") == []
    assert :ok = Grids.unregister(27_700)
    names = Enum.map(Grids.status(4277), & &1.name)
    assert "uk_os_OSTN15_NTv2_OSGBtoETRS.tif" in names
  end

  test "status is empty for CRSs without a datum grid and raises for unknown ones" do
    assert Grids.status("EPSG:4326") == []
    assert Grids.status("EPSG:3857") == []
    assert_raise ArgumentError, ~r/unknown EPSG code/, fn -> Grids.status("EPSG:999999") end
  end
end
