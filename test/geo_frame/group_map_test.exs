defmodule GeoFrame.GroupMapTest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  # Expected values come from GeoPandas 1.2 / pandas 3 (pandas 3 passes the group frame
  # *without* the key columns and puts the keys in the index; here the frame keeps its key
  # columns and the keys are columns, so only the values are compared):
  #
  #   df = pd.DataFrame(dict(key=["b","b","a","b","a",None], zone=[1,1,2,2,2,3],
  #        pop=[10,20,5,None,7,1], name=["p","q",None,"s","t","u"]))
  #   g = gpd.GeoDataFrame(df, geometry=gpd.points_from_xy(range(6), range(6)), crs=3857)
  #   g.groupby("key").apply(lambda d: d.nlargest(1, "pop"))
  #     -> (a, row 4, 7.0), (b, row 1, 20.0), GeoDataFrame, EPSG:3857
  #   g.groupby("key", dropna=False).apply(lambda d: d.head(1)) -> rows 2 (a), 0 (b), 5 (nil)
  #   g.groupby("key").head(1).index -> [0, 2]; .tail(1).index -> [3, 4]   (original row order)
  #   g.groupby(["key","zone"])["name"].nunique() -> (a,2): 1, (b,1): 2, (b,2): 1
  setup do
    data =
      DataFrame.new(
        key: ["b", "b", "a", "b", "a", nil],
        zone: [1, 1, 2, 2, 2, 3],
        pop: [10, 20, 5, nil, 7, 1],
        name: ["p", "q", nil, "s", "t", "u"]
      )

    geometry =
      GeoSeries.from_xy(Enum.map(0..5, &(&1 * 1.0)), Enum.map(0..5, &(&1 * 1.0)),
        crs: "EPSG:3857"
      )

    %{gdf: GeoDataFrame.new(data, geometry)}
  end

  defp col(gdf, name), do: gdf.data |> DataFrame.pull(name) |> Series.to_list()

  defp max_pop(g),
    do: GeoDataFrame.filter_with(g, fn d -> Series.equal(d["pop"], Series.max(d["pop"])) end)

  describe "group_map/4" do
    test "applies the function per group in key order and concatenates (pandas apply)", %{
      gdf: gdf
    } do
      out = GeoDataFrame.group_map(gdf, "key", &max_pop/1)

      assert %GeoDataFrame{} = out
      assert col(out, "key") == ["a", "b"]
      assert col(out, "pop") == [7, 20]
      assert col(out, "name") == ["t", "q"]

      assert GeoSeries.to_wkt(out.geometry) ==
               GeoSeries.to_wkt(GeoSeries.take(gdf.geometry, [4, 1]))

      assert GeoSeries.crs(out.geometry) == GeoSeries.crs(gdf.geometry)
    end

    test "equals concatenating the function over filter_with per key", %{gdf: gdf} do
      expected =
        ["a", "b"]
        |> Enum.map(fn k ->
          gdf |> GeoDataFrame.filter_with(fn d -> Series.equal(d["key"], k) end) |> max_pop()
        end)
        |> GeoDataFrame.concat_rows()

      out = GeoDataFrame.group_map(gdf, "key", &max_pop/1)
      assert DataFrame.to_columns(out.data) == DataFrame.to_columns(expected.data)
      assert GeoSeries.to_wkt(out.geometry) == GeoSeries.to_wkt(expected.geometry)
      assert GeoDataFrame.crs(out) == GeoDataFrame.crs(expected)
    end

    test "dropna: false keeps the null group last, sort: false uses first appearance", %{gdf: gdf} do
      out = GeoDataFrame.group_map(gdf, "key", &GeoDataFrame.head(&1, 1), dropna: false)
      assert col(out, "name") == [nil, "p", "u"]
      assert col(out, "key") == ["a", "b", nil]

      out = GeoDataFrame.group_map(gdf, "key", &GeoDataFrame.head(&1, 1), sort: false)
      assert col(out, "key") == ["b", "a"]
    end

    test "several keys and a grouped input frame (groups are ignored)", %{gdf: gdf} do
      out =
        gdf
        |> GeoDataFrame.group_by("zone")
        |> GeoDataFrame.group_map(["key", "zone"], &GeoDataFrame.head(&1, 1))

      assert GeoDataFrame.groups(out) == []
      assert col(out, "key") == ["a", "b", "b"]
      assert col(out, "zone") == [2, 1, 2]
    end

    test "key columns the function dropped are prepended", %{gdf: gdf} do
      out =
        GeoDataFrame.group_map(gdf, ["key", "zone"], fn g ->
          GeoDataFrame.new(DataFrame.select(g.data, ["pop"]), g.geometry)
        end)

      assert DataFrame.names(out.data) == ["key", "zone", "pop"]
      assert col(out, "key") == ["a", "a", "b", "b", "b"]
      assert col(out, "zone") == [2, 2, 1, 1, 2]
      assert col(out, "pop") == [5, 7, 10, 20, nil]
    end

    test "a function returning Explorer dataframes gives an Explorer dataframe", %{gdf: gdf} do
      out =
        GeoDataFrame.group_map(gdf, "key", fn g ->
          DataFrame.new(n: [GeoDataFrame.n_rows(g)])
        end)

      assert %DataFrame{} = out
      assert DataFrame.to_columns(out, atom_keys: true) == %{key: ["a", "b"], n: [2, 3]}
    end

    test "rejects mixes, mismatched CRS or geometry column, and other returns", %{gdf: gdf} do
      assert_raise ArgumentError, ~r/mix/, fn ->
        GeoDataFrame.group_map(gdf, "key", fn g ->
          if col(g, "key") == ["a", "a"], do: g, else: g.data
        end)
      end

      assert_raise ArgumentError, ~r/group \["b"\].*CRS/, fn ->
        GeoDataFrame.group_map(gdf, "key", fn g ->
          if hd(col(g, "key")) == "b",
            do: GeoDataFrame.set_crs(g, 4326, allow_override: true),
            else: g
        end)
      end

      assert_raise ArgumentError, ~r/group \["b"\].*geometry column/, fn ->
        GeoDataFrame.group_map(gdf, "key", fn g ->
          if hd(col(g, "key")) == "b", do: GeoDataFrame.rename_geometry(g, "geom"), else: g
        end)
      end

      assert_raise ArgumentError, ~r/GeoDataFrame or an Explorer/, fn ->
        GeoDataFrame.group_map(gdf, "key", fn _ -> 1 end)
      end

      assert_raise ArgumentError, fn -> GeoDataFrame.group_map(gdf, "geometry", & &1) end
      assert_raise ArgumentError, fn -> GeoDataFrame.group_map(gdf, "nope", & &1) end
    end
  end

  describe "n_distinct" do
    test "excludes nulls (pandas nunique)", %{gdf: gdf} do
      out =
        gdf
        |> GeoDataFrame.group_by(["key", "zone"])
        |> GeoDataFrame.summarise(kinds: {:n_distinct, "name"})

      assert col(out, "kinds") == [1, 2, 1]

      out = gdf |> GeoDataFrame.group_by("key") |> GeoDataFrame.summarise(k: {:n_distinct, "pop"})
      assert col(out, "k") == [2, 2]
    end
  end

  describe "row verbs on a grouped frame act per group (Explorer semantics)" do
    setup %{gdf: gdf}, do: %{grouped: GeoDataFrame.group_by(gdf, "key")}

    defp positions(out, gdf) do
      wkts = GeoSeries.to_wkt(gdf.geometry)
      Enum.map(GeoSeries.to_wkt(out.geometry), fn w -> Enum.find_index(wkts, &(&1 == w)) end)
    end

    test "head and tail", %{gdf: gdf, grouped: grouped} do
      head = GeoDataFrame.head(grouped, 1)
      assert GeoDataFrame.groups(head) == ["key"]
      # original row order, as pandas groupby.head
      assert positions(head, gdf) == [0, 2, 5]
      assert col(head, "name") == ["p", nil, "u"]

      tail = GeoDataFrame.tail(grouped, 1)
      assert positions(tail, gdf) == [3, 4, 5]

      two = GeoDataFrame.head(grouped, 2)
      assert positions(two, gdf) == [0, 1, 2, 4, 5]
    end

    test "slice and sample", %{gdf: gdf, grouped: grouped} do
      slice = GeoDataFrame.slice(grouped, 1, 1)
      assert positions(slice, gdf) == [1, 4]
      assert col(slice, "name") == ["q", "t"]

      sample = GeoDataFrame.sample(grouped, 1, seed: 1)
      assert length(positions(sample, gdf)) == 3
      assert Enum.sort(col(sample, "key") |> Enum.map(&to_string/1)) == ["", "a", "b"]

      # the geometry follows the rows Explorer picked
      assert col(sample, "name") ==
               Enum.map(positions(sample, gdf), &Enum.at(col(gdf, "name"), &1))
    end

    test "ungrouped behaviour is unchanged", %{gdf: gdf} do
      assert col(GeoDataFrame.head(gdf, 2), "name") == ["p", "q"]
      assert col(GeoDataFrame.tail(gdf, 1), "name") == ["u"]
    end
  end

  describe "GeoFrame delegates" do
    test "group_by, summarise, ungroup and group_map", %{gdf: gdf} do
      grouped = GeoFrame.group_by(gdf, "key")
      assert GeoDataFrame.groups(grouped) == ["key"]

      assert grouped |> GeoFrame.summarise(total: {:sum, "pop"}) |> then(&col(&1, "total")) == [
               12,
               30
             ]

      assert GeoFrame.ungroup(grouped) == gdf
      assert gdf |> GeoFrame.group_map("key", &max_pop/1) |> then(&col(&1, "pop")) == [7, 20]
    end
  end
end
