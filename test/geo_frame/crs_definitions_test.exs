defmodule GeoFrame.CrsDefinitionsTest do
  # to_crs / set_crs with a CRS given as a definition or as a code outside the bundled table
  # (issue #100). Values are pyproj 3.8 / PROJ 9.8 results (fixture crs_convert.json).
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  @fixture Path.expand("../reference/fixtures/crs_datum/crs_convert.json", __DIR__)
           |> File.read!()
           |> Jason.decode!()

  defp point(x, y, crs), do: GeoSeries.from_wkt!(["POINT (#{x} #{y})"], crs: crs)

  defp xy(series) do
    [wkt] = GeoSeries.to_wkt(series)
    [x, y] = Regex.run(~r/\(([^ ]+) ([^)]+)\)/, wkt, capture: :all_but_first)
    {elem(Float.parse(x), 0), elem(Float.parse(y), 0)}
  end

  defp assert_close({x, y}, {ex, ey}, tolerance) do
    assert abs(x - ex) <= tolerance and abs(y - ey) <= tolerance,
           "got #{x} #{y}, expected #{ex} #{ey}"
  end

  # The first sample point of a fixture entry: {lon, lat, x, y}
  defp sample(code) do
    [lon, lat, x, y | _] = get_in(@fixture, ["crs", code, "pts"]) |> hd()
    {lon, lat, x, y}
  end

  for code <- ["5515", "8441", "6244", "3139"] do
    test "EPSG:#{code} is not in the bundled table and is resolved from the supplementary one" do
      {lon, lat, x, y} = sample(unquote(code))
      out = point(lon, lat, "EPSG:4326") |> GeoSeries.to_crs("EPSG:" <> unquote(code))
      assert GeoSeries.crs(out) == "EPSG:" <> unquote(code)
      assert_close(xy(out), {x, y}, 0.05)
    end
  end

  for flavour <- ["projjson", "wkt1", "wkt2"], code <- ["2000", "3078", "5041"] do
    test "EPSG:#{code} given as #{flavour} with no identifier" do
      entry = get_in(@fixture, ["crs", unquote(code)])
      definition = entry[unquote(flavour)]
      definition = if is_map(definition), do: Jason.encode!(definition), else: definition
      {lon, lat, x, y} = sample(unquote(code))

      # set_crs keeps the text; to_crs converts it
      source = point(lon, lat, "EPSG:4326")
      out = GeoSeries.to_crs(source, definition)
      assert GeoSeries.crs(out) == definition
      assert_close(xy(out), {x, y}, 0.02)

      # and back from a series that carries the definition
      back = point(x, y, definition) |> GeoSeries.to_crs("EPSG:4326")
      {bx, by} = xy(back)
      assert_close({bx, by}, {lon, lat}, 1.0e-6)
    end
  end

  test "a definition and the code it describes give the same coordinates" do
    {lon, lat, _, _} = sample("32633")
    definition = get_in(@fixture, ["crs", "32633", "wkt2"])
    from_code = point(lon, lat, "EPSG:4326") |> GeoSeries.to_crs("EPSG:32633") |> xy()
    from_text = point(lon, lat, "EPSG:4326") |> GeoSeries.to_crs(definition) |> xy()
    assert_close(from_text, from_code, 1.0e-6)
  end

  test "a definition PROJ would call a ballpark keeps the coordinates, a known datum shifts" do
    # No identifier: a geographic CRS finds its shifts by name (NTF Paris), a custom one has none.
    {lon, lat, x, y} = sample("4807")
    definition = get_in(@fixture, ["crs", "4807", "projjson"]) |> Jason.encode!()
    out = point(lon, lat, "EPSG:4326") |> GeoSeries.to_crs(definition)
    assert_close(xy(out), {x, y}, 1.0e-6)
  end

  test "a definition that cannot be converted is an error naming the cause" do
    definition = ~s({"type":"GeographicCRS","name":"custom"})

    assert_raise ArgumentError, ~r/PROJJSON.*datum/s, fn ->
      point(1, 2, definition) |> GeoSeries.to_crs("EPSG:3857")
    end

    assert_raise ArgumentError, ~r/unknown EPSG code 999999/, fn ->
      point(1, 2, "EPSG:4326") |> GeoSeries.to_crs("EPSG:999999")
    end
  end
end
