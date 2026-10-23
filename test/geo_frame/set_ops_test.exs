defmodule GeoFrame.SetOpsTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @a "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"
  @b "POLYGON ((0.5 0, 1.5 0, 1.5 1, 0.5 1, 0.5 0))"
  @far "POLYGON ((5 5, 6 5, 6 6, 5 6, 5 5))"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)
  defp areas(%GeoSeries{} = series), do: series |> GeoSeries.area() |> list()

  describe "reference cases (two unit squares offset by 0.5)" do
    setup do
      %{a: s([@a]), b: s([@b])}
    end

    test "intersection, union, difference, symmetric_difference areas", %{a: a, b: b} do
      assert [0.5] = a |> GeoSeries.intersection(b) |> areas() |> Enum.map(&Float.round(&1, 9))
      assert [1.5] = a |> GeoSeries.union(b) |> areas() |> Enum.map(&Float.round(&1, 9))
      assert [0.5] = a |> GeoSeries.difference(b) |> areas() |> Enum.map(&Float.round(&1, 9))

      assert [1.0] =
               a |> GeoSeries.symmetric_difference(b) |> areas() |> Enum.map(&Float.round(&1, 9))
    end

    test "disjoint intersection is an empty polygon, not null", %{a: a} do
      out = GeoSeries.intersection(a, @far)
      assert list(GeoSeries.is_empty(out)) == [true]
      assert list(GeoSeries.geom_type(out)) == ["Polygon"]
    end

    test "squares sharing only an edge intersect in that edge" do
      out = GeoSeries.intersection(s([@a]), "POLYGON ((1 0, 2 0, 2 1, 1 1, 1 0))")
      assert list(GeoSeries.geom_type(out)) == ["LineString"]
      assert list(GeoSeries.length(out)) == [1.0]
    end

    test "accepts a WKT scalar and broadcasts", %{a: a} do
      many = s([@a, @far])
      out = GeoSeries.intersection(many, @b)
      assert [0.5, +0.0] = areas(out)
      assert [0.5] = a |> GeoSeries.intersection(@b) |> areas()
    end
  end

  describe "other geometry types" do
    test "line clipped by a square" do
      line = s(["LINESTRING (-1 0.5, 2 0.5)"])
      assert [1.0] = line |> GeoSeries.intersection(@a) |> GeoSeries.length() |> list()
      assert [2.0] = line |> GeoSeries.difference(@a) |> GeoSeries.length() |> list()
    end

    test "points are kept or dropped" do
      pts = s(["POINT (0.5 0.5)", "POINT (9 9)"])
      assert list(pts |> GeoSeries.intersection(@a) |> GeoSeries.is_empty()) == [false, true]
      assert list(pts |> GeoSeries.difference(@a) |> GeoSeries.is_empty()) == [true, false]
    end

    test "a line union a polygon is a collection" do
      out = s(["LINESTRING (0 0, 5 5)"]) |> GeoSeries.union(@a)
      assert list(GeoSeries.geom_type(out)) == ["GeometryCollection"]
    end

    test "geometry collections are unsupported and give null" do
      gc = s(["GEOMETRYCOLLECTION (POINT (0 0), POINT (1 1))"])
      assert [nil] = gc |> GeoSeries.union(@a) |> GeoSeries.to_wkt()
    end
  end

  describe "alignment, nulls and CRS" do
    test "nulls propagate and the CRS is kept" do
      out = GeoSeries.union(s([@a, nil], crs: "EPSG:3857"), s([@b, @b]))
      assert GeoSeries.crs(out) == "EPSG:3857"
      assert [_, nil] = GeoSeries.to_wkt(out)
    end

    test "length mismatch and CRS mismatch raise" do
      assert_raise ArgumentError, ~r/length mismatch/, fn ->
        GeoSeries.union(s([@a, @a]), s([@a, @a, @a]))
      end

      assert_raise ArgumentError, ~r/CRS mismatch/, fn ->
        GeoSeries.union(s([@a], crs: "EPSG:4326"), s([@a], crs: "EPSG:3857"))
      end
    end
  end

  describe "union_all" do
    test "returns a one-row series covering all polygons, skipping nulls" do
      out = GeoSeries.union_all(s([@a, @b, nil, @far], crs: "EPSG:4326"))
      assert GeoSeries.crs(out) == "EPSG:4326"
      assert [2.5] = out |> areas() |> Enum.map(&Float.round(&1, 9))
    end

    test "points are deduplicated into a multipoint" do
      out = GeoSeries.union_all(s(["POINT (1 1)", "POINT (1 1)", "POINT (2 2)"]))
      assert list(GeoSeries.count_coordinates(out)) == [2]
    end

    test "empty input gives an empty geometry collection" do
      assert GeoSeries.union_all(s([nil])) |> GeoSeries.to_wkt() == ["GEOMETRYCOLLECTION EMPTY"]
    end
  end

  describe "properties over axis-aligned rectangles" do
    defp rect do
      StreamData.map(
        {StreamData.integer(-10..10), StreamData.integer(-10..10), StreamData.integer(1..8),
         StreamData.integer(1..8)},
        fn {x, y, w, h} ->
          "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"
        end
      )
    end

    defp pairs, do: StreamData.list_of({rect(), rect()}, min_length: 1, max_length: 6)

    defp split(pairs), do: {s(Enum.map(pairs, &elem(&1, 0))), s(Enum.map(pairs, &elem(&1, 1)))}

    defp close?(xs, ys), do: Enum.all?(Enum.zip(xs, ys), fn {x, y} -> abs(x - y) < 1.0e-9 end)

    property "inclusion-exclusion: area(a u b) = area(a) + area(b) - area(a n b)" do
      check all(ps <- pairs()) do
        {a, b} = split(ps)
        union = a |> GeoSeries.union(b) |> areas()
        inter = a |> GeoSeries.intersection(b) |> areas()
        expected = Enum.zip_with([areas(a), areas(b), inter], fn [x, y, i] -> x + y - i end)
        assert close?(union, expected)
      end
    end

    property "area(a - b) = area(a) - area(a n b)" do
      check all(ps <- pairs()) do
        {a, b} = split(ps)
        diff = a |> GeoSeries.difference(b) |> areas()
        inter = a |> GeoSeries.intersection(b) |> areas()
        assert close?(diff, Enum.zip_with(areas(a), inter, &(&1 - &2)))
      end
    end

    property "symmetric difference equals (a u b) - (a n b) by area" do
      check all(ps <- pairs()) do
        {a, b} = split(ps)
        sym = a |> GeoSeries.symmetric_difference(b) |> areas()
        union = GeoSeries.union(a, b)
        inter = GeoSeries.intersection(a, b)
        assert close?(sym, union |> GeoSeries.difference(inter) |> areas())
      end
    end

    property "the intersection is covered by both inputs (when non-empty)" do
      check all(ps <- pairs()) do
        {a, b} = split(ps)
        inter = GeoSeries.intersection(a, b)
        empty = list(GeoSeries.is_empty(inter))
        covered_a = list(GeoSeries.covered_by(inter, a))
        covered_b = list(GeoSeries.covered_by(inter, b))

        assert Enum.all?(Enum.zip([empty, covered_a, covered_b]), fn {e, ca, cb} ->
                 e or (ca and cb)
               end)
      end
    end

    property "union_all area is at least the largest input area" do
      check all(rects <- StreamData.list_of(rect(), min_length: 1, max_length: 8)) do
        series = s(rects)
        [total] = series |> GeoSeries.union_all() |> areas()
        assert total >= Enum.max(areas(series)) - 1.0e-9
      end
    end
  end
end
