defmodule GeoFrame.SjoinLargeTest do
  use ExUnit.Case, async: true

  # Large joins move their pair indices as u32 binaries (no lists of millions of integers)
  # and lay out the rows in Rust.

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp col(gdf, name), do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name)

  test "take/2 accepts a binary of native-endian u32 rows" do
    s = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"])
    indices = <<2::unsigned-native-32, 0::unsigned-native-32, 2::unsigned-native-32>>

    assert s |> GeoSeries.take(indices) |> GeoSeries.to_wkt() ==
             s |> GeoSeries.take([2, 0, 2]) |> GeoSeries.to_wkt()

    assert_raise ArgumentError, ~r/out of bounds/, fn ->
      GeoSeries.take(s, <<3::unsigned-native-32>>)
    end

    assert_raise ArgumentError, ~r/multiple of 4/, fn -> GeoSeries.take(s, <<1, 2, 3>>) end
  end

  test "the pair binaries decode to the lists of sindex_query/4 and sindex_nearest/3" do
    left = GeoSeries.from_wkt!(["POINT (1 1)", "POINT (11 11)", "POINT (50 50)"])

    right =
      GeoSeries.from_wkt!([
        "POLYGON ((10 10, 12 10, 12 12, 10 12, 10 10))",
        "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"
      ])

    {lb, rb} = GeoSeries.sindex_query_binaries(left, right, :intersects, [])
    assert is_binary(lb) and is_binary(rb)
    assert GeoSeries.sindex_query(left, right) == {[0, 1], [1, 0]}
    assert lb == <<0::unsigned-native-32, 1::unsigned-native-32>>

    {_, _, db} = GeoSeries.sindex_nearest_binaries(left, right, [])
    assert {_, _, ds} = GeoSeries.sindex_nearest(left, right)
    assert Series.to_list(Series.from_binary(db, {:f, 64})) == ds
  end

  describe "a join with more than a million pairs" do
    # Squares anchored at the origin, 1..m wide: every pair intersects, m * m pairs.
    @m 1100

    setup do
      wkt = for i <- 1..@m, do: "POLYGON ((0 0, #{i} 0, #{i} #{i}, 0 #{i}, 0 0))"
      squares = GeoDataFrame.new(DataFrame.new(n: Enum.to_list(1..@m)), GeoSeries.from_wkt!(wkt))
      %{squares: squares}
    end

    test "inner keeps every pair ordered by left then right", %{squares: squares} do
      out = GeoFrame.sjoin(squares, squares)

      assert GeoDataFrame.n_rows(out) == @m * @m
      assert col(out, "n_left") |> Series.slice(0, 3) |> Series.to_list() == [1, 1, 1]
      assert col(out, "index_right") |> Series.slice(0, 3) |> Series.to_list() == [0, 1, 2]

      last = @m * @m - 3
      assert col(out, "n_left") |> Series.slice(last, 3) |> Series.to_list() == [@m, @m, @m]

      assert col(out, "index_right") |> Series.slice(last, 3) |> Series.to_list() ==
               [@m - 3, @m - 2, @m - 1]

      assert col(out, "index_right") |> Series.sum() == @m * div(@m * (@m - 1), 2)
      assert col(out, "index_right") |> Series.dtype() == {:s, 64}
      assert GeoSeries.size(GeoDataFrame.geometry(out)) == @m * @m
    end

    test "left adds the unmatched left rows once, right orders by right row", %{squares: squares} do
      far = GeoSeries.from_wkt!(["POINT (5000 5000)", "POINT (0.5 0.5)"])
      points = GeoDataFrame.new(DataFrame.new(id: [1, 2]), far)

      left = GeoFrame.sjoin(points, squares, how: :left)
      assert GeoDataFrame.n_rows(left) == 1 + @m
      assert col(left, "id") |> Series.slice(0, 2) |> Series.to_list() == [1, 2]
      assert col(left, "index_right") |> Series.slice(0, 2) |> Series.to_list() == [nil, 0]
      assert col(left, "n") |> Series.slice(0, 2) |> Series.to_list() == [nil, 1]

      right = GeoFrame.sjoin(points, squares, how: :right)
      assert GeoDataFrame.n_rows(right) == @m
      assert col(right, "index_left") |> Series.slice(0, 2) |> Series.to_list() == [1, 1]
      assert col(right, "n") |> Series.slice(@m - 2, 2) |> Series.to_list() == [@m - 1, @m]
    end
  end
end
