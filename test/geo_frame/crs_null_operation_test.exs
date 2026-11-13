defmodule GeoFrame.CrsNullOperationTest do
  # The operation to WGS 84 that PROJ picks first is a null one for GDA94 and GDA2020 (a no-op, 3 m
  # accuracy): GeoFrame used to run the 1.7 m Helmert step. Values are pyproj 3.8.0 / PROJ 9.8.1
  # (`Transformer.from_crs(code, 4326, always_xy=True).transform(x, y)`), no grids installed.
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  # 1e-7 of a degree is about a centimetre
  @tolerance 1.0e-7

  defp to_wgs84(x, y, crs) do
    [wkt] =
      GeoSeries.from_wkt!(["POINT (#{x} #{y})"], crs: crs)
      |> GeoSeries.to_crs("EPSG:4326")
      |> GeoSeries.to_wkt()

    [lon, lat] = Regex.run(~r/\(([^ ]+) ([^)]+)\)/, wkt, capture: :all_but_first)
    {elem(Float.parse(lon), 0), elem(Float.parse(lat), 0)}
  end

  for {crs, x, y, lon, lat} <- [
        {"EPSG:4283", 133.0, -25.0, 133.0, -25.0},
        {"EPSG:28355", 888_447.1826766436, 6_240_982.385726567, 151.2, -33.9},
        {"EPSG:7855", 888_447.1826766436, 6_240_982.385726567, 151.2, -33.9},
        # ED50 in the Norwegian Lapland: PROJ takes the onshore Finland and Norway operation
        {"EPSG:4230", 18.95, 69.65, 18.948413480726057, 69.65004132575966},
        {"EPSG:4230", 10.75, 59.91, 10.748621952306841, 59.909548701527996}
      ] do
    test "#{crs} at #{x} #{y} is moved like pyproj" do
      {gx, gy} = to_wgs84(unquote(x), unquote(y), unquote(crs))
      assert_in_delta gx, unquote(lon), @tolerance
      assert_in_delta gy, unquote(lat), @tolerance
    end
  end
end
