defmodule GeoFrame.ParityHTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @square "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"
  @reversed "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)

  describe "geom_equals_exact / geom_equals_identical" do
    test "reversed ring is not exactly equal but is topologically equal" do
      a = s([@square])
      b = s([@reversed])
      assert list(GeoSeries.geom_equals_exact(a, b, 0.0)) == [false]
      assert list(GeoSeries.geom_equals(a, b)) == [true]
      assert list(GeoSeries.geom_equals_identical(a, b)) == [false]
    end

    test "tolerance, broadcast, nulls and WKT right-hand side" do
      a = s(["POINT (1 1)", nil, "POINT (1.0004 1)"])
      assert list(GeoSeries.geom_equals_exact(a, "POINT (1 1)", 0.001)) == [true, nil, true]
      assert list(GeoSeries.geom_equals_exact(a, "POINT (1 1)", 0.0)) == [true, nil, false]
      assert list(GeoSeries.geom_equals_identical(a, "POINT (1 1)")) == [true, nil, false]
    end

    test "validates tolerance, lengths and CRS" do
      a = s(["POINT (1 1)", "POINT (2 2)"])
      assert_raise ArgumentError, fn -> GeoSeries.geom_equals_exact(a, a, -1) end

      assert_raise FunctionClauseError, fn ->
        GeoSeries.geom_equals_exact(a, s(["POINT (1 1)"]), :x)
      end

      assert_raise ArgumentError, ~r/length mismatch/, fn ->
        GeoSeries.geom_equals_identical(a, s(["POINT (1 1)", "POINT (1 1)", "POINT (1 1)"]))
      end

      assert_raise ArgumentError, ~r/CRS mismatch/, fn ->
        GeoSeries.geom_equals_exact(
          s(["POINT (1 1)"], crs: "EPSG:4326"),
          s(["POINT (1 1)"], crs: "EPSG:3857"),
          0
        )
      end
    end

    property "a series is exactly and identically equal to itself" do
      check all(x <- float(min: -1.0e6, max: 1.0e6), y <- float(min: -1.0e6, max: 1.0e6)) do
        a = GeoSeries.from_xy([x], [y])
        assert list(GeoSeries.geom_equals_exact(a, a, 0.0)) == [true]
        assert list(GeoSeries.geom_equals_identical(a, a)) == [true]
      end
    end
  end

  describe "contains_properly" do
    test "boundary point is contained but not properly" do
      sq = s(List.duplicate(@square, 3))
      pts = s(["POINT (2 2)", "POINT (0 2)", "POINT (9 9)"])
      assert list(GeoSeries.contains_properly(sq, pts)) == [true, false, false]
      assert list(GeoSeries.contains(sq, pts)) == [true, false, false]
      assert list(GeoSeries.contains_properly(sq, "POINT (2 2)")) == [true, true, true]
    end

    test "line touching the boundary is contained, not properly" do
      sq = s([@square])
      line = s(["LINESTRING (0 2, 2 2)"])
      assert list(GeoSeries.contains(sq, line)) == [true]
      assert list(GeoSeries.contains_properly(sq, line)) == [false]
    end

    test "works through sindex_query and sjoin" do
      polys = s([@square, "POLYGON ((10 10, 14 10, 14 14, 10 14, 10 10))"])
      pts = s(["POINT (2 2)", "POINT (0 2)", "POINT (12 12)"])

      assert GeoSeries.sindex_query(polys, pts, :contains_properly) == {[0, 1], [0, 2]}

      left = GeoDataFrame.new(DataFrame.new(id: [1, 2]), polys)
      right = GeoDataFrame.new(DataFrame.new(name: ["a", "b", "c"]), pts)
      joined = GeoDataFrame.sjoin(left, right, predicate: :contains_properly)
      assert GeoDataFrame.n_rows(joined) == 2
    end

    property "contains_properly implies contains" do
      check all(
              x <- float(min: -1.0, max: 5.0),
              y <- float(min: -1.0, max: 5.0)
            ) do
        pt = GeoSeries.from_xy([x], [y])
        sq = s([@square])
        proper = list(GeoSeries.contains_properly(sq, pt))
        plain = list(GeoSeries.contains(sq, pt))
        assert proper == [false] or plain == [true]
      end
    end
  end

  describe "relate_pattern" do
    test "touching squares" do
      a = s([@square])
      b = s(["POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))"])
      assert list(GeoSeries.relate(a, b)) == ["FF2F11212"]
      assert list(GeoSeries.relate_pattern(a, b, "FF*F1****")) == [true]
      assert list(GeoSeries.relate_pattern(a, b, "T********")) == [false]
      assert list(GeoSeries.relate_pattern(a, "POINT (9 9)", "FF2FF10F2")) == [true]
    end

    test "validates the pattern" do
      a = s([@square])

      assert_raise ArgumentError, ~r/invalid DE-9IM pattern/, fn ->
        GeoSeries.relate_pattern(a, a, "T***")
      end

      assert_raise ArgumentError, ~r/invalid DE-9IM pattern/, fn ->
        GeoSeries.relate_pattern(a, a, "X********")
      end

      assert_raise FunctionClauseError, fn -> GeoSeries.relate_pattern(a, a, :nope) end
    end

    property "a pair always matches its own matrix" do
      check all(
              x <- float(min: -2.0, max: 6.0),
              y <- float(min: -2.0, max: 6.0)
            ) do
        a = s([@square])
        b = GeoSeries.from_xy([x], [y])
        [code] = a |> GeoSeries.relate(b) |> list()
        assert list(GeoSeries.relate_pattern(a, b, code)) == [true]
      end
    end
  end

  describe "cx" do
    setup do
      %{
        series:
          s([
            "POINT (1 1)",
            "LINESTRING (0 0, 10 10)",
            nil,
            "POLYGON ((20 20, 22 20, 22 22, 20 22, 20 20))",
            "POINT (5 0)"
          ])
      }
    end

    test "selects rows intersecting the box", %{series: series} do
      out = GeoSeries.cx(series, 0, 0, 2, 2)
      assert GeoSeries.to_wkt(out) == ["POINT(1 1)", "LINESTRING(0 0,10 10)"]
    end

    test "nil bounds are unbounded", %{series: series} do
      assert GeoSeries.size(GeoSeries.cx(series, nil, nil, nil, nil)) == 4
      assert GeoSeries.to_wkt(GeoSeries.cx(series, 15, nil, nil, nil)) |> length() == 1
      assert GeoSeries.to_wkt(GeoSeries.cx(series, nil, nil, 1, 1)) |> length() == 2
    end

    test "empty result and empty input" do
      assert GeoSeries.size(GeoSeries.cx(s(["POINT (1 1)"]), 5, 5, 6, 6)) == 0
      assert GeoSeries.size(GeoSeries.cx(s([]), 0, 0, 1, 1)) == 0
    end

    test "rejects an inverted box", %{series: series} do
      assert_raise ArgumentError, fn -> GeoSeries.cx(series, 5, 0, 1, 2) end
    end

    test "GeoDataFrame.cx keeps attributes aligned", %{series: series} do
      gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2, 3, 4, 5]), series)
      out = GeoDataFrame.cx(gdf, 0, 0, 2, 2)
      assert out |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list() == [1, 2]

      assert GeoSeries.to_wkt(GeoDataFrame.geometry(out)) == [
               "POINT(1 1)",
               "LINESTRING(0 0,10 10)"
             ]
    end

    property "equals a brute-force intersects filter and is a subset" do
      check all(
              points <-
                list_of({integer(-10..10), integer(-10..10)}, min_length: 1, max_length: 20),
              {x0, x1} <- {integer(-10..10), integer(-10..10)},
              {y0, y1} <- {integer(-10..10), integer(-10..10)}
            ) do
        {xmin, xmax} = Enum.min_max([x0, x1])
        {ymin, ymax} = Enum.min_max([y0, y1])
        {xs, ys} = Enum.unzip(points)
        series = GeoSeries.from_xy(xs, ys)

        expected =
          points
          |> Enum.filter(fn {x, y} -> x >= xmin and x <= xmax and y >= ymin and y <= ymax end)
          |> Enum.map(fn {x, y} -> "POINT(#{x} #{y})" end)

        out = GeoSeries.cx(series, xmin, ymin, xmax, ymax)
        assert GeoSeries.to_wkt(out) == expected
        assert GeoSeries.size(out) <= GeoSeries.size(series)
      end
    end
  end

  describe "spatial index helpers" do
    test "valid_query_predicates lists what sindex_query accepts" do
      predicates = GeoSeries.sindex_valid_query_predicates()
      assert :contains_properly in predicates
      assert :dwithin in predicates
      refute :disjoint in predicates
      a = s([@square])

      for p <- predicates do
        opts = if p == :dwithin, do: [distance: 1.0], else: []
        assert {_, _} = GeoSeries.sindex_query(a, a, p, opts)
      end
    end

    test "sindex_intersection returns rows whose envelope meets the bbox" do
      series = s(["POINT (1 1)", "LINESTRING (5 5, 9 9)", nil, "POINT (2 0)"])
      assert GeoSeries.sindex_intersection(series, {0, 0, 2, 2}) == [0, 3]
      assert GeoSeries.sindex_intersection(series, {5.0, 8.0, 6.0, 9.0}) == [1]
      assert GeoSeries.sindex_intersection(series, {50, 50, 60, 60}) == []
      assert_raise ArgumentError, fn -> GeoSeries.sindex_intersection(series, {2, 0, 0, 2}) end
    end
  end

  describe "concat" do
    test "GeoSeries.concat appends in order" do
      out = GeoSeries.concat([s(["POINT (1 1)", nil]), s([@square])])
      assert GeoSeries.size(out) == 3
      assert GeoSeries.to_wkt(out) |> Enum.at(1) == nil
    end

    test "CRS must match" do
      a = s(["POINT (1 1)"], crs: "EPSG:4326")
      b = s(["POINT (1 1)"], crs: "EPSG:3857")
      assert_raise ArgumentError, ~r/CRS mismatch/, fn -> GeoSeries.concat([a, b]) end
      assert_raise ArgumentError, fn -> GeoSeries.concat([]) end
      assert GeoSeries.crs(GeoSeries.concat([a, a])) == "EPSG:4326"
    end

    test "GeoDataFrame.concat_rows stacks attributes and geometry" do
      a =
        GeoDataFrame.new(
          DataFrame.new(id: [1, 2], v: ["a", "b"]),
          s(["POINT (0 0)", "POINT (1 1)"])
        )

      b = GeoDataFrame.new(DataFrame.new(id: [3], v: ["c"]), s(["POINT (2 2)"]))
      out = GeoDataFrame.concat_rows([a, b])
      assert GeoDataFrame.n_rows(out) == 3
      assert GeoDataFrame.names(out) == ["id", "v", "geometry"]
      assert out |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list() == [1, 2, 3]
      assert GeoFrame.concat([a, b]) |> GeoDataFrame.n_rows() == 3
      assert GeoFrame.concat([s(["POINT (0 0)"]), s(["POINT (1 1)"])]) |> GeoSeries.size() == 2
    end

    test "rejects schema, geometry column and CRS mismatches" do
      a = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (0 0)"]))
      other_cols = GeoDataFrame.new(DataFrame.new(name: ["x"]), s(["POINT (0 0)"]))

      other_geom =
        GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (0 0)"]), geometry_column: "geom")

      other_crs = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (0 0)"], crs: "EPSG:4326"))

      assert_raise ArgumentError, fn -> GeoDataFrame.concat_rows([a, other_cols]) end

      assert_raise ArgumentError, ~r/geometry column/, fn ->
        GeoDataFrame.concat_rows([a, other_geom])
      end

      assert_raise ArgumentError, ~r/CRS mismatch/, fn ->
        GeoDataFrame.concat_rows([a, other_crs])
      end

      assert_raise ArgumentError, fn -> GeoDataFrame.concat_rows([]) end
      assert_raise ArgumentError, fn -> GeoFrame.concat([a, s(["POINT (0 0)"])]) end
    end

    test "frames without attribute columns concatenate" do
      a = GeoDataFrame.new(DataFrame.new([]), s(["POINT (0 0)"]))
      out = GeoDataFrame.concat_rows([a, a])
      assert GeoDataFrame.n_rows(out) == 2
    end

    property "length is the sum of the parts" do
      check all(sizes <- list_of(integer(0..6), min_length: 1, max_length: 5)) do
        parts =
          Enum.map(sizes, fn n ->
            GeoSeries.from_xy(Enum.to_list(1..n//1), Enum.to_list(1..n//1))
          end)

        assert parts |> GeoSeries.concat() |> GeoSeries.size() == Enum.sum(sizes)
      end
    end
  end
end
