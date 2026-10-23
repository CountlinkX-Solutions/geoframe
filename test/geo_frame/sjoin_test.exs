defmodule GeoFrame.SjoinTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)

  defp square(x, y, size \\ 2),
    do:
      "POLYGON ((#{x} #{y}, #{x + size} #{y}, #{x + size} #{y + size}, #{x} #{y + size}, #{x} #{y}))"

  # Two polygons (A, B) and three points: one in A, one in B, one nowhere.
  setup do
    polygons =
      GeoDataFrame.new(
        DataFrame.new(name: ["A", "B"], value: [10, 20]),
        s([square(0, 0), square(10, 10)])
      )

    points =
      GeoDataFrame.new(
        DataFrame.new(id: [1, 2, 3]),
        s(["POINT (1 1)", "POINT (11 11)", "POINT (50 50)"])
      )

    %{polygons: polygons, points: points}
  end

  defp col(gdf, name), do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  describe "sindex_query/4" do
    test "returns parallel index lists ordered by (left, right)" do
      left = s(["POINT (1 1)", "POINT (11 11)", "POINT (50 50)", "POINT (2 2)"])
      right = s([square(10, 10), square(0, 0), square(1, 1, 5)])

      assert GeoSeries.sindex_query(left, right) ==
               {[0, 0, 1, 3, 3], [1, 2, 0, 1, 2]}

      # (1 1) and (2 2) sit on the boundary of some squares, so they are not "within" those.
      assert GeoSeries.sindex_query(left, right, :within) == {[0, 1, 3], [1, 0, 2]}
    end

    test "predicate direction is left.predicate(right)" do
      poly = s([square(0, 0, 4)])
      pts = s(["POINT (2 2)", "POINT (0 2)"])
      assert GeoSeries.sindex_query(poly, pts, :contains) == {[0], [0]}
      assert GeoSeries.sindex_query(poly, pts, :covers) == {[0, 0], [0, 1]}
      assert GeoSeries.sindex_query(pts, poly, :within) == {[0], [0]}
    end

    test "dwithin takes :distance" do
      left = s(["POINT (0 0)"])
      right = s(["POINT (3 4)", "POINT (30 40)"])
      assert GeoSeries.sindex_query(left, right, :dwithin, distance: 5) == {[0], [0]}
      assert GeoSeries.sindex_query(left, right, :dwithin, distance: 4.9) == {[], []}
    end

    test "errors" do
      a = s(["POINT (0 0)"], crs: "EPSG:4326")
      b = s(["POINT (0 0)"], crs: "EPSG:3857")
      assert_raise ArgumentError, ~r/CRS mismatch/, fn -> GeoSeries.sindex_query(a, b) end

      assert_raise ArgumentError, ~r/requires a distance/, fn ->
        GeoSeries.sindex_query(a, a, :dwithin)
      end

      assert_raise ArgumentError, ~r/disjoint/, fn -> GeoSeries.sindex_query(a, a, :disjoint) end

      assert_raise ArgumentError, ~r/unknown predicate/, fn ->
        GeoSeries.sindex_query(a, a, :bogus)
      end
    end

    property "equals brute force over all pairs, for several predicates" do
      predicates = [:intersects, :within, :contains, :touches, :covers, :covered_by, :crosses]

      point =
        StreamData.map({StreamData.integer(0..6), StreamData.integer(0..6)}, fn {x, y} ->
          "POINT (#{x} #{y})"
        end)

      rect =
        StreamData.map(
          {StreamData.integer(0..5), StreamData.integer(0..5), StreamData.integer(1..4)},
          fn {x, y, size} -> square(x, y, size) end
        )

      check all(
              lefts <- StreamData.list_of(StreamData.one_of([point, rect]), max_length: 12),
              rights <- StreamData.list_of(StreamData.one_of([point, rect]), max_length: 12),
              predicate <- StreamData.member_of(predicates),
              max_runs: 60
            ) do
        l = s(lefts)
        r = s(rights)
        {li, ri} = GeoSeries.sindex_query(l, r, predicate)

        expected =
          for {lw, i} <- Enum.with_index(lefts),
              {rw, j} <- Enum.with_index(rights),
              apply(GeoSeries, predicate, [s([lw]), s([rw])]) |> Series.to_list() == [true],
              do: {i, j}

        assert Enum.zip(li, ri) == expected
      end
    end
  end

  describe "sjoin/3 points in polygons" do
    test "inner (default): left geometry, left + right attributes, index_right", %{
      polygons: polygons,
      points: points
    } do
      out = GeoFrame.sjoin(points, polygons, predicate: :within)

      assert GeoDataFrame.names(out) == ["id", "name", "value", "index_right", "geometry"]
      assert col(out, "id") == [1, 2]
      assert col(out, "name") == ["A", "B"]
      assert col(out, "value") == [10, 20]
      assert col(out, "index_right") == [0, 1]
      assert out.data |> DataFrame.pull("index_right") |> Series.dtype() == {:s, 64}
      assert GeoSeries.to_wkt(out.geometry) == ["POINT(1 1)", "POINT(11 11)"]
    end

    test "default predicate is intersects; GeoDataFrame.sjoin/3 is the same function", %{
      polygons: polygons,
      points: points
    } do
      assert GeoDataFrame.sjoin(points, polygons) |> col("id") == [1, 2]
    end

    test "left join keeps unmatched left rows with nulls on the right", %{
      polygons: polygons,
      points: points
    } do
      out = GeoFrame.sjoin(points, polygons, how: :left, predicate: :within)
      assert col(out, "id") == [1, 2, 3]
      assert col(out, "name") == ["A", "B", nil]
      assert col(out, "value") == [10, 20, nil]
      assert col(out, "index_right") == [0, 1, nil]
      assert GeoSeries.to_wkt(out.geometry) |> List.last() == "POINT(50 50)"
    end

    test "right join uses the right geometry, index_left, ordered by right row", %{
      polygons: polygons,
      points: points
    } do
      extra =
        GeoDataFrame.new(
          DataFrame.new(name: ["A", "B", "C"]),
          s([square(10, 10), square(0, 0), square(90, 90)])
        )

      out = GeoFrame.sjoin(points, extra, how: :right, predicate: :within)
      assert GeoDataFrame.names(out) == ["id", "name", "index_left", "geometry"]
      assert col(out, "name") == ["A", "B", "C"]
      assert col(out, "id") == [2, 1, nil]
      assert col(out, "index_left") == [1, 0, nil]
      assert GeoSeries.to_wkt(out.geometry) |> length() == 3
      assert GeoSeries.to_wkt(out.geometry) |> hd() == GeoSeries.to_wkt(extra.geometry) |> hd()
      _ = polygons
    end

    test "one-to-many matches repeat the left row, ordered by left then right" do
      left = GeoDataFrame.new(DataFrame.new(id: [1, 2]), s(["POINT (1 1)", "POINT (9 9)"]))

      right =
        GeoDataFrame.new(
          DataFrame.new(tag: ["big", "small", "far"]),
          s([square(0, 0, 5), square(0, 0, 2), square(20, 20)])
        )

      out = GeoFrame.sjoin(left, right)
      assert col(out, "id") == [1, 1]
      assert col(out, "tag") == ["big", "small"]
      assert col(out, "index_right") == [0, 1]
    end

    test "colliding attribute names get suffixes; geometry column is the left one" do
      left =
        GeoDataFrame.new(DataFrame.new(id: [1], name: ["pt"]), s(["POINT (1 1)"]),
          geometry_column: "geom"
        )

      right =
        GeoDataFrame.new(DataFrame.new(id: [7], name: ["poly"]), s([square(0, 0)]))

      out = GeoFrame.sjoin(left, right)

      assert GeoDataFrame.names(out) == [
               "id_left",
               "name_left",
               "id_right",
               "name_right",
               "index_right",
               "geom"
             ]

      assert col(out, "id_left") == [1]
      assert col(out, "id_right") == [7]

      out = GeoFrame.sjoin(left, right, lsuffix: "l", rsuffix: "r")
      assert "id_l" in GeoDataFrame.names(out)
      assert "name_r" in GeoDataFrame.names(out)
    end

    test "dwithin joins by distance", %{points: points} do
      near = GeoDataFrame.new(DataFrame.new(k: ["x"]), s(["POINT (1 4)"]))
      out = GeoFrame.sjoin(points, near, predicate: :dwithin, distance: 3)
      assert col(out, "id") == [1]

      assert_raise ArgumentError, ~r/distance/, fn ->
        GeoFrame.sjoin(points, near, predicate: :dwithin)
      end
    end

    test "frames without attribute columns work" do
      left = GeoDataFrame.new(DataFrame.new([]), s(["POINT (1 1)", "POINT (5 5)"]))
      right = GeoDataFrame.new(DataFrame.new([]), s([square(0, 0)]))
      out = GeoFrame.sjoin(left, right, how: :left)
      assert GeoDataFrame.names(out) == ["index_right", "geometry"]
      assert col(out, "index_right") == [0, nil]
    end

    test "empty result keeps the schema", %{polygons: polygons} do
      far = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (500 500)"]))
      out = GeoFrame.sjoin(far, polygons)
      assert GeoDataFrame.n_rows(out) == 0
      assert GeoDataFrame.names(out) == ["id", "name", "value", "index_right", "geometry"]
    end

    test "null geometries never match but survive a left join", %{polygons: polygons} do
      left = GeoDataFrame.new(DataFrame.new(id: [1, 2]), s([nil, "POINT (1 1)"]))
      out = GeoFrame.sjoin(left, polygons, how: :left)
      assert col(out, "id") == [1, 2]
      assert col(out, "name") == [nil, "A"]
    end

    test "CRS mismatch raises", %{polygons: polygons} do
      a = GeoDataFrame.set_crs(polygons, "EPSG:4326")
      b = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (1 1)"], crs: "EPSG:3857"))
      assert_raise ArgumentError, ~r/CRS mismatch/, fn -> GeoFrame.sjoin(b, a) end
    end

    test "invalid options raise", %{polygons: polygons, points: points} do
      assert_raise ArgumentError, ~r/how/, fn -> GeoFrame.sjoin(points, polygons, how: :outer) end

      taken = GeoDataFrame.new(DataFrame.new(id: [1], index_right: [5]), s(["POINT (1 1)"]))
      assert_raise ArgumentError, ~r/index_right/, fn -> GeoFrame.sjoin(taken, polygons) end
    end
  end
end
