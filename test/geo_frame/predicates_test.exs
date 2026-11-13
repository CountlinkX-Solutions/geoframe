defmodule GeoFrame.PredicatesTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @square "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"
  @predicates ~w(intersects contains within touches crosses overlaps disjoint covers covered_by
                 geom_equals)a

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)

  describe "point against a polygon" do
    setup do
      %{points: s(["POINT (2 2)", "POINT (0 2)", "POINT (9 9)"])}
    end

    test "contains excludes the boundary, covers includes it", %{points: points} do
      squares = s(List.duplicate(@square, 3))
      assert list(GeoSeries.contains(squares, points)) == [true, false, false]
      assert list(GeoSeries.covers(squares, points)) == [true, true, false]
    end

    test "within excludes the boundary, covered_by includes it", %{points: points} do
      assert list(GeoSeries.within(points, @square)) == [true, false, false]
      assert list(GeoSeries.covered_by(points, @square)) == [true, true, false]
    end

    test "touches, intersects, disjoint", %{points: points} do
      assert list(GeoSeries.touches(points, @square)) == [false, true, false]
      assert list(GeoSeries.intersects(points, @square)) == [true, true, false]
      assert list(GeoSeries.disjoint(points, @square)) == [false, false, true]
    end
  end

  describe "polygon and line relations" do
    setup do
      a = s([@square, "LINESTRING (0 0, 4 4)", @square, @square])

      b =
        s([
          "POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))",
          "LINESTRING (0 4, 4 0)",
          "POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))",
          "POLYGON ((0 4, 4 4, 4 0, 0 0, 0 4))"
        ])

      %{a: a, b: b}
    end

    test "touches", %{a: a, b: b} do
      assert list(GeoSeries.touches(a, b)) == [true, false, false, false]
    end

    test "crosses", %{a: a, b: b} do
      assert list(GeoSeries.crosses(a, b)) == [false, true, false, false]
    end

    test "overlaps", %{a: a, b: b} do
      assert list(GeoSeries.overlaps(a, b)) == [false, false, true, false]
    end

    test "geom_equals ignores vertex order", %{a: a, b: b} do
      assert list(GeoSeries.geom_equals(a, b)) == [false, false, false, true]
    end

    test "intersects", %{a: a, b: b} do
      assert list(GeoSeries.intersects(a, b)) == [true, true, true, true]
    end
  end

  describe "distance, dwithin and relate" do
    test "distance is planar euclidean" do
      a = s(["POINT (0 0)", "POINT (2 2)", @square])
      b = s(["POINT (3 4)", @square, "POLYGON ((7 0, 9 0, 9 4, 7 4, 7 0))"])
      assert list(GeoSeries.distance(a, b)) == [5.0, 0.0, 3.0]
    end

    test "dwithin is inclusive and keeps nulls" do
      a = s(["POINT (0 0)", nil])
      b = s(["POINT (3 4)", "POINT (0 0)"])
      assert list(GeoSeries.dwithin(a, b, 5.0)) == [true, nil]
      assert list(GeoSeries.dwithin(a, b, 4.9)) == [false, nil]
    end

    test "relate returns DE-9IM strings, left geometry first" do
      a = s([@square, @square, "POINT (2 2)"])

      b =
        s([
          "POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))",
          "POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))",
          @square
        ])

      assert list(GeoSeries.relate(a, b)) == ["FF2F11212", "212101212", "0FFFFF212"]
      assert Series.dtype(GeoSeries.relate(a, b)) == :string
    end

    test "relate with a broadcast geometry keeps argument order" do
      points = s(["POINT (2 2)", "POINT (0 0)"])
      assert list(GeoSeries.relate(points, @square)) == ["0FFFFF212", "F0FFFF212"]
    end
  end

  describe "dtypes" do
    test "predicates are boolean and distance is f64" do
      a = s(["POINT (1 1)"])

      for name <- @predicates do
        assert Series.dtype(apply(GeoSeries, name, [a, a])) == :boolean, "#{name}"
      end

      assert Series.dtype(GeoSeries.distance(a, a)) == {:f, 64}
      assert Series.dtype(GeoSeries.dwithin(a, a, 1.0)) == :boolean
    end
  end

  describe "nulls and empties" do
    test "null on either side gives null" do
      a = s(["POINT (1 1)", nil, "POINT (1 1)"])
      b = s([@square, @square, nil])

      for name <- @predicates do
        assert [_, nil, nil] = list(apply(GeoSeries, name, [a, b])), "#{name}"
      end

      assert list(GeoSeries.distance(a, b)) == [0.0, nil, nil]
      assert [_, nil, nil] = list(GeoSeries.relate(a, b))
    end

    test "empty geometries give null, like the measures" do
      a = s(["POINT EMPTY", "POINT (1 1)"])
      assert list(GeoSeries.intersects(a, @square)) == [nil, true]
    end

    test "a null broadcast geometry gives all nulls" do
      a = s(["POINT (1 1)", "POINT (2 2)"])
      assert list(GeoSeries.intersects(a, s([nil]))) == [nil, nil]
    end
  end

  describe "broadcasting" do
    test "a WKT string and a 1-row series give the same answer as explicit pairing" do
      a = s(["POINT (2 2)", "POINT (0 2)", "POINT (9 9)", nil])
      explicit = s(List.duplicate(@square, 4))

      for name <- @predicates do
        expected = list(apply(GeoSeries, name, [a, explicit]))
        assert list(apply(GeoSeries, name, [a, @square])) == expected, "#{name} (wkt)"
        assert list(apply(GeoSeries, name, [a, s([@square])])) == expected, "#{name} (series)"
      end
    end

    test "a WKT string takes the CRS of the series" do
      a = s(["POINT (1 1)"], crs: "EPSG:4326")
      assert list(GeoSeries.intersects(a, @square)) == [true]
    end

    test "invalid WKT raises" do
      assert_raise ArgumentError, ~r/invalid WKT/, fn ->
        GeoSeries.intersects(s(["POINT (1 1)"]), "nope")
      end
    end
  end

  describe "argument checks" do
    test "length mismatch raises ArgumentError for every operation" do
      a = s(["POINT (1 1)", "POINT (2 2)"])
      b = s(["POINT (1 1)", "POINT (2 2)", "POINT (3 3)"])

      for name <- @predicates ++ [:distance, :relate] do
        assert_raise ArgumentError, ~r/length mismatch/, fn -> apply(GeoSeries, name, [a, b]) end
      end

      assert_raise ArgumentError, ~r/length mismatch/, fn -> GeoSeries.dwithin(a, b, 1.0) end
    end

    test "a 1-row left side does not broadcast against a longer right side" do
      a = s(["POINT (1 1)"])
      b = s(["POINT (1 1)", "POINT (2 2)"])
      assert_raise ArgumentError, ~r/length mismatch/, fn -> GeoSeries.intersects(a, b) end
    end

    test "CRS mismatch raises, unknown CRS is compatible" do
      a = s(["POINT (1 1)"], crs: "EPSG:4326")
      b = GeoSeries.set_crs(a, "EPSG:3857", allow_override: true)

      assert_raise ArgumentError, ~r/CRS mismatch/, fn -> GeoSeries.intersects(a, b) end
      assert_raise ArgumentError, ~r/CRS mismatch/, fn -> GeoSeries.distance(a, b) end
      assert list(GeoSeries.intersects(a, GeoSeries.set_crs(a, nil))) == [true]
      assert list(GeoSeries.intersects(a, a)) == [true]
    end

    test "dwithin requires a number" do
      a = s(["POINT (1 1)"])
      assert_raise FunctionClauseError, fn -> GeoSeries.dwithin(a, a, "far") end
    end
  end

  describe "properties" do
    defp coord, do: StreamData.integer(-20..20)

    defp geometry do
      point = StreamData.map({coord(), coord()}, fn {x, y} -> "POINT (#{x} #{y})" end)

      box =
        StreamData.map(
          {coord(), coord(), StreamData.integer(1..10), StreamData.integer(1..10)},
          fn
            {x, y, w, h} ->
              "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"
          end
        )

      line =
        StreamData.map({coord(), coord(), coord(), coord()}, fn {a, b, c, d} ->
          if {a, b} == {c, d}, do: "POINT (#{a} #{b})", else: "LINESTRING (#{a} #{b}, #{c} #{d})"
        end)

      StreamData.one_of([point, box, line])
    end

    defp pair_lists,
      do: StreamData.list_of({geometry(), geometry()}, min_length: 1, max_length: 8)

    defp split(pairs), do: {s(Enum.map(pairs, &elem(&1, 0))), s(Enum.map(pairs, &elem(&1, 1)))}

    property "within(a, b) == contains(b, a)" do
      check all(pairs <- pair_lists()) do
        {a, b} = split(pairs)
        assert list(GeoSeries.within(a, b)) == list(GeoSeries.contains(b, a))
        assert list(GeoSeries.covered_by(a, b)) == list(GeoSeries.covers(b, a))
      end
    end

    property "intersects is the negation of disjoint" do
      check all(pairs <- pair_lists()) do
        {a, b} = split(pairs)
        disjoint = a |> GeoSeries.disjoint(b) |> list() |> Enum.map(&(not &1))
        assert list(GeoSeries.intersects(a, b)) == disjoint
      end
    end

    property "predicates are symmetric where they should be" do
      check all(pairs <- pair_lists()) do
        {a, b} = split(pairs)

        for name <- ~w(intersects touches overlaps crosses geom_equals disjoint)a do
          assert list(apply(GeoSeries, name, [a, b])) == list(apply(GeoSeries, name, [b, a])),
                 "#{name}"
        end
      end
    end

    property "distance(a, a) == 0 and distance is symmetric and non-negative" do
      check all(pairs <- pair_lists()) do
        {a, b} = split(pairs)
        assert Enum.all?(list(GeoSeries.distance(a, a)), &(&1 == 0.0))
        d = list(GeoSeries.distance(a, b))
        assert d == list(GeoSeries.distance(b, a))
        assert Enum.all?(d, &(&1 >= 0.0))
      end
    end

    property "dwithin agrees with distance, and distance 0 means intersects" do
      check all(pairs <- pair_lists(), limit <- StreamData.integer(0..30)) do
        {a, b} = split(pairs)
        distances = list(GeoSeries.distance(a, b))
        assert list(GeoSeries.dwithin(a, b, limit * 1.0)) == Enum.map(distances, &(&1 <= limit))
        assert list(GeoSeries.dwithin(a, b, 0.0)) == list(GeoSeries.intersects(a, b))
      end
    end

    property "broadcasting equals explicit pairing" do
      check all(pairs <- pair_lists(), one <- geometry()) do
        {a, _} = split(pairs)
        explicit = s(List.duplicate(one, length(pairs)))

        for name <- @predicates do
          assert list(apply(GeoSeries, name, [a, one])) ==
                   list(apply(GeoSeries, name, [a, explicit])),
                 "#{name}"
        end

        assert list(GeoSeries.relate(a, one)) == list(GeoSeries.relate(a, explicit))
      end
    end
  end
end
