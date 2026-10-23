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

  describe "offset_curve/3" do
    test "offsets to the left for positive and to the right for negative distances" do
      series = s(["LINESTRING (0 0, 10 0)", "LINESTRING (0 0, 10 0)"])

      assert series |> GeoSeries.offset_curve(1) |> GeoSeries.to_wkt() ==
               ["LINESTRING(0 1,10 1)", "LINESTRING(0 1,10 1)"]

      assert series |> GeoSeries.offset_curve(-2) |> GeoSeries.to_wkt() ==
               ["LINESTRING(0 -2,10 -2)", "LINESTRING(0 -2,10 -2)"]
    end

    test "takes a distance per row and keeps nulls and the CRS" do
      series = s(["LINESTRING (0 0, 10 0)", nil, "LINESTRING (0 0, 10 0)"], crs: "EPSG:3857")
      result = GeoSeries.offset_curve(series, [1, 1, nil])

      assert GeoSeries.to_wkt(result) == ["LINESTRING(0 1,10 1)", nil, nil]
      assert GeoSeries.crs(result) == "EPSG:3857"

      by_series = GeoSeries.offset_curve(series, Series.from_list([2.0, 2.0, 3.0]))
      assert GeoSeries.to_wkt(by_series) == ["LINESTRING(0 2,10 2)", nil, "LINESTRING(0 3,10 3)"]
    end

    test "join styles round, mitre and bevel" do
      series = s(["LINESTRING (0 0, 3 4, 6 4)"])

      mitre = GeoSeries.offset_curve(series, 1, join_style: :mitre)
      bevel = GeoSeries.offset_curve(series, 1, join_style: :bevel)

      assert list(
               GeoSeries.geom_equals_exact(
                 mitre,
                 s(["LINESTRING (-0.8 0.6, 2.5 5, 6 5)"]),
                 1.0e-9
               )
             ) ==
               [true]

      assert list(
               GeoSeries.geom_equals_exact(
                 bevel,
                 s(["LINESTRING (-0.8 0.6, 2.2 4.6, 3 5, 6 5)"]),
                 1.0e-9
               )
             ) ==
               [true]

      [round] = series |> GeoSeries.offset_curve(1, quad_segs: 16) |> GeoSeries.to_wkt()
      [coarse] = series |> GeoSeries.offset_curve(1, quad_segs: 8) |> GeoSeries.to_wkt()
      assert String.length(round) > String.length(coarse)
    end

    test "returns a multi-line when the curve breaks and an empty line when nothing remains" do
      series =
        s(["LINESTRING (0 0, 4 4, 4 0, 0 4)", "POINT (1 1)", "LINESTRING (0 0, 10 0, 0 0.5)"])

      assert [multi, empty, gone] = series |> GeoSeries.offset_curve(1) |> GeoSeries.to_wkt()
      assert multi =~ "MULTILINESTRING"
      assert empty == "LINESTRING EMPTY"
      assert gone == "LINESTRING EMPTY"
    end

    test "offsets the rings of a polygon outwards for positive distances" do
      series = s(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"])
      [ring] = series |> GeoSeries.offset_curve(1, join_style: :mitre) |> GeoSeries.to_wkt()
      assert ring =~ "LINESTRING"

      [length] =
        series |> GeoSeries.offset_curve(1, join_style: :mitre) |> GeoSeries.length() |> list()

      assert_in_delta length, 24.0, 1.0e-9
    end

    test "validates its options and the distance" do
      series = s(["LINESTRING (0 0, 1 1)"])

      assert_raise ArgumentError, ~r/join_style/, fn ->
        GeoSeries.offset_curve(series, 1, join_style: :x)
      end

      assert_raise ArgumentError, ~r/quad_segs/, fn ->
        GeoSeries.offset_curve(series, 1, quad_segs: 0)
      end

      assert_raise ArgumentError, ~r/quad_segs/, fn ->
        GeoSeries.offset_curve(series, 1, quad_segs: 100_000)
      end

      assert_raise ArgumentError, ~r/mitre_limit/, fn ->
        GeoSeries.offset_curve(series, 1, mitre_limit: 0)
      end

      assert_raise ArgumentError, ~r/distance/, fn -> GeoSeries.offset_curve(series, "x") end

      assert_raise ArgumentError, ~r/length mismatch/, fn ->
        GeoSeries.offset_curve(series, [1, 2])
      end

      assert_raise ArgumentError, fn -> GeoSeries.offset_curve(series, 1, bogus: true) end
    end

    test "refuses parameters that would build millions of vertices" do
      coords = Enum.map_join(0..999, ", ", fn i -> "#{i} #{rem(i, 2)}" end)
      series = s(["LINESTRING (#{coords})"])

      assert_raise ArgumentError, ~r/vertices/, fn ->
        GeoSeries.offset_curve(series, 0.1, quad_segs: 10_000)
      end
    end
  end
end
