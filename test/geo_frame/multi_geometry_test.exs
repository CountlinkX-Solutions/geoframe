defmodule GeoFrame.MultiGeometryTest do
  # Secondary geometry columns (issue #61). A GeoDataFrame has one active geometry and any
  # number of secondary GeoSeries columns, like a GeoPandas frame with several geometry
  # dtype columns; GeoParquet round-trips all of them with their own `geo` metadata entry.
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @fixtures Path.expand("../reference/fixtures/io/multi", __DIR__)
  @manifest @fixtures |> Path.join("manifest.json") |> File.read!() |> Jason.decode!()

  defp frame do
    data = DataFrame.new(id: [1, 2, 3], name: ["a", "b", nil])
    geometry = GeoSeries.from_wkt!(["POINT(0 0)", "POINT(1 1)", nil], crs: "EPSG:4326")

    footprint =
      GeoSeries.from_wkt!(
        ["POLYGON((0 0,1 0,1 1,0 0))", nil, "POLYGON((5 5,6 5,6 6,5 5))"],
        crs: "EPSG:3857"
      )

    data |> GeoDataFrame.new(geometry) |> GeoDataFrame.put_geometry("footprint", footprint)
  end

  defp wkt(series), do: GeoSeries.to_wkt(series)
  defp ids(gdf), do: gdf |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list()

  describe "model" do
    test "put_geometry adds a secondary column that names, dtypes and pull report" do
      gdf = frame()

      assert GeoDataFrame.names(gdf) == ["id", "name", "geometry", "footprint"]
      assert GeoDataFrame.n_columns(gdf) == 4
      assert GeoDataFrame.geometry_columns(gdf) == ["geometry", "footprint"]
      assert %{"geometry" => :geometry, "footprint" => :geometry} = GeoDataFrame.dtypes(gdf)
      assert %GeoSeries{} = GeoDataFrame.pull(gdf, "footprint")
      assert GeoSeries.crs(GeoDataFrame.pull(gdf, "footprint")) == "EPSG:3857"
    end

    test "put_geometry validates the name and the row count" do
      gdf = frame()
      one = GeoSeries.from_wkt!(["POINT(0 0)"])
      three = GeoSeries.from_wkt!(["POINT(0 0)", nil, nil])

      assert_raise ArgumentError, ~r/3 rows/, fn -> GeoDataFrame.put_geometry(gdf, "x", one) end
      assert_raise ArgumentError, ~r/"id"/, fn -> GeoDataFrame.put_geometry(gdf, "id", three) end

      assert_raise ArgumentError, ~r/"geometry"/, fn ->
        GeoDataFrame.put_geometry(gdf, "geometry", three)
      end

      replaced = GeoDataFrame.put_geometry(gdf, "footprint", three)
      assert wkt(GeoDataFrame.pull(replaced, "footprint")) == wkt(three)
      assert GeoDataFrame.geometry_columns(replaced) == ["geometry", "footprint"]
    end

    test "set_geometry swaps the active column and keeps the old one" do
      gdf = GeoDataFrame.set_geometry(frame(), "footprint")

      assert gdf.geometry_column == "footprint"
      assert GeoDataFrame.crs(gdf) == "EPSG:3857"
      assert GeoDataFrame.geometry_columns(gdf) == ["footprint", "geometry"]
      assert wkt(GeoDataFrame.pull(gdf, "geometry")) == ["POINT(0 0)", "POINT(1 1)", nil]
      assert GeoDataFrame.names(gdf) == ["id", "name", "footprint", "geometry"]
    end

    test "set_geometry drop: true removes the old active column" do
      gdf = GeoDataFrame.set_geometry(frame(), "footprint", drop: true)

      assert GeoDataFrame.geometry_columns(gdf) == ["footprint"]
      assert GeoDataFrame.names(gdf) == ["id", "name", "footprint"]
    end

    test "set_geometry on an unknown or non-geometry column raises" do
      assert_raise ArgumentError, ~r/"nope"/, fn -> GeoDataFrame.set_geometry(frame(), "nope") end
      assert_raise ArgumentError, ~r/"id"/, fn -> GeoDataFrame.set_geometry(frame(), "id") end
    end

    test "set_geometry also takes a series and a column name" do
      new = GeoSeries.from_wkt!(["POINT(9 9)", "POINT(8 8)", "POINT(7 7)"])
      gdf = GeoDataFrame.set_geometry(frame(), new, geometry_column: "pts")

      assert gdf.geometry_column == "pts"
      assert wkt(gdf.geometry) == wkt(new)
      assert GeoDataFrame.geometry_columns(gdf) == ["pts", "geometry", "footprint"]
    end
  end

  describe "rows stay aligned" do
    test "take, slice, head, tail, filter, sort, sample, shuffle, distinct, drop_nil, cx" do
      gdf = frame()
      footprint = fn g -> g |> GeoDataFrame.pull("footprint") |> wkt() end

      assert footprint.(GeoDataFrame.take(gdf, [2, 0])) == [
               "POLYGON((5 5,6 5,6 6,5 5))",
               "POLYGON((0 0,1 0,1 1,0 0))"
             ]

      assert footprint.(GeoDataFrame.head(gdf, 1)) == ["POLYGON((0 0,1 0,1 1,0 0))"]
      assert footprint.(GeoDataFrame.tail(gdf, 1)) == ["POLYGON((5 5,6 5,6 6,5 5))"]
      assert footprint.(GeoDataFrame.slice(gdf, 1, 1)) == [nil]

      filtered = GeoDataFrame.filter_with(gdf, &Series.greater(&1["id"], 1))
      assert ids(filtered) == [2, 3]
      assert footprint.(filtered) == [nil, "POLYGON((5 5,6 5,6 6,5 5))"]

      sorted = GeoDataFrame.sort_with(gdf, &[desc: &1["id"]])
      assert ids(sorted) == [3, 2, 1]
      assert hd(footprint.(sorted)) == "POLYGON((5 5,6 5,6 6,5 5))"

      for op <- [&GeoDataFrame.sample(&1, 3, seed: 1), &GeoDataFrame.shuffle(&1, seed: 2)] do
        out = op.(gdf)
        pairs = Enum.zip(ids(out), footprint.(out))
        assert Enum.sort(pairs) == Enum.sort(Enum.zip(ids(gdf), footprint.(gdf)))
      end

      assert footprint.(GeoDataFrame.distinct(gdf)) == footprint.(gdf)
      assert ids(GeoDataFrame.drop_nil(gdf)) == [1, 2]
      assert footprint.(GeoDataFrame.cx(gdf, 0, 0, 2, 2)) == ["POLYGON((0 0,1 0,1 1,0 0))", nil]
    end

    test "join keeps the left secondary geometry, null for right-only rows" do
      right = DataFrame.new(id: [3, 9], extra: ["x", "y"])
      joined = GeoDataFrame.join(frame(), right, on: "id", how: :right)

      assert ids(joined) == [3, 9]

      assert joined |> GeoDataFrame.pull("footprint") |> wkt() ==
               ["POLYGON((5 5,6 5,6 6,5 5))", nil]

      assert GeoDataFrame.crs(joined) == "EPSG:4326"
    end

    test "concat_rows stacks every geometry column and demands the same set" do
      both = GeoDataFrame.concat_rows([frame(), frame()])
      assert GeoDataFrame.n_rows(both) == 6
      assert both |> GeoDataFrame.pull("footprint") |> GeoSeries.size() == 6

      assert_raise ArgumentError, ~r/geometry column/, fn ->
        GeoDataFrame.concat_rows([frame(), GeoDataFrame.set_geometry(frame(), "footprint")])
      end

      plain =
        GeoDataFrame.new(DataFrame.new(id: [1, 2, 3], name: ["a", "b", nil]), frame().geometry)

      assert_raise ArgumentError, ~r/geometry column/, fn ->
        GeoDataFrame.concat_rows([frame(), plain])
      end
    end

    test "explode repeats the secondary geometry of each source row" do
      data = DataFrame.new(id: [1, 2])

      gdf =
        data
        |> GeoDataFrame.new(GeoSeries.from_wkt!(["MULTIPOINT ((0 0), (1 1))", "POINT(2 2)"]))
        |> GeoDataFrame.put_geometry("tag", GeoSeries.from_wkt!(["POINT(10 10)", "POINT(20 20)"]))

      out = GeoDataFrame.explode(gdf)
      assert ids(out) == [1, 1, 2]

      assert out |> GeoDataFrame.pull("tag") |> wkt() ==
               ["POINT(10 10)", "POINT(10 10)", "POINT(20 20)"]
    end

    test "dissolve keeps the secondary columns (first non-null geometry per group)" do
      out = GeoDataFrame.dissolve(frame(), by: "name")
      assert GeoDataFrame.geometry_columns(out) == ["geometry", "footprint"]
    end
  end

  describe "column verbs" do
    test "select and discard treat secondary geometry columns like columns" do
      gdf = frame()

      assert GeoDataFrame.geometry_columns(GeoDataFrame.select(gdf, ["id"])) == ["geometry"]

      assert GeoDataFrame.geometry_columns(GeoDataFrame.select(gdf, ["id", "footprint"])) ==
               ["geometry", "footprint"]

      assert GeoDataFrame.geometry_columns(GeoDataFrame.discard(gdf, ["footprint"])) ==
               ["geometry"]

      assert GeoDataFrame.names(GeoDataFrame.discard(gdf, ["id"])) ==
               ["name", "geometry", "footprint"]
    end

    test "rename renames a secondary geometry column and protects the names" do
      renamed = GeoDataFrame.rename(frame(), %{"footprint" => "fp", "id" => "key"})

      assert GeoDataFrame.names(renamed) == ["key", "name", "geometry", "fp"]

      assert_raise ArgumentError, ~r/reserved/, fn ->
        GeoDataFrame.rename(frame(), %{"id" => "footprint"})
      end

      assert_raise ArgumentError, ~r/reserved/, fn ->
        GeoDataFrame.put(frame(), "footprint", [1, 2, 3])
      end
    end

    test "to_explorer and the Table.Reader list the secondary column too" do
      gdf = frame()
      explorer = GeoDataFrame.to_explorer(gdf, encoding: :wkt)

      assert DataFrame.names(explorer) == ["id", "name", "geometry", "footprint"]

      assert Series.to_list(explorer["footprint"]) ==
               ["POLYGON((0 0,1 0,1 1,0 0))", nil, "POLYGON((5 5,6 5,6 6,5 5))"]

      {:columns, meta, columns} = Table.Reader.init(gdf)
      assert meta.columns == ["id", "name", "geometry", "footprint"]
      assert length(columns) == 4
    end
  end

  describe "GeoParquet" do
    for {file, encoding} <- [
          {"two_wkb.parquet", "WKB"},
          {"two_native.parquet", "native"},
          {"two_covering.parquet", "WKB with covering"}
        ] do
      test "reads a GeoPandas file with three geometry columns (#{encoding})" do
        file = unquote(file)
        expected = @manifest["files"][file]
        gdf = GeoFrame.read_parquet!(Path.join(@fixtures, file))

        assert gdf.geometry_column == expected["primary"]
        assert GeoDataFrame.geometry_columns(gdf) == expected["geometry_columns"]
        assert GeoDataFrame.names(gdf) == ["id", "name" | expected["geometry_columns"]]
        assert ids(gdf) == expected["id"]

        for name <- expected["geometry_columns"] do
          series = GeoDataFrame.pull(gdf, name)
          want = expected["columns"][name]
          assert GeoSeries.crs(series) == want["crs"], "crs of #{name}"
          assert wkt(series) == normalize(want["wkt"]), "geometry #{name}"
        end
      end
    end

    test "round-trips every geometry column, CRS and metadata entry", %{tmp_dir: dir} do
      path = Path.join(dir, "out.parquet")
      assert :ok = GeoFrame.write_parquet(frame(), path)

      geo = GeoFrame.geoparquet_metadata!(path)
      assert geo["primary_column"] == "geometry"
      assert Map.keys(geo["columns"]) |> Enum.sort() == ["footprint", "geometry"]
      assert geo["columns"]["footprint"]["geometry_types"] == ["Polygon"]

      back = GeoFrame.read_parquet!(path)
      assert GeoDataFrame.names(back) == ["id", "name", "geometry", "footprint"]
      assert wkt(back.geometry) == wkt(frame().geometry)

      assert wkt(GeoDataFrame.pull(back, "footprint")) ==
               wkt(GeoDataFrame.pull(frame(), "footprint"))

      assert GeoDataFrame.crs(back) == "EPSG:4326"
      assert GeoSeries.crs(GeoDataFrame.pull(back, "footprint")) == "EPSG:3857"
    end

    test "a GeoPandas file survives a read and write unchanged", %{tmp_dir: dir} do
      source = GeoFrame.read_parquet!(Path.join(@fixtures, "two_wkb.parquet"))
      path = Path.join(dir, "again.parquet")
      assert :ok = GeoFrame.write_parquet(source, path)

      geo = GeoFrame.geoparquet_metadata!(path)
      assert geo["primary_column"] == "geometry"
      assert geo["columns"] |> Map.keys() |> Enum.sort() == ["centroid", "footprint", "geometry"]
      # An unknown CRS stays an explicit null, a known one keeps its definition.
      assert Map.fetch!(geo["columns"]["centroid"], "crs") == nil
      assert geo["columns"]["footprint"]["crs"]["id"]["code"] == 3857

      back = GeoFrame.read_parquet!(path)
      assert GeoDataFrame.names(back) == GeoDataFrame.names(source)

      for name <- GeoDataFrame.geometry_columns(source) do
        assert wkt(GeoDataFrame.pull(back, name)) == wkt(GeoDataFrame.pull(source, name))

        assert GeoSeries.crs(GeoDataFrame.pull(back, name)) ==
                 GeoSeries.crs(GeoDataFrame.pull(source, name))
      end
    end

    test "round-trips with native encoding and covering columns", %{tmp_dir: dir} do
      path = Path.join(dir, "native.parquet")
      gdf = GeoDataFrame.discard(frame(), "name")

      assert :ok =
               GeoFrame.write_parquet(gdf, path,
                 geometry_encoding: :geoarrow,
                 write_covering_bbox: true
               )

      back = GeoFrame.read_parquet!(path)
      assert GeoDataFrame.names(back) == ["id", "geometry", "footprint"]
      assert wkt(GeoDataFrame.pull(back, "footprint")) == wkt(GeoDataFrame.pull(gdf, "footprint"))
    end

    test "columns: selects secondary geometry columns by name", %{tmp_dir: dir} do
      path = Path.join(dir, "cols.parquet")
      :ok = GeoFrame.write_parquet(frame(), path)

      assert path |> GeoFrame.read_parquet!(columns: ["id"]) |> GeoDataFrame.geometry_columns() ==
               ["geometry"]

      assert path
             |> GeoFrame.read_parquet!(columns: ["id", "footprint"])
             |> GeoDataFrame.geometry_columns() == ["geometry", "footprint"]
    end

    test "a bbox filter drops the same rows from every geometry column", %{tmp_dir: dir} do
      path = Path.join(dir, "bbox.parquet")
      :ok = GeoFrame.write_parquet(frame(), path)

      back = GeoFrame.read_parquet!(path, bbox: {0.5, 0.5, 2.0, 2.0})
      assert ids(back) == [2]
      assert back |> GeoDataFrame.pull("footprint") |> wkt() == [nil]
    end

    test "schema 1.0.0 still rejects native encoding with several geometry columns", %{
      tmp_dir: dir
    } do
      assert {:error, message} =
               GeoFrame.write_parquet(frame(), Path.join(dir, "x.parquet"),
                 geometry_encoding: :geoarrow,
                 schema_version: "1.0.0"
               )

      assert message =~ "1.0.0"
    end
  end

  defp normalize(rows), do: rows |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt()
end
