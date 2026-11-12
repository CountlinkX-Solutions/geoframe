defmodule GeoFrame.ParityCTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @line "LINESTRING (0 0, 10 0, 10 10)"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)

  # Rows compare equal within 1e-9; two null rows count as equal.
  defp equal?(%GeoSeries{} = actual, expected) when is_list(expected) do
    actual
    |> GeoSeries.geom_equals_exact(s(expected), 1.0e-9)
    |> list()
  end

  defp close?(a, b), do: abs(a - b) < 1.0e-9

  describe "interpolate" do
    test "distance, negative distance and clamping" do
      series = s([@line, @line, @line, @line, nil, "POINT (1 1)"])
      out = GeoSeries.interpolate(series, [5, 15, -5, 99, 1, 1])

      assert GeoSeries.to_wkt(out) ==
               ["POINT(5 0)", "POINT(10 5)", "POINT(10 5)", "POINT(10 10)", nil, nil]
    end

    test "normalized at 0, 0.5 and 1" do
      series = s([@line])

      for {f, wkt} <- [{0, "POINT(0 0)"}, {0.5, "POINT(10 0)"}, {1, "POINT(10 10)"}] do
        assert series |> GeoSeries.interpolate(f, normalized: true) |> GeoSeries.to_wkt() ==
                 [wkt]
      end
    end

    test "a scalar broadcasts, a series works, and the CRS is kept" do
      series = s([@line, "LINESTRING (0 0, 4 0)"], crs: "EPSG:3857")
      out = GeoSeries.interpolate(series, Series.from_list([1, 2]))
      assert GeoSeries.to_wkt(out) == ["POINT(1 0)", "POINT(2 0)"]
      assert GeoSeries.crs(out) == "EPSG:3857"
      assert GeoSeries.to_wkt(GeoSeries.interpolate(series, 1)) == ["POINT(1 0)", "POINT(1 0)"]
    end

    test "multilinestring parts are concatenated; nil distance nulls the row" do
      series = s(["MULTILINESTRING ((0 0, 10 0), (100 100, 100 120))", @line])
      out = GeoSeries.interpolate(series, [15, nil])
      assert GeoSeries.to_wkt(out) == ["POINT(100 105)", nil]
    end

    test "errors" do
      assert_raise ArgumentError, ~r/length/, fn ->
        GeoSeries.interpolate(s([@line, @line]), [1])
      end

      assert_raise ArgumentError, fn -> GeoSeries.interpolate(s([@line]), "1") end
      assert_raise ArgumentError, fn -> GeoSeries.interpolate(s([@line]), 1, normalised: true) end
    end
  end

  describe "project" do
    test "a point off the line, plain and normalized" do
      series = s([@line])
      assert list(GeoSeries.project(series, "POINT (4 3)")) == [4.0]
      assert list(GeoSeries.project(series, "POINT (4 3)", normalized: true)) == [0.2]
      assert list(GeoSeries.project(series, "POINT (13 7)")) == [17.0]
    end

    test "series pairing, broadcast, nulls and non-lines" do
      lines = s([@line, "LINESTRING (0 0, 4 0)", nil, "POINT (0 0)"])
      assert list(GeoSeries.project(lines, "POINT (2 1)")) == [2.0, 2.0, nil, nil]
      points = s(["POINT (10 5)", "POINT (3 3)", "POINT (0 0)", "POINT (0 0)"])
      assert list(GeoSeries.project(lines, points)) == [15.0, 3.0, nil, nil]
    end

    test "CRS and length mismatches raise" do
      a = s([@line], crs: "EPSG:3857")
      b = s(["POINT (1 1)"], crs: "EPSG:4326")
      assert_raise ArgumentError, ~r/CRS/, fn -> GeoSeries.project(a, b) end
      two = s(["POINT (1 1)", "POINT (2 2)"])

      assert_raise ArgumentError, ~r/length/, fn ->
        GeoSeries.project(s([@line, @line, @line]), two)
      end
    end
  end

  describe "segmentize" do
    test "vertex counts and values" do
      out = GeoSeries.segmentize(s(["LINESTRING (0 0, 10 0)", "POINT (1 1)", nil]), 3)
      assert GeoSeries.to_wkt(out) == ["LINESTRING(0 0,2.5 0,5 0,7.5 0,10 0)", "POINT(1 1)", nil]
      assert list(GeoSeries.count_coordinates(out)) == [5, 1, nil]
    end

    test "polygons get vertices on every ring" do
      out = GeoSeries.segmentize(s(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"]), 2)
      assert list(GeoSeries.count_coordinates(out)) == [9]
    end

    test "a bad length raises" do
      for bad <- [0, -1] do
        assert_raise ArgumentError, fn -> GeoSeries.segmentize(s([@line]), bad) end
      end

      assert_raise ArgumentError, fn -> GeoSeries.segmentize(s([@line]), "1") end
    end
  end

  describe "remove_repeated_points" do
    test "exact repeats by default, tolerance on request" do
      series = s(["LINESTRING (0 0, 0 0, 1 1, 1 1, 2 2)", "LINESTRING (0 0, 0.1 0, 5 0)", nil])

      assert GeoSeries.to_wkt(GeoSeries.remove_repeated_points(series)) ==
               ["LINESTRING(0 0,1 1,2 2)", "LINESTRING(0 0,0.1 0,5 0)", nil]

      assert GeoSeries.to_wkt(GeoSeries.remove_repeated_points(series, 0.5)) ==
               ["LINESTRING(0 0,1 1,2 2)", "LINESTRING(0 0,5 0)", nil]
    end

    test "a negative tolerance raises" do
      assert_raise ArgumentError, fn -> GeoSeries.remove_repeated_points(s([@line]), -1) end
    end
  end

  describe "reverse and orientation" do
    test "reverse flips every line and ring" do
      series =
        s(["LINESTRING (0 0, 1 1, 2 0)", "POLYGON ((0 0, 4 0, 4 4, 0 0))", "POINT (1 1)", nil])

      assert GeoSeries.to_wkt(GeoSeries.reverse(series)) ==
               ["LINESTRING(2 0,1 1,0 0)", "POLYGON((0 0,4 4,4 0,0 0))", "POINT(1 1)", nil]
    end

    test "orient_polygons" do
      cw = "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"
      series = s([cw, "LINESTRING (0 0, 1 1)"])
      out = GeoSeries.orient_polygons(series)
      assert GeoSeries.to_wkt(out) == ["POLYGON((0 0,4 0,4 4,0 4,0 0))", "LINESTRING(0 0,1 1)"]
      back = GeoSeries.orient_polygons(out, exterior_cw: true)
      assert equal?(back, [cw, "LINESTRING (0 0, 1 1)"]) == [true, true]
    end

    test "is_ccw follows Shapely" do
      series =
        s([
          "LINESTRING (0 0, 1 0, 1 1, 0 0)",
          "LINESTRING (0 0, 1 1, 1 0, 0 0)",
          "LINESTRING (0 0, 1 1, 0 1)",
          "POLYGON ((0 0, 1 0, 1 1, 0 0))",
          nil
        ])

      assert list(GeoSeries.is_ccw(series)) == [true, false, false, false, nil]
    end
  end

  describe "distances" do
    test "hausdorff between offset lines and the Shapely docs values" do
      a = s(["LINESTRING (130 0, 0 0, 0 150)"])
      b = s(["LINESTRING (10 10, 10 150, 130 10)"])
      assert [d] = list(GeoSeries.hausdorff_distance(a, b))
      assert close?(d, 14.142135623730951)
      assert [d] = list(GeoSeries.hausdorff_distance(a, b, densify: 0.5))
      assert close?(d, 70.0)

      assert list(
               GeoSeries.hausdorff_distance(
                 s(["LINESTRING (0 0, 10 0)"]),
                 "LINESTRING (0 1, 10 1)"
               )
             ) == [1.0]
    end

    test "hausdorff validates densify and CRS" do
      a = s(["POINT (0 0)"])

      assert_raise ArgumentError, ~r/densify/, fn ->
        GeoSeries.hausdorff_distance(a, a, densify: 2)
      end

      assert_raise ArgumentError, fn -> GeoSeries.hausdorff_distance(a, a, densfy: 0.5) end
      b = s(["POINT (0 0)"], crs: "EPSG:4326")
      c = s(["POINT (0 0)"], crs: "EPSG:3857")
      assert_raise ArgumentError, ~r/CRS/, fn -> GeoSeries.hausdorff_distance(b, c) end
    end

    test "frechet of a reversed line; non-lines are null" do
      a = s(["LINESTRING (0 0, 10 0)", "POINT (0 0)", nil])
      assert list(GeoSeries.frechet_distance(a, "LINESTRING (10 0, 0 0)")) == [10.0, nil, nil]
      assert list(GeoSeries.frechet_distance(a, a)) == [0.0, nil, nil]

      b = s(["LINESTRING (0 1, 5 1, 10 1)"])

      assert [d] =
               list(GeoSeries.frechet_distance(s(["LINESTRING (0 0, 10 0)"]), b, densify: 0.5))

      assert close?(d, :math.sqrt(2.5 * 2.5 + 1))
    end
  end

  describe "shortest_line" do
    test "its length equals distance" do
      a = s(["LINESTRING (0 0, 10 0)", "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", nil])
      b = s(["LINESTRING (3 5, 20 5)", "POLYGON ((3 0, 4 0, 4 1, 3 1, 3 0))", "POINT (0 0)"])
      out = GeoSeries.shortest_line(a, b)
      assert list(GeoSeries.length(out)) == [5.0, 2.0, nil]
      assert list(GeoSeries.length(out)) == list(GeoSeries.distance(a, b))
    end

    test "points, broadcast and intersecting inputs" do
      a = s(["LINESTRING (0 0, 10 0)", "LINESTRING (0 0, 10 10)"])

      assert GeoSeries.to_wkt(GeoSeries.shortest_line(a, "POINT (4 3)")) |> hd() ==
               "LINESTRING(4 0,4 3)"

      out = GeoSeries.shortest_line(s(["LINESTRING (0 0, 10 10)"]), "LINESTRING (0 10, 10 0)")
      assert GeoSeries.to_wkt(out) == ["LINESTRING(5 5,5 5)"]
    end

    test "CRS mismatch raises" do
      a = s(["POINT (0 0)"], crs: "EPSG:4326")
      b = s(["POINT (1 1)"], crs: "EPSG:3857")
      assert_raise ArgumentError, ~r/CRS/, fn -> GeoSeries.shortest_line(a, b) end
    end
  end

  describe "line_merge" do
    test "three connected segments become one line" do
      series = s(["MULTILINESTRING ((0 0, 1 1), (1 1, 2 2), (2 2, 3 3))"])
      assert GeoSeries.to_wkt(GeoSeries.line_merge(series)) == ["LINESTRING(0 0,1 1,2 2,3 3)"]
    end

    test "a T-junction is not merged" do
      wkt = "MULTILINESTRING ((0 0, 5 0), (5 0, 10 0), (5 0, 5 5))"

      assert GeoSeries.to_wkt(GeoSeries.line_merge(s([wkt]))) ==
               ["MULTILINESTRING((0 0,5 0),(5 0,10 0),(5 0,5 5))"]
    end

    test "directed only joins head to tail" do
      series = s(["MULTILINESTRING ((0 0, 1 1), (2 2, 1 1))"])
      assert GeoSeries.to_wkt(GeoSeries.line_merge(series)) == ["LINESTRING(0 0,1 1,2 2)"]

      assert GeoSeries.to_wkt(GeoSeries.line_merge(series, directed: true)) ==
               ["MULTILINESTRING((0 0,1 1),(2 2,1 1))"]
    end

    test "passes lines through, empties other types, keeps nulls and CRS" do
      series = s(["LINESTRING (0 0, 1 1)", "POINT (1 1)", nil], crs: "EPSG:3857")
      out = GeoSeries.line_merge(series)
      assert GeoSeries.to_wkt(out) == ["LINESTRING(0 0,1 1)", "GEOMETRYCOLLECTION EMPTY", nil]
      assert GeoSeries.crs(out) == "EPSG:3857"
      assert_raise ArgumentError, fn -> GeoSeries.line_merge(series, directd: true) end
    end
  end

  describe "project on an overlapping line" do
    test "matches Shapely line_locate_point when a segment retraces an earlier one" do
      # Reference: shapely.line_locate_point(line, line_interpolate_point(line, d)) == 107.14922302131022
      # (the later pass), not the earlier ~44.64.
      series = s(["LINESTRING (48 24, -24 48, 6 38)"])
      point = GeoSeries.interpolate(series, 44.64010466677198)
      assert [back] = list(GeoSeries.project(series, point))
      assert_in_delta back, 107.14922302131022, 1.0e-9
    end
  end

  describe "properties" do
    defp coord, do: StreamData.integer(-50..50)

    defp line_wkt do
      gen all(
            pts <- StreamData.list_of({coord(), coord()}, min_length: 2, max_length: 6),
            pts |> Enum.chunk_every(2, 1, :discard) |> Enum.any?(fn [a, b] -> a != b end)
          ) do
        "LINESTRING (" <> Enum.map_join(pts, ", ", fn {x, y} -> "#{x} #{y}" end) <> ")"
      end
    end

    property "project(interpolate(d)) is d for d in [0, length]" do
      check all(wkt <- line_wkt(), f <- StreamData.float(min: 0.0, max: 1.0)) do
        series = s([wkt])
        [len] = list(GeoSeries.length(series))
        d = f * len
        point = GeoSeries.interpolate(series, d)
        [back] = list(GeoSeries.project(series, point))
        # A self-crossing or self-overlapping line passes through the same point more than
        # once, and GEOS picks the pass by float noise (first or last), exactly as Shapely
        # does. So compare the located point and the range, not the distance.
        again = GeoSeries.interpolate(series, back)
        assert Enum.all?(list(GeoSeries.geom_equals_exact(point, again, 1.0e-6)))
        assert back >= 0.0 and back <= len + 1.0e-6
      end
    end

    property "segmentize keeps the length and bounds segment size" do
      check all(wkt <- line_wkt(), m <- StreamData.integer(1..20)) do
        series = s([wkt])
        out = GeoSeries.segmentize(series, m)
        assert [a] = list(GeoSeries.length(series))
        assert [b] = list(GeoSeries.length(out))
        assert abs(a - b) < 1.0e-9 * max(a, 1.0)

        df = GeoSeries.get_coordinates(out)
        xs = Series.to_list(df["x"])
        ys = Series.to_list(df["y"])

        longest =
          [xs, ys]
          |> Enum.map(&Enum.chunk_every(&1, 2, 1, :discard))
          |> Enum.zip_with(fn [[x0, x1], [y0, y1]] ->
            :math.sqrt((x1 - x0) ** 2 + (y1 - y0) ** 2)
          end)
          |> Enum.max()

        assert longest <= m + 1.0e-9
      end
    end

    property "reverse twice is the identity" do
      check all(wkt <- line_wkt()) do
        series = s([wkt, "POLYGON ((0 0, 4 0, 4 4, 0 0))"])
        back = series |> GeoSeries.reverse() |> GeoSeries.reverse()
        assert list(GeoSeries.geom_equals_identical(back, series)) == [true, true]
      end
    end

    property "shortest_line length equals distance" do
      check all(a <- line_wkt(), b <- line_wkt()) do
        left = s([a])
        right = s([b])
        [d] = list(GeoSeries.distance(left, right))
        [l] = left |> GeoSeries.shortest_line(right) |> GeoSeries.length() |> list()
        assert abs(d - l) < 1.0e-9
      end
    end

    property "hausdorff of a geometry with itself is zero" do
      check all(wkt <- line_wkt()) do
        series = s([wkt])
        assert list(GeoSeries.hausdorff_distance(series, series)) == [0.0]
      end
    end
  end
end
