defmodule GeoFrame.ReadFileRowsTest do
  # The open-reader counter is VM wide: run apart from the async modules.
  use ExUnit.Case, async: false

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @moduletag :tmp_dir

  @rows 25

  @files [
    {"GeoParquet", "s.parquet", [row_group_size: 4]},
    {"FlatGeobuf", "s.fgb", []},
    {"Feather", "s.feather", []},
    {"Shapefile", "s.shp", []},
    {"Shapefile zip", "s.zip", []},
    {"GeoPackage", "s.gpkg", []},
    {"GeoJSON", "s.geojson", [force: true]}
  ]

  # `rows:` value and the `{offset, length}` slice of the whole read it must equal.
  @windows [
    {10, {0, 10}},
    {0, {0, 0}},
    {1, {0, 1}},
    {100, {0, 25}},
    {5..14, {5, 10}},
    {0..9, {0, 10}},
    {20..40, {20, 5}},
    {24..24, {24, 1}},
    {30..40, {30, 0}},
    {4..2//1, {4, 0}},
    {3..3, {3, 1}}
  ]

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

  defp written(dir, name, opts) do
    path = Path.join(dir, name)
    :ok = GeoFrame.write_file(sample(@rows), path, opts)
    path
  end

  defp assert_same_frame(actual, expected) do
    assert actual.geometry_column == expected.geometry_column
    assert GeoDataFrame.crs(actual) == GeoDataFrame.crs(expected)
    assert GeoDataFrame.n_rows(actual) == GeoDataFrame.n_rows(expected)

    assert DataFrame.dtypes(GeoDataFrame.data(actual)) ==
             DataFrame.dtypes(GeoDataFrame.data(expected))

    for name <- DataFrame.names(GeoDataFrame.data(expected)) do
      assert col(actual, name) == col(expected, name)
    end

    assert wkt(actual) == wkt(expected)
  end

  defp col(gdf, name), do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()
  defp wkt(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()

  for {label, file, write_opts} <- @files do
    describe "#{label}" do
      test "rows: equals slicing the whole read", %{tmp_dir: dir} do
        path = written(dir, unquote(file), unquote(write_opts))
        whole = GeoFrame.read_file!(path)

        for {rows, {offset, length}} <- @windows do
          {:ok, actual} = GeoFrame.read_file(path, rows: rows)

          assert_same_frame(actual, GeoDataFrame.slice(whole, offset, length))
        end
      end

      test "rows: releases the reader, also when the read fails", %{tmp_dir: dir} do
        path = written(dir, unquote(file), unquote(write_opts))
        before = Native.open_readers()
        assert {:ok, _} = GeoFrame.read_file(path, rows: 3)
        assert {:ok, _} = GeoFrame.read_file(path, rows: 3..5)
        assert Native.open_readers() == before
      end
    end
  end

  describe "rows: with other options" do
    test "applies after bbox:", %{tmp_dir: dir} do
      for {file, opts} <- [
            {"s.parquet", []},
            {"s.fgb", []},
            {"s.shp", []},
            {"s.gpkg", []},
            {"s.geojson", [force: true]}
          ] do
        path = written(dir, file, opts)
        bbox = {3, 0, 17, 100}

        whole =
          path |> GeoFrame.stream_file(bbox: bbox) |> Enum.to_list() |> GeoDataFrame.concat_rows()

        {:ok, actual} = GeoFrame.read_file(path, bbox: bbox, rows: 4..8)
        assert_same_frame(actual, GeoDataFrame.slice(whole, 4, 5))

        # A bbox that matches nothing leaves an empty frame with the file's schema.
        {:ok, none} = GeoFrame.read_file(path, bbox: {500, 500, 600, 600}, rows: 3)
        assert GeoDataFrame.n_rows(none) == 0
        assert GeoDataFrame.crs(none) == GeoDataFrame.crs(whole)
      end
    end

    test "columns: and layer: reach the reader", %{tmp_dir: dir} do
      parquet = written(dir, "c.parquet", [])
      {:ok, gdf} = GeoFrame.read_file(parquet, columns: ["name"], rows: 2..4)
      assert DataFrame.names(GeoDataFrame.data(gdf)) == ["name"]
      assert col(gdf, "name") == ["n2", "n3", "n4"]

      gpkg = written(dir, "l.gpkg", layer: "first")

      :ok =
        GeoFrame.write_file(GeoDataFrame.head(sample(@rows), 6), gpkg, layer: "second", mode: :a)

      {:ok, gdf} = GeoFrame.read_file(gpkg, layer: "second", rows: 4..9)
      assert col(gdf, "id") == [4, 5]
    end

    test "rows: nil reads everything", %{tmp_dir: dir} do
      path = written(dir, "n.parquet", [])
      assert_same_frame(GeoFrame.read_file!(path, rows: nil), GeoFrame.read_file!(path))
    end

    test "a file with no rows gives an empty frame", %{tmp_dir: dir} do
      path = Path.join(dir, "e.parquet")
      :ok = GeoFrame.write_file(GeoDataFrame.head(sample(3), 0), path)
      {:ok, gdf} = GeoFrame.read_file(path, rows: 5)
      assert GeoDataFrame.n_rows(gdf) == 0
    end
  end

  describe "errors" do
    test "invalid rows: raise ArgumentError", %{tmp_dir: dir} do
      path = written(dir, "s.parquet", [])

      for rows <- [-1, 1.5, "3", :all, 1..10//2, 10..1//-1, -3..5, [1, 2]] do
        assert_raise ArgumentError, ~r/rows must be/, fn ->
          GeoFrame.read_file(path, rows: rows)
        end
      end
    end

    test "unknown options still raise, and read failures are error tuples", %{tmp_dir: dir} do
      path = written(dir, "s.parquet", [])

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.read_file(path, rows: 3, layer: "x")
      end

      assert {:error, message} = GeoFrame.read_file(Path.join(dir, "missing.parquet"), rows: 3)
      assert message =~ "cannot open"

      junk = Path.join(dir, "junk.fgb")
      File.write!(junk, "not flatgeobuf")
      assert {:error, _} = GeoFrame.read_file(junk, rows: 3)

      assert_raise ArgumentError, ~r/rows must be/, fn ->
        GeoFrame.read_file!(path, rows: -2)
      end

      assert_raise ArgumentError, fn ->
        GeoFrame.read_file!(Path.join(dir, "missing.parquet"), rows: 3)
      end
    end
  end
end
