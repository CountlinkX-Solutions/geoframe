defmodule GeoFrame.PostgisValidationTest do
  # Argument validation runs before any connection is used, so no database is needed.
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp frame(columns) do
    data = DataFrame.new(Map.new(columns, &{"c#{&1}", [&1]}))
    GeoDataFrame.new(data, GeoSeries.from_wkt!(["POINT (1 1)"], crs: "EPSG:4326"))
  end

  test "dtype overrides with constraints are rejected" do
    for bad <- [
          "integer PRIMARY KEY",
          "integer NOT NULL",
          "text DEFAULT 'x'",
          "int, extra int",
          "integer; DROP TABLE x",
          "double precision NOT NULL",
          ""
        ] do
      assert_raise ArgumentError, ~r/invalid SQL type/, fn ->
        GeoFrame.to_postgis(frame([1]), nil, "t", dtype: %{"c1" => bad})
      end
    end
  end

  test "a frame wider than PostgreSQL allows is an argument error, not a crash" do
    assert_raise ArgumentError, ~r/1600 columns/, fn ->
      GeoFrame.to_postgis(frame(1..1700), nil, "t")
    end
  end
end
