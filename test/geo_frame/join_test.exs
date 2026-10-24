defmodule GeoFrame.JoinTest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  # Mirrors the GeoPandas reference (`gdf.merge(df, on="k", how=...)`):
  # left keys [1, 2, 2, 3] with POINT (i i) for row i; right keys [2, 3, 4, 2].
  defp left(opts \\ []) do
    data = DataFrame.new(k: [1, 2, 2, 3], a: ["w", "x", "y", "z"])
    geometry = GeoSeries.from_wkt!(Enum.map(0..3, &"POINT (#{&1} #{&1})"), opts)
    GeoDataFrame.new(data, geometry, Keyword.take(opts, [:geometry_column]))
  end

  defp right, do: DataFrame.new(k: [2, 3, 4, 2], b: ["p", "q", "r", "s"])

  # Row tuples {k, a, b, wkt}, sorted: Polars and pandas order unmatched rows differently,
  # but every row must carry the right geometry.
  defp rows(gdf) do
    data = GeoDataFrame.data(gdf)
    cols = for n <- ["k", "a", "b"], do: data |> DataFrame.pull(n) |> Series.to_list()
    wkt = gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()
    cols |> Kernel.++([wkt]) |> Enum.zip() |> Enum.sort()
  end

  defp p(i), do: "POINT(#{i} #{i})"

  test "inner matches GeoPandas and is the default" do
    expected = [
      {2, "x", "p", p(1)},
      {2, "x", "s", p(1)},
      {2, "y", "p", p(2)},
      {2, "y", "s", p(2)},
      {3, "z", "q", p(3)}
    ]

    assert rows(GeoDataFrame.join(left(), right(), on: "k")) == expected
    assert rows(GeoDataFrame.join(left(), right(), on: "k", how: :inner)) == expected
  end

  test "left keeps left order and repeats geometry for duplicated keys" do
    out = GeoDataFrame.join(left(), right(), on: "k", how: :left)

    assert out |> GeoDataFrame.geometry() |> GeoSeries.to_wkt() ==
             [p(0), p(1), p(1), p(2), p(2), p(3)]

    assert rows(out) == [
             {1, "w", nil, p(0)},
             {2, "x", "p", p(1)},
             {2, "x", "s", p(1)},
             {2, "y", "p", p(2)},
             {2, "y", "s", p(2)},
             {3, "z", "q", p(3)}
           ]
  end

  test "right gives null geometry for unmatched right rows" do
    out = GeoDataFrame.join(left(), right(), on: "k", how: :right)
    assert GeoDataFrame.n_rows(out) == 6
    assert length(GeoDataFrame.geometry(out) |> GeoSeries.to_wkt()) == 6
    assert Enum.count(rows(out), &(elem(&1, 3) == nil)) == 1
    assert {4, nil, "r", nil} in rows(out)

    matched = Enum.reject(rows(out), &(elem(&1, 3) == nil))

    assert matched == [
             {2, "x", "p", p(1)},
             {2, "x", "s", p(1)},
             {2, "y", "p", p(2)},
             {2, "y", "s", p(2)},
             {3, "z", "q", p(3)}
           ]
  end

  test "outer keeps unmatched on both sides, null geometry only for right-only rows" do
    out = GeoDataFrame.join(left(), right(), on: "k", how: :outer)
    wkt = GeoDataFrame.geometry(out) |> GeoSeries.to_wkt()
    assert length(wkt) == 7
    assert Enum.count(wkt, &is_nil/1) == 1

    by_b =
      out
      |> GeoDataFrame.data()
      |> DataFrame.pull("b")
      |> Series.to_list()
      |> Enum.zip(wkt)

    assert {nil, p(0)} in by_b
    assert {"r", nil} in by_b
    assert {"q", p(3)} in by_b
  end

  test "a GeoDataFrame on the right contributes attributes only" do
    r = GeoDataFrame.new(right(), GeoSeries.from_wkt!(Enum.map(0..3, &"POINT (9 #{&1})")))
    out = GeoDataFrame.join(left(), r, on: "k")
    assert rows(out) == rows(GeoDataFrame.join(left(), right(), on: "k"))
    assert out.geometry_column == left().geometry_column
  end

  test "CRS and geometry column name are preserved" do
    out = GeoDataFrame.join(left(crs: "EPSG:4326"), right(), on: "k", how: :outer)
    assert GeoDataFrame.crs(out) == "EPSG:4326"
    assert out.geometry_column == left().geometry_column
  end

  test "cross join pairs every row" do
    out = GeoDataFrame.join(left(), right(), how: :cross)
    assert GeoDataFrame.n_rows(out) == 16
    wkt = GeoDataFrame.geometry(out) |> GeoSeries.to_wkt()
    ks = out |> GeoDataFrame.data() |> DataFrame.pull("k") |> Series.to_list()
    expected = %{1 => [p(0)], 3 => [p(3)], 2 => [p(1), p(2)]}

    for {k, w} <- Enum.zip(ks, wkt), do: assert(w in expected[k])

    assert Enum.frequencies(wkt) |> Map.values() |> Enum.all?(&(&1 == 4))
  end

  test "cross join works for a frame without attribute columns" do
    geometry = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"])
    gdf = GeoDataFrame.new(DataFrame.new([]), geometry)
    out = GeoDataFrame.join(gdf, DataFrame.new(b: [1, 2, 3]), how: :cross)
    assert GeoDataFrame.n_rows(out) == 6

    assert out |> GeoDataFrame.geometry() |> GeoSeries.to_wkt() |> Enum.frequencies() ==
             %{"POINT(0 0)" => 3, "POINT(1 1)" => 3}
  end

  test "a keyed join on a frame without attribute columns raises" do
    gdf = GeoDataFrame.new(DataFrame.new([]), GeoSeries.from_wkt!(["POINT (0 0)"]))
    assert_raise ArgumentError, fn -> GeoDataFrame.join(gdf, right(), on: "k") end
  end

  test "a right column named like the geometry column raises" do
    geo = left().geometry_column
    r = DataFrame.new([{"k", [2]}, {geo, [1]}])

    assert_raise ArgumentError, ~r/reserved for the geometry/, fn ->
      GeoDataFrame.join(left(), r, on: "k")
    end
  end

  test "joining on the geometry column raises" do
    geo = left().geometry_column
    r = DataFrame.new([{geo, [1]}])
    assert_raise ArgumentError, ~r/geometry/, fn -> GeoDataFrame.join(left(), r, on: geo) end
  end

  test "the module-level GeoFrame.merge delegates" do
    assert rows(GeoFrame.merge(left(), right(), on: "k")) ==
             rows(GeoDataFrame.join(left(), right(), on: "k"))
  end
end
