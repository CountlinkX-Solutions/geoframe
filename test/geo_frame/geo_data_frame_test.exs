defmodule GeoFrame.GeoDataFrameTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  require DataFrame

  defp sample(n \\ 5, opts \\ []) do
    data = DataFrame.new(id: Enum.to_list(0..(n - 1)), name: Enum.map(0..(n - 1), &"n#{&1}"))
    geometry = GeoSeries.from_wkt!(Enum.map(0..(n - 1), &"POINT (#{&1} #{&1 * 2})"), opts)
    GeoDataFrame.new(data, geometry, opts)
  end

  defp ids(gdf), do: gdf |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list()

  defp xs(gdf) do
    gdf
    |> GeoDataFrame.geometry()
    |> GeoSeries.to_wkt()
    |> Enum.map(fn "POINT(" <> rest ->
      rest |> String.split(" ") |> hd() |> String.to_integer()
    end)
  end

  describe "new/3" do
    test "builds a frame and exposes basic accessors" do
      gdf = sample(3, crs: "EPSG:4326")
      assert GeoDataFrame.n_rows(gdf) == 3
      assert GeoDataFrame.names(gdf) == ["id", "name", "geometry"]
      assert GeoDataFrame.crs(gdf) == "EPSG:4326"
      assert %GeoSeries{} = GeoDataFrame.geometry(gdf)
      assert %DataFrame{} = GeoDataFrame.data(gdf)
    end

    test "raises on row count mismatch" do
      data = DataFrame.new(id: [1, 2])
      geometry = GeoSeries.from_wkt!(["POINT (0 0)"])
      assert_raise ArgumentError, ~r/rows/, fn -> GeoDataFrame.new(data, geometry) end
    end

    test "raises when data already has the geometry column" do
      data = DataFrame.new(geometry: [1])
      geometry = GeoSeries.from_wkt!(["POINT (0 0)"])
      assert_raise ArgumentError, ~r/geometry/, fn -> GeoDataFrame.new(data, geometry) end
    end

    test "honours geometry_column" do
      data = DataFrame.new(geometry: [1])
      geometry = GeoSeries.from_wkt!(["POINT (0 0)"])
      gdf = GeoDataFrame.new(data, geometry, geometry_column: "geom")
      assert GeoDataFrame.names(gdf) == ["geometry", "geom"]
    end
  end

  describe "from_wkt/3 and from_wkb/3" do
    test "moves a WKT column into the geometry" do
      df = DataFrame.new(id: [1, 2], wkt: ["POINT (1 2)", nil])
      assert {:ok, gdf} = GeoDataFrame.from_wkt(df, "wkt", crs: "EPSG:4326")
      assert GeoDataFrame.names(gdf) == ["id", "wkt"]
      assert GeoDataFrame.crs(gdf) == "EPSG:4326"
      assert GeoSeries.to_wkt(GeoDataFrame.geometry(gdf)) == ["POINT(1 2)", nil]
      refute "wkt" in DataFrame.names(GeoDataFrame.data(gdf))
    end

    test "reports invalid geometry and missing column" do
      df = DataFrame.new(id: [1], wkt: ["nope"])
      assert {:error, msg} = GeoDataFrame.from_wkt(df, "wkt")
      assert msg =~ "row 0"
      assert {:error, _} = GeoDataFrame.from_wkt(df, "missing")
      assert_raise ArgumentError, fn -> GeoDataFrame.from_wkt!(df, "wkt") end
    end

    test "from_wkb round-trips with to_explorer" do
      gdf = sample(3)
      df = GeoDataFrame.to_explorer(gdf)
      assert DataFrame.names(df) == ["id", "name", "geometry"]
      assert {:ok, back} = GeoDataFrame.from_wkb(df, "geometry")
      assert xs(back) == [0, 1, 2]
      assert ids(back) == [0, 1, 2]
    end

    test "to_explorer with wkt encoding" do
      df = GeoDataFrame.to_explorer(sample(2), encoding: :wkt)
      assert df["geometry"] |> Series.to_list() == ["POINT(0 0)", "POINT(1 2)"]
    end
  end

  describe "row-changing operations keep rows aligned" do
    test "take/2" do
      gdf = GeoDataFrame.take(sample(), [4, 0, 0])
      assert ids(gdf) == [4, 0, 0]
      assert xs(gdf) == [4, 0, 0]
    end

    test "slice/3 with positive and negative offsets" do
      assert gdf = GeoDataFrame.slice(sample(), 1, 2)
      assert {ids(gdf), xs(gdf)} == {[1, 2], [1, 2]}
      gdf = GeoDataFrame.slice(sample(), -2, 2)
      assert {ids(gdf), xs(gdf)} == {[3, 4], [3, 4]}
    end

    test "head/2 and tail/2" do
      assert xs(GeoDataFrame.head(sample(), 2)) == [0, 1]
      assert ids(GeoDataFrame.head(sample(), 2)) == [0, 1]
      assert xs(GeoDataFrame.tail(sample(), 2)) == [3, 4]
      assert ids(GeoDataFrame.tail(sample(), 2)) == [3, 4]
    end

    test "filter_with/2" do
      gdf = GeoDataFrame.filter_with(sample(), &Series.greater(&1["id"], 2))
      assert {ids(gdf), xs(gdf)} == {[3, 4], [3, 4]}
      assert GeoDataFrame.names(gdf) == ["id", "name", "geometry"]
    end

    test "filter_with/2 accepts a lazy expression" do
      gdf = GeoDataFrame.filter_with(sample(), fn df -> Series.equal(df["name"], "n1") end)
      assert {ids(gdf), xs(gdf)} == {[1], [1]}
    end

    test "filter_with/2 works when a column collides with the default temp name" do
      data = DataFrame.new([{"id", [0, 1, 2]}, {"__geoframe_row__", [9, 9, 9]}])
      g = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"])
      gdf = GeoDataFrame.new(data, g)
      out = GeoDataFrame.filter_with(gdf, &Series.greater(&1["id"], 0))
      assert ids(out) == [1, 2]
      assert xs(out) == [1, 2]
      assert GeoDataFrame.names(out) == ["id", "__geoframe_row__", "geometry"]
    end

    test "sort_with/2" do
      gdf = GeoDataFrame.sort_with(sample(), &[desc: &1["id"]])
      assert {ids(gdf), xs(gdf)} == {[4, 3, 2, 1, 0], [4, 3, 2, 1, 0]}
    end

    test "works on frames without attribute columns" do
      g = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"])
      gdf = GeoDataFrame.new(DataFrame.new([]), g)
      assert GeoDataFrame.n_rows(gdf) == 2
      assert GeoDataFrame.n_rows(GeoDataFrame.head(gdf, 1)) == 1

      filtered =
        GeoDataFrame.filter_with(gdf, fn df -> Series.greater(df[hd(DataFrame.names(df))], 0) end)

      assert GeoDataFrame.n_rows(filtered) == 1
    end
  end

  describe "attribute operations" do
    test "mutate_with/2 keeps geometry" do
      gdf = GeoDataFrame.mutate_with(sample(), &[double: Series.multiply(&1["id"], 2)])
      assert GeoDataFrame.names(gdf) == ["id", "name", "double", "geometry"]
      assert xs(gdf) == [0, 1, 2, 3, 4]
    end

    test "mutate_with/2 refuses to shadow the geometry column" do
      assert_raise ArgumentError, ~r/geometry/, fn ->
        GeoDataFrame.mutate_with(sample(), &[geometry: &1["id"]])
      end
    end

    test "select/2 always keeps geometry" do
      gdf = GeoDataFrame.select(sample(), ["name"])
      assert GeoDataFrame.names(gdf) == ["name", "geometry"]
      gdf = GeoDataFrame.select(sample(), ["name", "geometry"])
      assert GeoDataFrame.names(gdf) == ["name", "geometry"]
    end

    test "rename/2 renames attributes" do
      gdf = GeoDataFrame.rename(sample(), %{"id" => "key"})
      assert GeoDataFrame.names(gdf) == ["key", "name", "geometry"]

      assert_raise ArgumentError, ~r/geometry/, fn ->
        GeoDataFrame.rename(sample(), %{"id" => "geometry"})
      end
    end

    test "set_crs/2" do
      gdf = GeoDataFrame.set_crs(sample(), "EPSG:3857")
      assert GeoDataFrame.crs(gdf) == "EPSG:3857"
    end
  end

  describe "spatial measures" do
    setup do
      data = DataFrame.new(id: [1, 2])

      g =
        GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))", "POINT (1 1)"],
          crs: "EPSG:4326"
        )

      {:ok, gdf: GeoDataFrame.new(data, g)}
    end

    test "area, length, bounds", %{gdf: gdf} do
      assert Series.to_list(GeoDataFrame.area(gdf)) == [12.0, 0.0]
      assert Series.to_list(GeoDataFrame.length(gdf)) == [14.0, 0.0]
      assert %DataFrame{} = GeoDataFrame.bounds(gdf)
    end

    test "centroid replaces geometry", %{gdf: gdf} do
      c = GeoDataFrame.centroid(gdf)
      assert GeoSeries.to_wkt(GeoDataFrame.geometry(c)) == ["POINT(2 1.5)", "POINT(1 1)"]
      assert GeoDataFrame.crs(c) == "EPSG:4326"
      assert DataFrame.pull(GeoDataFrame.data(c), "id") |> Series.to_list() == [1, 2]
    end
  end

  describe "protocols" do
    test "Table.Reader exposes attributes and WKT geometry as columns" do
      assert {:columns, %{columns: cols, count: 2}, columns} = Table.Reader.init(sample(2))
      assert cols == ["id", "name", "geometry"]

      assert Enum.map(columns, &Enum.to_list/1) ==
               [[0, 1], ["n0", "n1"], ["POINT(0 0)", "POINT(1 2)"]]
    end

    test "Table.to_rows works" do
      rows = Table.to_rows(sample(2)) |> Enum.to_list()
      assert hd(rows) == %{"id" => 0, "name" => "n0", "geometry" => "POINT(0 0)"}
    end

    test "Inspect summarises the frame" do
      out = inspect(sample(7, crs: "EPSG:4326"))
      assert out =~ "GeoFrame.GeoDataFrame"
      assert out =~ "rows: 7"
      assert out =~ "EPSG:4326"
      assert out =~ "POINT(0 0)"
    end
  end

  describe "alignment invariant (properties)" do
    defp tagged(ids) do
      data = DataFrame.new(id: ids)
      g = GeoSeries.from_wkt!(Enum.map(ids, &"POINT (#{&1} 0)"))
      GeoDataFrame.new(data, g)
    end

    defp op_gen do
      one_of([
        tuple({constant(:take), list_of(integer(0..1000), max_length: 20)}),
        tuple({constant(:slice), integer(-30..30), integer(0..30)}),
        tuple({constant(:head), integer(0..30)}),
        tuple({constant(:tail), integer(0..30)}),
        tuple({constant(:filter), integer(-5..40)}),
        tuple({constant(:sort), boolean()})
      ])
    end

    defp apply_op(gdf, {:take, raw}) do
      n = GeoDataFrame.n_rows(gdf)
      if n == 0, do: gdf, else: GeoDataFrame.take(gdf, Enum.map(raw, &rem(&1, n)))
    end

    defp apply_op(gdf, {:slice, o, l}), do: GeoDataFrame.slice(gdf, o, l)
    defp apply_op(gdf, {:head, n}), do: GeoDataFrame.head(gdf, n)
    defp apply_op(gdf, {:tail, n}), do: GeoDataFrame.tail(gdf, n)

    defp apply_op(gdf, {:filter, t}),
      do: GeoDataFrame.filter_with(gdf, &Series.greater(&1["id"], t))

    defp apply_op(gdf, {:sort, true}), do: GeoDataFrame.sort_with(gdf, &[desc: &1["id"]])
    defp apply_op(gdf, {:sort, false}), do: GeoDataFrame.sort_with(gdf, &[asc: &1["id"]])

    property "id and geometry stay paired after random operation sequences" do
      check all(
              raw_ids <- list_of(integer(0..10_000), min_length: 1, max_length: 30),
              ops <- list_of(op_gen(), max_length: 6),
              max_runs: 60
            ) do
        gdf = Enum.reduce(ops, tagged(raw_ids), &apply_op(&2, &1))
        assert ids(gdf) == xs(gdf)
        assert GeoDataFrame.n_rows(gdf) == length(ids(gdf))
      end
    end
  end
end
