defmodule GeoFrame.SjoinNearestTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)

  defp square(x, y, size),
    do:
      "POLYGON ((#{x} #{y}, #{x + size} #{y}, #{x + size} #{y + size}, #{x} #{y + size}, #{x} #{y}))"

  defp col(gdf, name), do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  setup do
    shops =
      GeoDataFrame.new(
        DataFrame.new(shop: ["a", "b", "c"]),
        s(["POINT (0 0)", "POINT (10 0)", "POINT (0 10)"])
      )

    homes =
      GeoDataFrame.new(
        DataFrame.new(home: [1, 2, 3]),
        s(["POINT (1 0)", "POINT (9 1)", "POINT (100 90)"])
      )

    %{shops: shops, homes: homes}
  end

  describe "sindex_nearest/3" do
    test "returns parallel left, right and distance lists" do
      left = s(["POINT (1 0)", "POINT (9 1)"])
      right = s(["POINT (0 0)", "POINT (10 0)"])
      {li, ri, ds} = GeoSeries.sindex_nearest(left, right)
      assert {li, ri} == {[0, 1], [0, 1]}
      assert_in_delta Enum.at(ds, 0), 1.0, 1.0e-9
      assert_in_delta Enum.at(ds, 1), :math.sqrt(2), 1.0e-9
    end

    test "all ties, max_distance and exclusive" do
      left = s(["POINT (0 0)"])
      right = s(["POINT (1 0)", "POINT (-1 0)", "POINT (0 0)"])
      assert {[0], [2], [+0.0]} = GeoSeries.sindex_nearest(left, right)
      assert {[0, 0], [0, 1], [1.0, 1.0]} = GeoSeries.sindex_nearest(left, right, exclusive: true)
      assert {[], [], []} = GeoSeries.sindex_nearest(s(["POINT (50 0)"]), right, max_distance: 10)
    end

    test "bad options and CRS mismatch raise" do
      a = s(["POINT (0 0)"], crs: "EPSG:4326")
      b = s(["POINT (0 0)"], crs: "EPSG:3857")
      assert_raise ArgumentError, ~r/CRS mismatch/, fn -> GeoSeries.sindex_nearest(a, b) end

      assert_raise ArgumentError, ~r/max_distance/, fn ->
        GeoSeries.sindex_nearest(a, a, max_distance: -1)
      end
    end
  end

  describe "sjoin_nearest/3" do
    test "inner (default): nearest right row per left row, GeoPandas column order", ctx do
      out = GeoFrame.sjoin_nearest(ctx.homes, ctx.shops)
      assert GeoDataFrame.names(out) == ["home", "index_right", "shop", "geometry"]
      assert col(out, "home") == [1, 2, 3]
      assert col(out, "index_right") == [0, 1, 1]
      assert col(out, "shop") == ["a", "b", "b"]
      assert GeoSeries.to_wkt(out.geometry) |> hd() == "POINT(1 0)"
      assert GeoDataFrame.sjoin_nearest(ctx.homes, ctx.shops) |> col("home") == [1, 2, 3]
    end

    test "distance_col adds an f64 column before the geometry", ctx do
      out = GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, distance_col: "dist")
      assert GeoDataFrame.names(out) == ["home", "index_right", "shop", "dist", "geometry"]
      assert out.data |> DataFrame.pull("dist") |> Series.dtype() == {:f, 64}
      [d1, d2, _] = col(out, "dist")
      assert_in_delta d1, 1.0, 1.0e-9
      assert_in_delta d2, :math.sqrt(2), 1.0e-9
    end

    test "max_distance: inner drops, left keeps rows with nulls", ctx do
      inner = GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, max_distance: 5, distance_col: "d")
      assert col(inner, "home") == [1, 2]

      left =
        GeoFrame.sjoin_nearest(ctx.homes, ctx.shops,
          how: :left,
          max_distance: 5,
          distance_col: "d"
        )

      assert col(left, "home") == [1, 2, 3]
      assert col(left, "index_right") == [0, 1, nil]
      assert col(left, "shop") == ["a", "b", nil]
      assert List.last(col(left, "d")) == nil
    end

    test "ties return every equidistant match ordered by right index" do
      left = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (0 0)"]))

      right =
        GeoDataFrame.new(
          DataFrame.new(tag: ["e", "w", "far", "n"]),
          s(["POINT (1 0)", "POINT (-1 0)", "POINT (9 9)", "POINT (0 1)"])
        )

      out = GeoFrame.sjoin_nearest(left, right)
      assert col(out, "tag") == ["e", "w", "n"]
      assert col(out, "index_right") == [0, 1, 3]
      assert col(out, "id") == [1, 1, 1]
    end

    test "uses geometry distance: a point inside a polygon is at distance 0" do
      left = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (2 2)"]))

      right =
        GeoDataFrame.new(
          DataFrame.new(k: ["poly", "pt"]),
          s([square(0, 0, 4), "POINT (2.5 2)"])
        )

      out = GeoFrame.sjoin_nearest(left, right, distance_col: "d")
      assert col(out, "k") == ["poly"]
      assert col(out, "d") == [0.0]
    end

    test "exclusive: true ignores equal geometries (self join)" do
      pts =
        GeoDataFrame.new(
          DataFrame.new(id: [1, 2, 3]),
          s(["POINT (0 0)", "POINT (1 0)", "POINT (5 0)"])
        )

      out = GeoFrame.sjoin_nearest(pts, pts, exclusive: true, lsuffix: "a", rsuffix: "b")
      assert col(out, "id_a") == [1, 2, 3]
      assert col(out, "id_b") == [2, 1, 2]
      assert col(out, "index_right") == [1, 0, 1]
    end

    test "right join: nearest left per right row, geometry from the right", ctx do
      out = GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, how: :right, distance_col: "d")
      assert GeoDataFrame.names(out) == ["index_left", "home", "shop", "d", "geometry"]
      assert col(out, "shop") == ["a", "b", "c"]
      assert col(out, "index_left") == [0, 1, 0]
      assert col(out, "home") == [1, 2, 1]
      assert GeoSeries.to_wkt(out.geometry) == GeoSeries.to_wkt(ctx.shops.geometry)
    end

    test "null and empty left geometries only survive a left join", ctx do
      left = GeoDataFrame.new(DataFrame.new(id: [1, 2]), s([nil, "POINT (1 0)"]))
      assert GeoFrame.sjoin_nearest(left, ctx.shops) |> col("id") == [2]
      out = GeoFrame.sjoin_nearest(left, ctx.shops, how: :left)
      assert col(out, "id") == [1, 2]
      assert col(out, "index_right") == [nil, 0]
    end

    test "invalid options raise", ctx do
      assert_raise ArgumentError, ~r/how/, fn ->
        GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, how: :outer)
      end

      assert_raise ArgumentError, ~r/max_distance/, fn ->
        GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, max_distance: -1)
      end

      assert_raise ArgumentError, ~r/exclusive/, fn ->
        GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, exclusive: :yes)
      end

      assert_raise ArgumentError, ~r/two columns named "shop"/, fn ->
        GeoFrame.sjoin_nearest(ctx.homes, ctx.shops, distance_col: "shop")
      end
    end
  end

  describe "properties" do
    defp coord, do: StreamData.integer(-20..20)

    defp point_wkt,
      do: StreamData.map({coord(), coord()}, fn {x, y} -> "POINT (#{x} #{y})" end)

    defp rect_wkt do
      StreamData.map({coord(), coord(), StreamData.integer(1..6)}, fn {x, y, w} ->
        square(x, y, w)
      end)
    end

    defp distances_to(l, right) do
      for {r, j} <- Enum.with_index(right) do
        [d] = GeoSeries.distance(s([l]), s([r])) |> Series.to_list()
        {j, d}
      end
    end

    defp brute_force(left, right, max_distance) do
      left
      |> Enum.with_index()
      |> Enum.flat_map(fn {l, i} ->
        ds = distances_to(l, right)
        min = ds |> Enum.map(&elem(&1, 1)) |> Enum.min()
        within = is_nil(max_distance) or min <= max_distance
        for {j, d} <- ds, within, d == min, do: {i, j, d}
      end)
    end

    property "nearest equals brute force over all pairs (with all ties)" do
      check all(
              left <- StreamData.list_of(point_wkt(), min_length: 1, max_length: 15),
              right <-
                StreamData.list_of(StreamData.one_of([point_wkt(), rect_wkt()]),
                  min_length: 1,
                  max_length: 15
                ),
              max_distance <-
                StreamData.one_of([StreamData.constant(nil), StreamData.integer(0..15)]),
              max_runs: 60
            ) do
        opts = if max_distance, do: [max_distance: max_distance], else: []
        {li, ri, ds} = GeoSeries.sindex_nearest(s(left), s(right), opts)
        assert Enum.zip([li, ri, ds]) == brute_force(left, right, max_distance)
      end
    end
  end
end
