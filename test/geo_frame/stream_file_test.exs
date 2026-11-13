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

  describe "resource lifetime" do
    test "the stream is lazy: nothing opens before it is enumerated", %{tmp_dir: dir} do
      path = written(dir, "s.parquet")
      before = Native.open_readers()
      _stream = GeoFrame.stream_file(path, batch_size: 5)
      assert Native.open_readers() == before
    end

    for {name, file, opts} <- [{"GeoParquet", "s.parquet", []}, {"FlatGeobuf", "s.fgb", []}] do
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
    test "formats that cannot stream yet raise ArgumentError naming the format", %{tmp_dir: dir} do
      for file <- ["a.geojson", "a.shp", "a.gpkg", "a.feather", "a.zip"] do
        assert_raise ArgumentError, ~r/stream_file\/2 does not support/, fn ->
          GeoFrame.stream_file(Path.join(dir, file))
        end
      end
    end

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
