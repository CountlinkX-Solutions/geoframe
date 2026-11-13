defmodule GeoFrame.StreamFileTest do
  # The open-reader counter is VM wide: run apart from the async modules.
  use ExUnit.Case, async: false

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @moduletag :tmp_dir

  @rows 25

  defp sample(rows) do
    data =
      DataFrame.new(
        id: Enum.to_list(0..(rows - 1)),
        name: Enum.map(0..(rows - 1), &"n#{&1}"),
        score: Enum.map(0..(rows - 1), &(&1 * 0.5))
      )

    wkt = Enum.map(0..(rows - 1), &"POINT (#{&1} #{&1 * 2})")
    GeoDataFrame.new(data, GeoSeries.from_wkt!(wkt, crs: "EPSG:3857"))
  end

  defp written(dir, name, opts \\ [], rows \\ @rows) do
    path = Path.join(dir, name)
    # GeoJSON refuses a projected CRS unless forced; the coordinates are kept as they are.
    opts = if Path.extname(name) == ".geojson", do: [force: true] ++ opts, else: opts
    :ok = GeoFrame.write_file(sample(rows), path, opts)
    path
  end

  defp col(gdf, name), do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  defp assert_same_as_read_file(path, opts, batch_size) do
    whole = GeoFrame.read_file!(path, opts)
    batches = path |> GeoFrame.stream_file([batch_size: batch_size] ++ opts) |> Enum.to_list()

    assert Enum.all?(batches, &match?(%GeoDataFrame{}, &1))
    assert Enum.sum(Enum.map(batches, &GeoDataFrame.n_rows/1)) == GeoDataFrame.n_rows(whole)

    for batch <- batches do
      assert GeoDataFrame.crs(batch) == GeoDataFrame.crs(whole)
      assert batch.geometry_column == whole.geometry_column

      assert batch |> GeoDataFrame.data() |> DataFrame.dtypes() ==
               whole |> GeoDataFrame.data() |> DataFrame.dtypes()
    end

    for name <- DataFrame.names(GeoDataFrame.data(whole)) do
      assert Enum.flat_map(batches, &col(&1, name)) == col(whole, name)
    end

    assert Enum.flat_map(batches, &(&1 |> GeoDataFrame.geometry() |> GeoSeries.to_wkt())) ==
             whole |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()

    batches
  end

  describe "GeoParquet" do
    test "batches have the requested size and concatenate to read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.parquet", row_group_size: 10)
      batches = assert_same_as_read_file(path, [], 7)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [7, 7, 7, 4]
    end

    test "batch_size ignores the row group size", %{tmp_dir: dir} do
      path = written(dir, "s.parquet", row_group_size: 4)
      sizes = path |> GeoFrame.stream_file(batch_size: 10) |> Enum.map(&GeoDataFrame.n_rows/1)
      assert sizes == [10, 10, 5]
    end

    test "columns: projects the attributes", %{tmp_dir: dir} do
      path = written(dir, "s.parquet")

      [batch | _] =
        path |> GeoFrame.stream_file(columns: ["name"], batch_size: 10) |> Enum.to_list()

      assert DataFrame.names(GeoDataFrame.data(batch)) == ["name"]
    end

    test "bbox: keeps the rows read_file/2 keeps", %{tmp_dir: dir} do
      for covering <- [true, false] do
        path =
          written(dir, "s#{covering}.parquet", write_covering_bbox: covering, row_group_size: 4)

        batches = assert_same_as_read_file(path, [bbox: {3, 0, 9, 100}], 3)
        assert Enum.flat_map(batches, &col(&1, "id")) == Enum.to_list(3..9)
      end
    end

    test "an empty file yields no batches", %{tmp_dir: dir} do
      path = Path.join(dir, "empty.parquet")
      empty = GeoDataFrame.new(DataFrame.new(id: [1]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      :ok = GeoFrame.write_file(GeoDataFrame.head(empty, 0), path)
      assert path |> GeoFrame.stream_file() |> Enum.to_list() == []
    end
  end

  describe "FlatGeobuf" do
    test "batches concatenate to read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.fgb", index: false)
      batches = assert_same_as_read_file(path, [], 8)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [8, 8, 8, 1]
    end

    test "an indexed (Hilbert ordered) file streams in file order", %{tmp_dir: dir} do
      path = written(dir, "s.fgb", index: true)
      assert_same_as_read_file(path, [], 6)
    end

    test "bbox: uses the index; a file without one is an error", %{tmp_dir: dir} do
      path = written(dir, "s.fgb", index: true)
      batches = assert_same_as_read_file(path, [bbox: {3, 0, 9, 100}], 3)
      assert batches |> Enum.flat_map(&col(&1, "id")) |> Enum.sort() == Enum.to_list(3..9)

      plain = written(dir, "plain.fgb", index: false)

      assert_raise ArgumentError, ~r/no spatial index/, fn ->
        plain |> GeoFrame.stream_file(bbox: {3, 0, 9, 100}) |> Enum.to_list()
      end
    end

    test "driver: overrides the extension", %{tmp_dir: dir} do
      path = written(dir, "s.fgb")
      odd = Path.join(dir, "s.dat")
      File.cp!(path, odd)
      assert odd |> GeoFrame.stream_file(driver: :flatgeobuf, batch_size: 10) |> Enum.count() == 3
    end
  end

  describe "Feather / Arrow IPC" do
    test "batches have the requested size and concatenate to read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.feather")
      batches = assert_same_as_read_file(path, [], 7)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [7, 7, 7, 4]
    end

    test "the .arrow extension and every geometry encoding stream", %{tmp_dir: dir} do
      for {file, opts} <- [{"s.arrow", []}, {"native.feather", [geometry_encoding: :geoarrow]}] do
        path = written(dir, file, opts)
        assert_same_as_read_file(path, [], 10)
      end
    end

    test "columns: projects the attributes", %{tmp_dir: dir} do
      path = written(dir, "s.feather")
      batches = assert_same_as_read_file(path, [columns: ["name"]], 10)
      assert DataFrame.names(GeoDataFrame.data(hd(batches))) == ["name"]
    end

    test "geometry: names the geometry column", %{tmp_dir: dir} do
      path = Path.join(dir, "geom.feather")
      gdf = GeoDataFrame.new(sample(@rows).data, sample(@rows).geometry, geometry_column: "geom")
      :ok = GeoFrame.write_file(gdf, path)
      batches = assert_same_as_read_file(path, [geometry: "geom"], 10)
      assert hd(batches).geometry_column == "geom"
    end

    test "bbox: is not supported and raises", %{tmp_dir: dir} do
      path = written(dir, "s.feather")

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.stream_file(path, bbox: {0, 0, 1, 1})
      end
    end

    test "an empty file yields no batches", %{tmp_dir: dir} do
      path = Path.join(dir, "empty.feather")
      empty = GeoDataFrame.new(DataFrame.new(id: [1]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      :ok = GeoFrame.write_file(GeoDataFrame.head(empty, 0), path)
      assert path |> GeoFrame.stream_file() |> Enum.to_list() == []
    end

    test "a missing column or a file that is not Feather raises when enumerated", %{tmp_dir: dir} do
      path = written(dir, "s.feather")

      assert_raise ArgumentError, ~r/column "nope" not found/, fn ->
        path |> GeoFrame.stream_file(columns: ["nope"]) |> Enum.to_list()
      end

      junk = Path.join(dir, "junk.feather")
      File.write!(junk, "not arrow")
      assert_raise ArgumentError, fn -> junk |> GeoFrame.stream_file() |> Enum.to_list() end
    end
  end

  describe "Shapefile" do
    test "batches have the requested size and concatenate to read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.shp")
      batches = assert_same_as_read_file(path, [], 7)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [7, 7, 7, 4]
    end

    test "bbox: filters record by record like a post-read filter", %{tmp_dir: dir} do
      path = written(dir, "s.shp")

      batches =
        path |> GeoFrame.stream_file(bbox: {3, 0, 9, 100}, batch_size: 3) |> Enum.to_list()

      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [3, 3, 1]
      assert Enum.flat_map(batches, &col(&1, "id")) == Enum.to_list(3..9)

      assert_raise ArgumentError, ~r/bbox/, fn ->
        path |> GeoFrame.stream_file(bbox: {9, 0, 3, 100}) |> Enum.to_list()
      end
    end

    test "encoding: is applied like read_file/2", %{tmp_dir: dir} do
      path = Path.join(dir, "latin.shp")
      geometry = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"])
      gdf = GeoDataFrame.new(DataFrame.new(t: ["é", "ü"]), geometry)
      :ok = GeoFrame.write_file(gdf, path, encoding: "ISO-8859-1")

      assert_same_as_read_file(path, [], 1)
      batches = assert_same_as_read_file(path, [encoding: "UTF-8"], 1)
      refute Enum.flat_map(batches, &col(&1, "t")) == ["é", "ü"]
    end

    test "a .zip archive streams like a .shp and leaves no files behind", %{tmp_dir: dir} do
      path = written(dir, "s.zip")

      scratch = fn ->
        File.ls!(System.tmp_dir!()) |> Enum.filter(&String.starts_with?(&1, "geoframe-shp-"))
      end

      before = scratch.()
      batches = assert_same_as_read_file(path, [], 10)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [10, 10, 5]
      assert scratch.() == before
      assert [_] = path |> GeoFrame.stream_file(batch_size: 5) |> Enum.take(1)
      assert scratch.() == before
    end

    test "an empty shapefile yields no batches and junk raises", %{tmp_dir: dir} do
      path = Path.join(dir, "empty.shp")
      empty = GeoDataFrame.new(DataFrame.new(id: [1]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      :ok = GeoFrame.write_file(GeoDataFrame.head(empty, 0), path)
      assert path |> GeoFrame.stream_file() |> Enum.to_list() == []

      junk = Path.join(dir, "junk.shp")
      File.write!(junk, "not a shapefile")
      assert_raise ArgumentError, fn -> junk |> GeoFrame.stream_file() |> Enum.to_list() end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.stream_file(path, columns: ["id"])
      end
    end
  end

  describe "GeoPackage" do
    test "batches have the requested size and concatenate to read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.gpkg")
      batches = assert_same_as_read_file(path, [], 7)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [7, 7, 7, 4]
    end

    test "layer: selects a layer like read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.gpkg", layer: "first")
      second = GeoDataFrame.head(sample(@rows), 6)
      :ok = GeoFrame.write_file(second, path, layer: "second", mode: :a)

      assert_raise ArgumentError, ~r/several layers/, fn ->
        path |> GeoFrame.stream_file() |> Enum.to_list()
      end

      assert_raise ArgumentError, ~r/layer "nope" not found/, fn ->
        path |> GeoFrame.stream_file(layer: "nope") |> Enum.to_list()
      end

      batches = assert_same_as_read_file(path, [layer: "second"], 4)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [4, 2]
      assert_same_as_read_file(path, [layer: "first"], 10)
    end

    test "columns: projects the attributes", %{tmp_dir: dir} do
      path = written(dir, "s.gpkg")
      batches = path |> GeoFrame.stream_file(columns: ["name"], batch_size: 10) |> Enum.to_list()
      assert DataFrame.names(GeoDataFrame.data(hd(batches))) == ["name"]
      assert Enum.flat_map(batches, &col(&1, "name")) == Enum.map(0..24, &"n#{&1}")

      assert_raise ArgumentError, ~r/column "nope" not found/, fn ->
        path |> GeoFrame.stream_file(columns: ["nope"]) |> Enum.to_list()
      end
    end

    test "bbox: keeps the matching rows with and without an R-tree", %{tmp_dir: dir} do
      for indexed <- [true, false] do
        path = written(dir, "s#{indexed}.gpkg", spatial_index: indexed)

        batches =
          path |> GeoFrame.stream_file(bbox: {3, 0, 9, 100}, batch_size: 3) |> Enum.to_list()

        assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [3, 3, 1]
        assert Enum.flat_map(batches, &col(&1, "id")) == Enum.to_list(3..9)
        assert Enum.all?(batches, &(GeoDataFrame.crs(&1) == "EPSG:3857"))
      end
    end

    test "an empty layer yields no batches", %{tmp_dir: dir} do
      path = Path.join(dir, "empty.gpkg")
      empty = GeoDataFrame.new(DataFrame.new(id: [1]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      :ok = GeoFrame.write_file(GeoDataFrame.head(empty, 0), path)
      assert path |> GeoFrame.stream_file() |> Enum.to_list() == []

      junk = Path.join(dir, "junk.gpkg")
      File.write!(junk, String.duplicate("not sqlite ", 20))
      assert_raise ArgumentError, fn -> junk |> GeoFrame.stream_file() |> Enum.to_list() end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.stream_file(path, encoding: "x")
      end
    end
  end

  describe "GeoJSON" do
    defp geojson_with_late_types(dir) do
      path = Path.join(dir, "late.geojson")

      features =
        for i <- 0..8 do
          ~s({"type":"Feature","properties":{"n":#{i},"s":"a#{i}","flag":true},) <>
            ~s("geometry":{"type":"Point","coordinates":[#{i},#{i * 2}]}})
        end

      # The last feature turns `n` into a float, adds `late` and a nested value.
      last =
        ~s({"type":"Feature","properties":{"n":2.5,"s":"z","late":"x","flag":null,) <>
          ~s("nested":{"a":[1,2]}},"geometry":{"type":"Point","coordinates":[9,18]}})

      File.write!(
        path,
        ~s({"type":"FeatureCollection","features":[#{Enum.join(features ++ [last], ",")}]})
      )

      path
    end

    test "batches have the requested size and concatenate to read_file/2", %{tmp_dir: dir} do
      path = written(dir, "s.geojson")
      batches = assert_same_as_read_file(path, [], 7)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [7, 7, 7, 4]
      assert Enum.all?(batches, &(GeoDataFrame.crs(&1) == "OGC:CRS84"))
    end

    test "every batch has the dtypes the whole read infers, even for a late feature", %{
      tmp_dir: dir
    } do
      path = geojson_with_late_types(dir)
      whole = GeoFrame.read_file!(path)
      dtypes = whole |> GeoDataFrame.data() |> DataFrame.dtypes()
      assert dtypes["n"] == {:f, 64}
      assert Map.has_key?(dtypes, "late")

      batches = assert_same_as_read_file(path, [], 3)
      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [3, 3, 3, 1]
      assert col(hd(batches), "late") == [nil, nil, nil]
    end

    test "bbox: filters feature by feature and refills batches", %{tmp_dir: dir} do
      path = written(dir, "s.geojson")

      batches =
        path |> GeoFrame.stream_file(bbox: {3, 0, 9, 100}, batch_size: 3) |> Enum.to_list()

      assert Enum.map(batches, &GeoDataFrame.n_rows/1) == [3, 3, 1]
      assert Enum.flat_map(batches, &col(&1, "id")) == Enum.to_list(3..9)

      assert_raise ArgumentError, ~r/bbox must have minx/, fn ->
        GeoFrame.stream_file(path, bbox: {9, 0, 3, 100})
      end
    end

    test "an empty collection yields no batches and a bad file raises", %{tmp_dir: dir} do
      empty = Path.join(dir, "empty.geojson")
      File.write!(empty, ~s({"type":"FeatureCollection","features":[]}))
      assert empty |> GeoFrame.stream_file() |> Enum.to_list() == []

      junk = Path.join(dir, "junk.json")
      File.write!(junk, ~s({"type":"FeatureCollection","features":[{"type":"Feature"))

      assert_raise ArgumentError, ~r/invalid GeoJSON/, fn ->
        junk |> GeoFrame.stream_file() |> Enum.to_list()
      end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.stream_file(empty, columns: ["id"])
      end
    end
  end

  describe "resource lifetime" do
    test "the stream is lazy: nothing opens before it is enumerated", %{tmp_dir: dir} do
      path = written(dir, "s.parquet")
      before = Native.open_readers()
      _stream = GeoFrame.stream_file(path, batch_size: 5)
      assert Native.open_readers() == before
    end

    for {name, file, opts} <- [
          {"GeoParquet", "s.parquet", []},
          {"FlatGeobuf", "s.fgb", []},
          {"Feather", "s.feather", []},
          {"Shapefile", "s.shp", []},
          {"Shapefile zip", "s.zip", []},
          {"GeoPackage", "s.gpkg", []},
          {"GeoJSON", "s.geojson", []}
        ] do
      test "#{name}: a full run closes the reader", %{tmp_dir: dir} do
        path = written(dir, unquote(file))
        before = Native.open_readers()
        assert path |> GeoFrame.stream_file([batch_size: 5] ++ unquote(opts)) |> Enum.count() == 5
        assert Native.open_readers() == before
      end

      test "#{name}: halting early with Enum.take/2 closes the reader", %{tmp_dir: dir} do
        path = written(dir, unquote(file))
        before = Native.open_readers()
        assert [batch] = path |> GeoFrame.stream_file(batch_size: 5) |> Enum.take(1)
        assert GeoDataFrame.n_rows(batch) == 5
        assert Native.open_readers() == before
      end

      test "#{name}: an error in the consumer closes the reader", %{tmp_dir: dir} do
        path = written(dir, unquote(file))
        before = Native.open_readers()

        assert_raise RuntimeError, "boom", fn ->
          path |> GeoFrame.stream_file(batch_size: 5) |> Enum.each(fn _ -> raise "boom" end)
        end

        assert Native.open_readers() == before
      end

      test "#{name}: the stream can be enumerated twice", %{tmp_dir: dir} do
        path = written(dir, unquote(file))
        stream = GeoFrame.stream_file(path, batch_size: 10)
        assert Enum.count(stream) == 3
        assert Enum.count(stream) == 3
      end
    end

    test "closing a reader twice, or reading after close, is harmless", %{tmp_dir: dir} do
      path = written(dir, "s.parquet")
      {:ok, reader} = Native.open_parquet_reader(path, nil, nil, 5)
      assert Native.close_reader(reader) == :ok
      assert Native.close_reader(reader) == :ok
      assert Native.next_batch(reader) == {:ok, nil}
    end
  end

  describe "errors" do
    test "an unknown extension or driver raises ArgumentError" do
      assert_raise ArgumentError, ~r/cannot infer the format/, fn ->
        GeoFrame.stream_file("a.xyz")
      end

      assert_raise ArgumentError, ~r/unknown driver/, fn ->
        GeoFrame.stream_file("a.parquet", driver: :nope)
      end
    end

    test "invalid options raise when the stream is built", %{tmp_dir: dir} do
      path = written(dir, "s.parquet")

      assert_raise ArgumentError, ~r/batch_size must be a positive integer/, fn ->
        GeoFrame.stream_file(path, batch_size: 0)
      end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.stream_file(path, layer: "x")
      end

      fgb = written(dir, "s.fgb")

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.stream_file(fgb, columns: ["id"])
      end

      assert_raise ArgumentError, ~r/bbox must be/, fn ->
        GeoFrame.stream_file(path, bbox: {1, 2})
      end
    end

    test "a missing or invalid file raises ArgumentError when enumerated", %{tmp_dir: dir} do
      stream = GeoFrame.stream_file(Path.join(dir, "missing.parquet"))
      assert_raise ArgumentError, ~r/cannot open/, fn -> Enum.to_list(stream) end

      junk = Path.join(dir, "junk.fgb")
      File.write!(junk, "not flatgeobuf")
      assert_raise ArgumentError, fn -> junk |> GeoFrame.stream_file() |> Enum.to_list() end
    end
  end
end
