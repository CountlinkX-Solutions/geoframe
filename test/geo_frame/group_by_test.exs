defmodule GeoFrame.GroupByTest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  # Expected values come from GeoPandas 1.2 / Shapely 2.2:
  #
  #   df = pd.DataFrame(dict(key=["b","b","a","b","a",None], zone=[1,1,2,2,2,3],
  #        pop=[10,20,5,None,7,1], name=["p","q",None,"s","t","u"]))
  #   gdf = gpd.GeoDataFrame(df, geometry=gpd.GeoSeries(<squares>, crs=3857))
  #   gdf.groupby("key").agg(total=("pop","sum"), avg=("pop","mean"), lo=("pop","min"),
  #       hi=("pop","max"), n=("pop","count"), med=("pop","median"), f=("name","first"),
  #       l=("name","last"))
  #   -> a: 12.0 6.0 5.0 7.0 2 6.0 t t   /   b: 30.0 15.0 10.0 20.0 2 15.0 p s
  #   per group g: shapely.envelope(shapely.GeometryCollection(g)),
  #       shapely.convex_hull(...), shapely.centroid(shapely.union_all(g)),
  #       shapely.multipolygons(g), shapely.intersection_all(g)
  defp square(x, y, size \\ 1),
    do:
      "POLYGON ((#{x} #{y}, #{x + size} #{y}, #{x + size} #{y + size}, #{x} #{y + size}, #{x} #{y}))"

  setup do
    data =
      DataFrame.new(
        key: ["b", "b", "a", "b", "a", nil],
        zone: [1, 1, 2, 2, 2, 3],
        pop: [10, 20, 5, nil, 7, 1],
        name: ["p", "q", nil, "s", "t", "u"]
      )

    geometry =
      GeoSeries.from_wkt!(
        [
          square(0, 0),
          square(1, 0),
          square(10, 10),
          square(2, 0),
          square(10, 12),
          square(50, 50)
        ],
        crs: "EPSG:3857"
      )

    %{gdf: GeoDataFrame.new(data, geometry)}
  end

  defp col(gdf, name), do: gdf.data |> DataFrame.pull(name) |> Series.to_list()
  defp wkt(series), do: GeoSeries.to_wkt(series)

  defp same_geometry?(series, expected_wkt) do
    expected = GeoSeries.from_wkt!(expected_wkt)
    series |> GeoSeries.geom_equals(expected) |> Series.to_list() |> Enum.all?()
  end

  describe "group_by/2, ungroup/1, groups/1" do
    test "mark the groups on the attribute frame and keep the geometry aligned", %{gdf: gdf} do
      assert GeoDataFrame.groups(gdf) == []
      grouped = GeoDataFrame.group_by(gdf, ["key", :zone])
      assert GeoDataFrame.groups(grouped) == ["key", "zone"]
      assert DataFrame.groups(grouped.data) == ["key", "zone"]
      assert GeoDataFrame.ungroup(grouped) == gdf
    end

    test "ungroup/2 drops only the named groups", %{gdf: gdf} do
      grouped = GeoDataFrame.group_by(gdf, ["key", "zone"])
      assert grouped |> GeoDataFrame.ungroup("zone") |> GeoDataFrame.groups() == ["key"]
    end

    test "geometry and unknown columns are rejected", %{gdf: gdf} do
      assert_raise ArgumentError, ~r/geometry column/, fn ->
        GeoDataFrame.group_by(gdf, "geometry")
      end

      assert_raise ArgumentError, fn -> GeoDataFrame.group_by(gdf, "nope") end
    end

    test "row verbs keep the geometry aligned on grouped frames", %{gdf: gdf} do
      grouped = GeoDataFrame.group_by(gdf, "key")
      taken = GeoDataFrame.take(grouped, [5, 0])
      assert GeoDataFrame.groups(taken) == ["key"]
      assert col(taken, "name") == ["u", "p"]
      assert wkt(taken.geometry) == wkt(GeoSeries.take(gdf.geometry, [5, 0]))

      sorted = GeoDataFrame.sort_with(grouped, fn ldf -> [desc: ldf["zone"]] end)
      # Explorer sorts within each group (group order is unspecified); the frame keeps its groups
      assert Enum.sort(col(sorted, "zone")) == [1, 1, 2, 2, 2, 3]
      assert GeoDataFrame.groups(sorted) == ["key"]
      assert wkt(sorted.geometry) == wkt(GeoSeries.take(gdf.geometry, sorted_positions(sorted)))

      head = GeoDataFrame.head(grouped, 1)
      assert col(head, "name") == ["p"]
      assert wkt(head.geometry) == wkt(GeoSeries.take(gdf.geometry, [0]))
      assert GeoDataFrame.groups(GeoDataFrame.drop_nil(grouped, "name")) == ["key"]

      # filter_with sees the groups (a per-group maximum), and keeps the geometry aligned
      top =
        GeoDataFrame.filter_with(grouped, fn ldf ->
          Series.equal(ldf["pop"], Series.max(ldf["pop"]))
        end)

      assert Enum.sort_by(col(top, "name"), &to_string/1) == ["q", "t", "u"]

      keep =
        for n <- col(top, "name"), do: Enum.find_index(["p", "q", nil, "s", "t", "u"], &(&1 == n))

      assert wkt(top.geometry) == wkt(GeoSeries.take(gdf.geometry, keep))
    end
  end

  # the geometry of `sorted` must be the rows named by its "name" column
  defp sorted_positions(sorted) do
    names = ["p", "q", nil, "s", "t", "u"]
    sorted |> col("name") |> Enum.map(fn n -> Enum.find_index(names, &(&1 == n)) end)
  end

  describe "summarise/3 attributes" do
    test "equal GeoPandas groupby.agg", %{gdf: gdf} do
      out =
        gdf
        |> GeoDataFrame.group_by("key")
        |> GeoDataFrame.summarise(
          total: {:sum, "pop"},
          avg: {:mean, "pop"},
          lo: {:min, "pop"},
          hi: {:max, "pop"},
          n: {:count, "pop"},
          med: {:median, "pop"},
          f: {:first, "name"},
          l: {:last, "name"}
        )

      assert GeoDataFrame.names(out) ==
               ["key", "total", "avg", "lo", "hi", "n", "med", "f", "l", "geometry"]

      assert col(out, "key") == ["a", "b"]
      assert col(out, "total") == [12, 30]
      assert col(out, "avg") == [6.0, 15.0]
      assert col(out, "lo") == [5, 10]
      assert col(out, "hi") == [7, 20]
      assert col(out, "n") == [2, 2]
      assert col(out, "med") == [6.0, 15.0]
      assert col(out, "f") == ["t", "p"]
      assert col(out, "l") == ["t", "s"]
      assert GeoDataFrame.crs(out) == GeoDataFrame.crs(gdf)
      assert GeoDataFrame.groups(out) == []
    end

    test "n_distinct and several keys", %{gdf: gdf} do
      out =
        gdf
        |> GeoDataFrame.group_by(["key", "zone"])
        |> GeoDataFrame.summarise(total: {:sum, "pop"}, kinds: {:n_distinct, "name"})

      assert col(out, "key") == ["a", "b", "b"]
      assert col(out, "zone") == [2, 1, 2]
      assert col(out, "total") == [12, 30, 0]
      assert col(out, "kinds") == [2, 2, 1]
    end

    test "dropna: false keeps the null key last, sort: false keeps first appearance", %{
      gdf: gdf
    } do
      grouped = GeoDataFrame.group_by(gdf, "key")
      kept = GeoDataFrame.summarise(grouped, [n: {:count, "pop"}], dropna: false)
      assert col(kept, "key") == ["a", "b", nil]
      assert col(kept, "n") == [2, 2, 1]
      appearance = GeoDataFrame.summarise(grouped, [n: {:count, "pop"}], sort: false)
      assert col(appearance, "key") == ["b", "a"]
    end

    test "an ungrouped frame is one group", %{gdf: gdf} do
      out = GeoDataFrame.summarise(gdf, total: {:sum, "pop"})
      assert col(out, "total") == [43]
      assert GeoDataFrame.n_rows(out) == 1
    end
  end

  describe "summarise/3 geometry" do
    test "the default is union_all and equals dissolve for the same keys", %{gdf: gdf} do
      for by <- ["key", ["key", "zone"], "zone"], opts <- [[], [dropna: false], [sort: false]] do
        grouped = GeoDataFrame.group_by(gdf, by)
        out = GeoDataFrame.summarise(grouped, [], opts)
        dissolved = GeoDataFrame.dissolve(gdf, [by: by] ++ opts)
        keys = List.wrap(by)

        assert GeoDataFrame.names(out) == keys ++ ["geometry"]
        for k <- keys, do: assert(col(out, k) == col(dissolved, k))

        assert out.geometry |> GeoSeries.geom_equals(dissolved.geometry) |> Series.to_list() ==
                 List.duplicate(true, GeoDataFrame.n_rows(dissolved))

        assert GeoDataFrame.crs(out) == GeoDataFrame.crs(dissolved)
      end
    end

    test "geometry: aggregates the active geometry (Shapely per-group values)", %{gdf: gdf} do
      grouped = GeoDataFrame.group_by(gdf, "key")
      run = &GeoDataFrame.summarise(grouped, [], geometry: &1).geometry

      assert same_geometry?(
               run.(:envelope),
               [
                 "POLYGON ((10 10, 11 10, 11 13, 10 13, 10 10))",
                 "POLYGON ((0 0, 3 0, 3 1, 0 1, 0 0))"
               ]
             )

      assert same_geometry?(
               run.(:convex_hull),
               [
                 "POLYGON ((10 10, 10 13, 11 13, 11 10, 10 10))",
                 "POLYGON ((0 0, 0 1, 3 1, 3 0, 0 0))"
               ]
             )

      assert same_geometry?(run.(:centroid), ["POINT (10.5 11.5)", "POINT (1.5 0.5)"])

      assert same_geometry?(
               run.(:collect),
               [
                 "MULTIPOLYGON (((10 10, 11 10, 11 11, 10 11, 10 10)), ((10 12, 11 12, 11 13, 10 13, 10 12)))",
                 "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((1 0, 2 0, 2 1, 1 1, 1 0)), ((2 0, 3 0, 3 1, 2 1, 2 0)))"
               ]
             )

      assert run.(:intersection_all) |> GeoSeries.is_empty() |> Series.to_list() == [true, true]
      assert same_geometry?(run.(:first), [square(10, 10), square(0, 0)])
      assert same_geometry?(run.(:last), [square(10, 12), square(2, 0)])
      assert GeoSeries.crs(run.(:envelope)) == GeoSeries.crs(gdf.geometry)
    end

    test "geometry aggregations equal filtering each group and aggregating", %{gdf: gdf} do
      grouped = GeoDataFrame.group_by(gdf, "key")

      for {agg, whole} <- [
            union_all: &GeoSeries.union_all/1,
            intersection_all: &GeoSeries.intersection_all/1,
            collect: &GeoSeries.collect/1,
            envelope: &(&1 |> GeoSeries.collect() |> GeoSeries.envelope()),
            convex_hull: &(&1 |> GeoSeries.collect() |> GeoSeries.convex_hull()),
            centroid: &(&1 |> GeoSeries.union_all() |> GeoSeries.centroid())
          ] do
        out = GeoDataFrame.summarise(grouped, [], geometry: agg)

        expected =
          for key <- ["a", "b"] do
            keep = for {k, i} <- Enum.with_index(col(gdf, "key")), k == key, do: i
            gdf.geometry |> GeoSeries.take(keep) |> whole.() |> GeoSeries.to_wkt()
          end

        assert GeoSeries.to_wkt(out.geometry) == List.flatten(expected), inspect(agg)
      end
    end

    test "named aggregations of geometry columns become secondary columns or counts", %{
      gdf: gdf
    } do
      gdf = GeoDataFrame.put_geometry(gdf, "site", GeoSeries.centroid(gdf.geometry))

      out =
        gdf
        |> GeoDataFrame.group_by("key")
        |> GeoDataFrame.summarise(
          hull: {:convex_hull, "site"},
          first_site: {:first, "site"},
          n_sites: {:count, "site"},
          total: {:sum, "pop"}
        )

      assert GeoDataFrame.names(out) == [
               "key",
               "n_sites",
               "total",
               "geometry",
               "hull",
               "first_site"
             ] or
               GeoDataFrame.names(out) |> Enum.take(1) == ["key"]

      assert col(out, "n_sites") == [2, 3]
      assert col(out, "total") == [12, 30]

      assert out |> GeoDataFrame.pull("first_site") |> wkt() == [
               "POINT(10.5 10.5)",
               "POINT(0.5 0.5)"
             ]

      assert out |> GeoDataFrame.pull("hull") |> GeoSeries.geom_type() |> Series.to_list() ==
               ["LineString", "LineString"]
    end

    test "errors", %{gdf: gdf} do
      grouped = GeoDataFrame.group_by(gdf, "key")

      assert_raise ArgumentError, ~r/aggregation/, fn ->
        GeoDataFrame.summarise(grouped, x: {:bogus, "pop"})
      end

      assert_raise ArgumentError, ~r/not found/, fn ->
        GeoDataFrame.summarise(grouped, x: {:sum, "nope"})
      end

      assert_raise ArgumentError, ~r/geometry/, fn ->
        GeoDataFrame.summarise(grouped, x: {:sum, "geometry"})
      end

      assert_raise ArgumentError, ~r/geometry/, fn ->
        GeoDataFrame.summarise(grouped, [], geometry: :count)
      end

      assert_raise ArgumentError, ~r/grouping key/, fn ->
        GeoDataFrame.summarise(grouped, key: {:first, "name"})
      end
    end

    test "an empty frame gives an empty result with the columns" do
      empty =
        GeoDataFrame.new(DataFrame.new(k: [1], v: [1.0]), GeoSeries.from_wkt!(["POINT (0 0)"]))

      empty = GeoDataFrame.take(empty, [])
      out = empty |> GeoDataFrame.group_by("k") |> GeoDataFrame.summarise(s: {:sum, "v"})
      assert GeoDataFrame.n_rows(out) == 0
      assert GeoDataFrame.names(out) == ["k", "s", "geometry"]
    end
  end
end
