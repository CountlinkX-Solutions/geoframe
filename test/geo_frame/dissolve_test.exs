defmodule GeoFrame.DissolveTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp square(x, y, size \\ 1),
    do:
      "POLYGON ((#{x} #{y}, #{x + size} #{y}, #{x + size} #{y + size}, #{x} #{y + size}, #{x} #{y}))"

  # Rows 0,1 are adjacent squares in group "b"; row 2 is group "a"; row 3 is group "b".
  setup do
    data =
      DataFrame.new(
        key: ["b", "b", "a", "b"],
        zone: [1, 1, 2, 2],
        population: [10, 20, 5, 7],
        name: ["p", "q", "r", "s"]
      )

    geometry =
      GeoSeries.from_wkt!([square(0, 0), square(1, 0), square(10, 10), square(2, 0)],
        crs: "EPSG:3857"
      )

    %{gdf: GeoDataFrame.new(data, geometry)}
  end

  defp col(gdf, name), do: gdf.data |> DataFrame.pull(name) |> Series.to_list()
  defp areas(gdf), do: gdf |> GeoDataFrame.area() |> Series.to_list()

  test "dissolve by one key sorts keys, keeps them as leading columns and unions geometry", %{
    gdf: gdf
  } do
    out = GeoDataFrame.dissolve(gdf, by: "key")
    assert GeoDataFrame.names(out) == ["key", "zone", "population", "name", "geometry"]
    assert col(out, "key") == ["a", "b"]
    assert col(out, "population") == [5, 10]
    assert col(out, "name") == ["r", "p"]
    assert areas(out) == [1.0, 3.0]
    assert GeoDataFrame.crs(out) == "EPSG:3857"
  end

  test "atom by and list of keys", %{gdf: gdf} do
    out = GeoDataFrame.dissolve(gdf, by: [:key, :zone])
    assert col(out, "key") == ["a", "b", "b"]
    assert col(out, "zone") == [2, 1, 2]
    assert areas(out) == [1.0, 2.0, 1.0]
    assert GeoDataFrame.dissolve(gdf, by: :key) |> GeoDataFrame.n_rows() == 2
  end

  test "no key dissolves everything into one row", %{gdf: gdf} do
    out = GeoDataFrame.dissolve(gdf)
    assert GeoDataFrame.n_rows(out) == 1
    assert GeoDataFrame.names(out) == ["key", "zone", "population", "name", "geometry"]
    assert col(out, "key") == ["b"]
    assert areas(out) == [4.0]
    assert GeoDataFrame.crs(out) == "EPSG:3857"
  end

  test "sort: false keeps first-appearance order", %{gdf: gdf} do
    out = GeoDataFrame.dissolve(gdf, by: "key", sort: false)
    assert col(out, "key") == ["b", "a"]
    assert areas(out) == [3.0, 1.0]
  end

  test "aggfunc atoms", %{gdf: gdf} do
    numeric = GeoDataFrame.select(gdf, ["key", "population"])
    run = fn f -> numeric |> GeoDataFrame.dissolve(by: "key", aggfunc: f) |> col("population") end
    assert run.(:first) == [5, 10]
    assert run.(:last) == [5, 7]
    assert run.(:sum) == [5, 37]
    assert run.(:min) == [5, 7]
    assert run.(:max) == [5, 20]
    assert run.(:count) == [1, 3]
    assert run.(:mean) == [5.0, 37 / 3]
  end

  test "per-column aggfunc keeps only the listed columns", %{gdf: gdf} do
    for {spec, order} <- [
          {[population: :sum, name: :last], ["population", "name"]},
          # map iteration order is by key
          {%{"population" => :sum, "name" => :last}, ["name", "population"]}
        ] do
      out = GeoDataFrame.dissolve(gdf, by: "key", aggfunc: spec)
      assert GeoDataFrame.names(out) == ["key"] ++ order ++ ["geometry"]
      assert col(out, "population") == [5, 37]
      assert col(out, "name") == ["r", "s"]
    end
  end

  test "first and last skip nulls per group, like pandas" do
    gdf =
      GeoDataFrame.new(
        DataFrame.new(
          key: ["a", "a", "a", "b", "b", "c"],
          v: [nil, 2, 3, nil, nil, 1],
          w: [1, nil, nil, nil, nil, nil]
        ),
        GeoSeries.from_wkt!(Enum.map(0..5, &square(&1 * 3, 0)))
      )

    first = GeoDataFrame.dissolve(gdf, by: "key", aggfunc: :first)
    assert col(first, "v") == [2, nil, 1]
    assert col(first, "w") == [1, nil, nil]

    last = GeoDataFrame.dissolve(gdf, by: "key", aggfunc: :last)
    assert col(last, "v") == [3, nil, 1]
    assert col(last, "w") == [1, nil, nil]
  end

  test "dropna true drops null keys, false keeps a null group" do
    gdf =
      GeoDataFrame.new(
        DataFrame.new(key: ["a", nil, "a", nil], v: [1, 2, 3, 4]),
        GeoSeries.from_wkt!([square(0, 0), square(5, 5), square(1, 0), square(6, 5)])
      )

    dropped = GeoDataFrame.dissolve(gdf, by: "key")
    assert col(dropped, "key") == ["a"]
    assert areas(dropped) == [2.0]

    kept = GeoDataFrame.dissolve(gdf, by: "key", dropna: false)
    assert col(kept, "key") == ["a", nil]
    assert col(kept, "v") == [1, 2]
    assert areas(kept) == [2.0, 2.0]
  end

  test "null geometries are skipped in the union", %{gdf: _} do
    gdf =
      GeoDataFrame.new(
        DataFrame.new(key: ["a", "a"]),
        GeoSeries.from_wkt!([square(0, 0), nil])
      )

    assert gdf |> GeoDataFrame.dissolve(by: "key") |> areas() == [1.0]
  end

  test "errors on bad input", %{gdf: gdf} do
    assert_raise ArgumentError, ~r/not found/, fn -> GeoDataFrame.dissolve(gdf, by: "nope") end
    assert_raise ArgumentError, ~r/geometry/, fn -> GeoDataFrame.dissolve(gdf, by: "geometry") end

    assert_raise ArgumentError, ~r/aggfunc/, fn ->
      GeoDataFrame.dissolve(gdf, by: "key", aggfunc: :median)
    end

    assert_raise ArgumentError, ~r/not found/, fn ->
      GeoDataFrame.dissolve(gdf, by: "key", aggfunc: [nope: :sum])
    end
  end

  test "empty frame dissolves to an empty frame" do
    gdf = GeoDataFrame.new(DataFrame.new(key: [], v: []), GeoSeries.from_wkt!([]))
    assert gdf |> GeoDataFrame.dissolve(by: "key") |> GeoDataFrame.n_rows() == 0
  end

  test "GeoFrame.dissolve/2 delegates", %{gdf: gdf} do
    assert GeoFrame.dissolve(gdf, by: "key") |> GeoDataFrame.n_rows() == 2
  end

  property "group area equals union_all area of the group; rows equal distinct keys" do
    rect = {integer(0..20), integer(0..20), integer(1..5), integer(1..5)}
    row = {rect, member_of(["a", "b", "c", nil])}

    check all(rows <- list_of(row, min_length: 1, max_length: 12), max_runs: 40) do
      wkts = for {{x, y, w, h}, _} <- rows, do: rect_wkt(x, y, w, h)
      keys = for {_, k} <- rows, do: k

      gdf =
        GeoDataFrame.new(
          DataFrame.new(key: keys, i: Enum.to_list(0..(length(rows) - 1))),
          GeoSeries.from_wkt!(wkts)
        )

      out = GeoDataFrame.dissolve(gdf, by: "key")
      distinct = keys |> Enum.reject(&is_nil/1) |> Enum.uniq() |> Enum.sort()
      assert col(out, "key") == distinct

      for {key, area} <- Enum.zip(col(out, "key"), areas(out)) do
        members = for {w, ^key} <- Enum.zip(wkts, keys), do: w

        [expected] =
          members
          |> GeoSeries.from_wkt!()
          |> GeoSeries.union_all()
          |> GeoSeries.area()
          |> Series.to_list()

        assert_in_delta area, expected, 1.0e-9
      end
    end
  end

  defp rect_wkt(x, y, w, h),
    do: "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"

  describe "as_index, level, observed" do
    # GeoPandas 1.2 / pandas 3.1:
    #   g.dissolve("k", observed=True)           -> only the observed keys
    #   g.dissolve("k", as_index=False).columns -> k, geometry, v (k is a column)
    #   g = gpd.GeoDataFrame({"k": pd.Categorical(["b","a"], categories=list("ab")),
    #                         "j": pd.Categorical(["a","b"], categories=list("ab")),
    #                         "v": [1, 2]}, geometry=[Point(0,0), Point(1,1)])
    #   g.dissolve(["k","j"], observed=False, as_index=False[, sort=False][, aggfunc=...]):
    #     sort=True:  aa, ab 2, ba 1, bb (first: NaN; sum: 0)
    #     sort=False: ba 1, bb, aa, ab 2
    # Explorer 0.13 categoricals share one process-wide category mapping, so a column has no
    # categories of its own beyond its values: each key's levels are its observed values, and
    # observed: false adds the unobserved combinations of several keys.
    defp categorical do
      data =
        DataFrame.new(
          k: Series.from_list(["a", "a", "c", "b", "d"], dtype: :category),
          v: Series.from_list([1, 2, 3, 4, 5])
        )

      geometry =
        GeoSeries.from_wkt!(
          ["POINT (0 0)", "POINT (1 1)", "POINT (2 2)", "POINT (3 3)", "POINT (4 4)"],
          crs: "EPSG:4326"
        )

      GeoDataFrame.take(GeoDataFrame.new(data, geometry), [0, 1, 2])
    end

    defp wkts(gdf),
      do:
        gdf.geometry
        |> GeoSeries.to_wkt()
        |> Enum.map(&String.replace(&1, " ", ""))

    defp same?(a, b),
      do: wkts(a) == wkts(b) and DataFrame.to_columns(a.data) == DataFrame.to_columns(b.data)

    test "as_index: false is accepted and equals the default", %{gdf: gdf} do
      assert same?(
               GeoDataFrame.dissolve(gdf, by: "key", as_index: false),
               GeoDataFrame.dissolve(gdf, by: "key")
             )
    end

    test "as_index: true raises, there is no index", %{gdf: gdf} do
      assert_raise ArgumentError, ~r/as_index: true.*no index/s, fn ->
        GeoDataFrame.dissolve(gdf, by: "key", as_index: true)
      end

      assert_raise ArgumentError, ~r/as_index/, fn ->
        GeoDataFrame.dissolve(gdf, by: "key", as_index: :yes)
      end
    end

    test "level raises, there is no index", %{gdf: gdf} do
      assert_raise ArgumentError, ~r/level.*no index/s, fn ->
        GeoDataFrame.dissolve(gdf, level: 0)
      end
    end

    test "observed defaults to true: unobserved categories are dropped" do
      out = GeoDataFrame.dissolve(categorical(), by: "k")
      assert col(out, "k") == ["a", "c"]
      assert same?(GeoDataFrame.dissolve(categorical(), by: "k", observed: true), out)
    end

    test "observed: false with one key adds no group: a column has no unused categories" do
      # categorical() drops the rows of "b" and "d"; with Explorer's shared category mapping
      # they are not categories of this column any more than any other string.
      out = GeoDataFrame.dissolve(categorical(), by: "k", observed: false)
      assert col(out, "k") == ["a", "c"]
      assert same?(out, GeoDataFrame.dissolve(categorical(), by: "k"))
    end

    defp two_keys do
      data =
        DataFrame.new(
          k: Series.from_list(["b", "a"], dtype: :category),
          j: Series.from_list(["a", "b"], dtype: :category),
          v: Series.from_list([1, 2])
        )

      geometry = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"], crs: "EPSG:4326")
      GeoDataFrame.new(data, geometry)
    end

    test "observed: false counts and sums empty groups as 0" do
      out = GeoDataFrame.dissolve(two_keys(), by: ["k", "j"], aggfunc: :sum, observed: false)
      assert col(out, "v") == [0, 2, 1, 0]
      out = GeoDataFrame.dissolve(two_keys(), by: ["k", "j"], aggfunc: :count, observed: false)
      assert col(out, "v") == [0, 1, 1, 0]
    end

    test "observed: false uses the key's own values, not categories of other series" do
      # Explorer 0.13 categoricals share one process-wide category mapping (Polars), so
      # `Series.categories/1` also returns strings categorized anywhere else in the session.
      _elsewhere = Series.from_list(["zz-from-another-series"], dtype: :category)

      data =
        DataFrame.new(
          k: Series.from_list(["b", "a"], dtype: :category),
          v: Series.from_list([1, 2])
        )

      gdf = GeoDataFrame.new(data, GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"]))
      out = GeoDataFrame.dissolve(gdf, by: "k", observed: false)
      assert col(out, "k") == ["a", "b"]
    end

    test "observed: false is a no-op without categorical keys", %{gdf: gdf} do
      assert same?(
               GeoDataFrame.dissolve(gdf, by: "key", observed: false),
               GeoDataFrame.dissolve(gdf, by: "key")
             )
    end

    test "observed: false with mixed categorical and plain keys raises" do
      gdf = categorical()
      gdf = %{gdf | data: DataFrame.put(gdf.data, "z", Series.from_list([1, 1, 2]))}

      assert_raise ArgumentError, ~r/observed: false/, fn ->
        GeoDataFrame.dissolve(gdf, by: ["k", "z"], observed: false)
      end
    end

    test "observed: false takes every combination of several category keys" do
      out = GeoDataFrame.dissolve(two_keys(), by: ["k", "j"], observed: false)

      assert Enum.zip(col(out, "k"), col(out, "j")) == [
               {"a", "a"},
               {"a", "b"},
               {"b", "a"},
               {"b", "b"}
             ]

      assert col(out, "v") == [nil, 2, 1, nil]

      assert wkts(out) == [
               "GEOMETRYCOLLECTIONEMPTY",
               "POINT(11)",
               "POINT(00)",
               "GEOMETRYCOLLECTIONEMPTY"
             ]

      assert GeoDataFrame.crs(out) == "EPSG:4326"

      out = GeoDataFrame.dissolve(two_keys(), by: ["k", "j"], observed: false, sort: false)

      assert Enum.zip(col(out, "k"), col(out, "j")) == [
               {"b", "a"},
               {"b", "b"},
               {"a", "a"},
               {"a", "b"}
             ]

      assert col(out, "v") == [1, nil, nil, 2]
    end
  end
end
