defmodule GeoFrame.FeatherRecordBatchesTest do
  # `write_feather(record_batch_size:)` and `to_arrow(record_batch_size:)`: rows per Arrow record
  # batch (pyarrow's `write_feather(chunksize=65536)`, which GeoPandas `to_feather` goes through).
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @rows 10

  defp sample(rows \\ @rows) do
    data = DataFrame.new(id: Enum.to_list(0..(rows - 1)))
    wkt = Enum.map(0..(rows - 1), &"POINT (#{&1} #{&1 * 2})")
    GeoDataFrame.new(data, GeoSeries.from_wkt!(wkt, crs: "EPSG:4326"))
  end

  defp ids(gdf), do: gdf |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list()

  # Messages of an uncompressed IPC stream start with the 0xFFFFFFFF continuation marker: the
  # schema, one per record batch, then the end-of-stream marker.
  defp record_batches(stream) do
    markers = length(:binary.matches(stream, <<0xFF, 0xFF, 0xFF, 0xFF>>))
    markers - 2
  end

  test "to_arrow splits the rows into record batches of the requested size" do
    for {size, batches} <- [{3, 4}, {10, 1}, {1000, 1}, {1, 10}] do
      bytes = GeoDataFrame.to_arrow(sample(), record_batch_size: size)
      assert record_batches(bytes) == batches, "size #{size}"
      assert bytes |> GeoDataFrame.from_arrow!() |> ids() == Enum.to_list(0..(@rows - 1))
    end
  end

  test "the default keeps small frames in one record batch and round-trips large ones" do
    assert record_batches(GeoDataFrame.to_arrow(sample())) == 1
    # Beyond 65 536 rows the default splits (the batch count is checked on the file with pyarrow
    # and in the Rust tests; the marker scan is only reliable for small bitmaps).
    big = sample(65_537)
    assert big |> GeoDataFrame.to_arrow() |> GeoDataFrame.from_arrow!() |> ids() == ids(big)
  end

  test "write_feather with small record batches reads back identically", %{tmp_dir: dir} do
    for {name, opts} <- [
          {"a.feather", []},
          {"b.arrow", [compression: :zstd]},
          {"c.feather", [geometry_encoding: :geoarrow, compression: :uncompressed]}
        ] do
      path = Path.join(dir, name)
      assert :ok = GeoFrame.write_feather(sample(), path, [record_batch_size: 3] ++ opts)
      gdf = GeoFrame.read_feather!(path)
      assert ids(gdf) == Enum.to_list(0..(@rows - 1))
      assert GeoSeries.to_wkt(GeoDataFrame.geometry(gdf)) |> Enum.at(4) == "POINT(4 8)"
      assert GeoSeries.crs(GeoDataFrame.geometry(gdf)) == "EPSG:4326"

      # Streaming the file works with any batch size, and is bounded by the record batches.
      streamed = path |> GeoFrame.stream_file(batch_size: 4) |> Enum.flat_map(&ids/1)
      assert streamed == Enum.to_list(0..(@rows - 1))
    end
  end

  test "an empty frame is written whatever the record batch size", %{tmp_dir: dir} do
    path = Path.join(dir, "empty.feather")
    empty = sample() |> GeoDataFrame.head(0)
    assert :ok = GeoFrame.write_feather(empty, path, record_batch_size: 2)
    assert path |> GeoFrame.read_feather!() |> ids() == []
  end

  test "an invalid record_batch_size raises and leaves no file", %{tmp_dir: dir} do
    path = Path.join(dir, "bad.feather")

    for bad <- [0, -1, 1.5, "10", :all] do
      assert_raise ArgumentError, ~r/record_batch_size/, fn ->
        GeoFrame.write_feather(sample(), path, record_batch_size: bad)
      end

      assert_raise ArgumentError, ~r/record_batch_size/, fn ->
        GeoDataFrame.to_arrow(sample(), record_batch_size: bad)
      end
    end

    refute File.exists?(path)
  end

  test "write_file passes record_batch_size to Feather and rejects it elsewhere", %{tmp_dir: dir} do
    path = Path.join(dir, "w.feather")
    assert :ok = GeoFrame.write_file(sample(), path, record_batch_size: 4)
    assert path |> GeoFrame.read_file!() |> ids() == Enum.to_list(0..(@rows - 1))

    assert_raise ArgumentError, fn ->
      GeoFrame.write_file(sample(), Path.join(dir, "x.parquet"), record_batch_size: 4)
    end
  end
end
