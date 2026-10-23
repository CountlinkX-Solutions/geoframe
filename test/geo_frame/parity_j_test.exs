defmodule GeoFrame.ParityJTest do
  use ExUnit.Case, async: true

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)
  defp canon(wkt), do: [wkt] |> s() |> GeoSeries.to_wkt() |> hd()

  describe "snap/3" do
    test "snaps vertices and inserts snap points into segments" do
      series =
        s([
          "POINT (0.5 2.5)",
          "LINESTRING (0 0, 10 0)",
          "POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0))"
        ])

      result = GeoSeries.snap(series, "POINT (0 2)", 1)

      assert GeoSeries.to_wkt(result) ==
               [
                 canon("POINT (0 2)"),
                 canon("LINESTRING (0 0, 10 0)"),
                 canon("POLYGON ((0 0, 0 2, 0 10, 10 10, 10 0, 0 0))")
               ]
    end

    test "pairs rows with another series and keeps CRS and nulls" do
      a = s(["LINESTRING (0 0, 10 0)", nil, "POINT (1 1)"], crs: "EPSG:4326")
      b = s(["POINT (5 0.5)", "POINT (0 0)", nil], crs: "EPSG:4326")

      result = GeoSeries.snap(a, b, 1.0)

      assert GeoSeries.to_wkt(result) == [canon("LINESTRING (0 0, 5 0.5, 10 0)"), nil, nil]
      assert GeoSeries.crs(result) == "EPSG:4326"
    end

    test "rejects a bad tolerance and mismatched lengths" do
      a = s(["POINT (0 0)", "POINT (1 1)"])
      assert_raise ArgumentError, ~r/tolerance/, fn -> GeoSeries.snap(a, "POINT (0 0)", "x") end

      assert_raise ArgumentError, ~r/length mismatch/, fn ->
        GeoSeries.snap(a, s(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"]), 1)
      end
    end
  end

  describe "shared_paths/2" do
    test "returns the same-direction and opposite-direction stretches" do
      a = s(["LINESTRING (0 0, 2 0, 4 0)", "LINESTRING (0 0, 4 0)", "LINESTRING (0 0, 4 0)"])
      b = s(["LINESTRING (0 0, 4 0)", "LINESTRING (4 0, 0 0)", "LINESTRING (2 0, 2 4)"])

      result = a |> GeoSeries.shared_paths(b) |> GeoSeries.normalize() |> GeoSeries.to_wkt()

      assert result ==
               [
                 "GEOMETRYCOLLECTION (MULTILINESTRING ((0 0, 2 0), (2 0, 4 0)), MULTILINESTRING EMPTY)",
                 "GEOMETRYCOLLECTION (MULTILINESTRING EMPTY, MULTILINESTRING ((0 0, 4 0)))",
                 "GEOMETRYCOLLECTION (MULTILINESTRING EMPTY, MULTILINESTRING EMPTY)"
               ]
               |> s()
               |> GeoSeries.normalize()
               |> GeoSeries.to_wkt()
    end

    test "gives nil for non-linear and null rows" do
      a = s(["POINT (0 0)", nil, "LINESTRING (0 0, 1 0)"])
      result = GeoSeries.shared_paths(a, "LINESTRING (0 0, 1 0)")
      assert [nil, nil, wkt] = GeoSeries.to_wkt(result)
      assert wkt =~ "GEOMETRYCOLLECTION"
    end
  end

  describe "minimum_clearance/1 and minimum_clearance_line/1" do
    test "report the clearance, infinity when none exists, nil for nulls" do
      series =
        s([
          "POINT (1 1)",
          "LINESTRING (0 0, 10 0, 5 0.5)",
          "POLYGON ((0 0, 6 0, 3 5, 0 0))",
          "LINESTRING EMPTY",
          nil
        ])

      assert list(GeoSeries.minimum_clearance(series)) == [:infinity, 0.5, 5.0, :infinity, nil]

      assert series
             |> GeoSeries.minimum_clearance_line()
             |> GeoSeries.normalize()
             |> GeoSeries.to_wkt() ==
               [
                 "LINESTRING EMPTY",
                 "LINESTRING (5 0, 5 0.5)",
                 "LINESTRING (3 0, 3 5)",
                 "LINESTRING EMPTY",
                 nil
               ]
               |> Enum.map(fn
                 nil -> nil
                 wkt -> wkt |> canon_n()
               end)
    end

    defp canon_n(wkt), do: [wkt] |> s() |> GeoSeries.normalize() |> GeoSeries.to_wkt() |> hd()
  end
end
