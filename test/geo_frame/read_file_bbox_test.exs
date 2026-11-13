defmodule GeoFrame.ReadFileBboxTest do
  # `read_file(bbox:)` for every format that GeoPandas filters (pyogrio / GDAL): the features
  # whose GEOMETRY intersects the box, not just their envelope. The open-reader counter is VM
  # wide: run apart from the async modules.
  use ExUnit.Case, async: false

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @moduletag :tmp_dir

  # id => geometry. Several have an envelope that meets a box while the geometry does not: 0 (a
  # diagonal), 5 (an L), 11 (the other diagonal), 12 and 13 sit around the box (6 0 9 3) or (6 3 9 9).
  @geometries [
    {0, "LINESTRING (0 0, 10 10)"},
    {1, "POINT (7 1)"},
    {2, "POINT (6 3)"},
    {3, "POINT (20 20)"},
    {4, "POLYGON ((0 0, 10 0, 10 1, 1 1, 1 10, 0 10, 0 0))"},
    {5, "POLYGON ((0 0, 0 10, 5 10, 5 5, 10 5, 10 0, 0 0))"},
    {6, "LINESTRING (5.9 0, 5.9 3)"},
    {7, "LINESTRING (9.5 -1, 9.5 5)"},
    {8, "LINESTRING (6 -5, 6 -1)"},
    {9, "POLYGON ((6.5 0.5, 8 0.5, 8 2, 6.5 2, 6.5 0.5))"},
    {10, "POINT EMPTY"},
    {11, "LINESTRING (0 10, 10 0)"},
    {12, "POLYGON ((7 4, 12 4, 12 8, 7 8, 7 4))"},
    {13, "POLYGON ((6 3, 6.5 3, 6.5 3.5, 6 3.5, 6 3))"},
    {14, nil}
  ]

  @boxes [
    {6, 0, 9, 3},
    {6, 3, 9, 9},
    {0, 0, 0.5, 0.5},
    {100, 100, 101, 101},
    {-1, -1, 11, 11},
    {6, 0, 6, 3}
  ]

  # GeoPandas 1.2.0 / pyogrio 0.13.0 / GDAL 3.12.4, ids of `read_file(path, bbox=box)`:
  #
  #     g = gpd.GeoDataFrame({"id": ids, "name": names}, geometry=geoms, crs=4326)   # the table above
  #     g.to_file("t.gpkg", driver="GPKG"); g.to_file("t.geojson", driver="GeoJSON")
  #     g[g.geom_type == "LineString"].to_file("lines.shp")   # likewise polys.shp, pts.shp
  #     list(gpd.read_file(path, bbox=box)["id"])
  #
  # (a FlatGeobuf file of the rows with a geometry gives the same ids as the GeoPackage).
  @gdal %{
    {6, 0, 9, 3} => %{
      mixed: [1, 2, 4, 5, 9, 11, 13],
      lines: [11],
      polys: [4, 5, 9, 13],
      pts: [1, 2]
    },
    {6, 3, 9, 9} => %{mixed: [0, 2, 5, 11, 12, 13], lines: [0, 11], polys: [5, 12, 13], pts: [2]},
    {0, 0, 0.5, 0.5} => %{mixed: [0, 4, 5], lines: [0], polys: [4, 5], pts: []},
    {100, 100, 101, 101} => %{mixed: [], lines: [], polys: [], pts: []},
    {-1, -1, 11, 11} => %{
      mixed: [0, 1, 2, 4, 5, 6, 7, 8, 9, 11, 12, 13],
      lines: [0, 6, 7, 8, 11],
      polys: [4, 5, 9, 12, 13],
      pts: [1, 2]
    },
    {6, 0, 6, 3} => %{mixed: [2, 4, 5, 13], lines: [], polys: [4, 5, 13], pts: [2]}
  }

  defp frame(rows) do
    data =
      DataFrame.new(
        id: Enum.map(rows, &elem(&1, 0)),
        name: Enum.map(rows, &"n#{elem(&1, 0)}")
      )

    GeoDataFrame.new(data, GeoSeries.from_wkt!(Enum.map(rows, &elem(&1, 1)), crs: "EPSG:4326"))
  end

  defp ids(gdf), do: gdf |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list()

  defp of_type(prefix),
    do: Enum.filter(@geometries, fn {_, w} -> w && String.starts_with?(w, prefix) end)

  defp write!(rows, path, opts \\ []) do
    :ok = GeoFrame.write_file(frame(rows), path, opts)
    path
  end

  # Every file with the rows it holds and the key of its expected ids.
  defp files(dir) do
    with_geometry = Enum.filter(@geometries, fn {_, w} -> w && w != "POINT EMPTY" end)

    [
      {"GeoPackage", write!(@geometries, Path.join(dir, "t.gpkg")), :mixed},
      {"GeoJSON", write!(@geometries, Path.join(dir, "t.geojson")), :mixed},
      {"FlatGeobuf", write!(with_geometry, Path.join(dir, "t.fgb")), :mixed},
      {"Shapefile lines", write!(of_type("LINESTRING"), Path.join(dir, "lines.shp")), :lines},
      {"Shapefile polygons", write!(of_type("POLYGON"), Path.join(dir, "polys.shp")), :polys},
      {"Shapefile points", write!(of_type("POINT ("), Path.join(dir, "pts.shp")), :pts}
    ]
  end

  test "bbox keeps the features whose geometry intersects it, as GeoPandas", %{tmp_dir: dir} do
    for {format, path, key} <- files(dir), box <- @boxes do
      expected = @gdal |> Map.fetch!(box) |> Map.fetch!(key)
      read = GeoFrame.read_file!(path, bbox: box)

      assert Enum.sort(ids(read)) == expected, "#{format} #{inspect(box)}"
      # The empty result still has the schema and CRS.
      assert read |> GeoDataFrame.data() |> DataFrame.names() |> Enum.sort() == ["id", "name"]
      assert read.geometry_column == "geometry"
      assert GeoDataFrame.crs(read) == GeoDataFrame.crs(GeoFrame.read_file!(path))
    end
  end

  test "bbox from the stream and from read_file agree", %{tmp_dir: dir} do
    for {format, path, _key} <- files(dir), box <- @boxes do
      streamed = path |> GeoFrame.stream_file(batch_size: 3, bbox: box) |> Enum.flat_map(&ids/1)
      assert streamed == ids(GeoFrame.read_file!(path, bbox: box)), "#{format} #{inspect(box)}"
    end
  end

  test "bbox applies before rows", %{tmp_dir: dir} do
    path = write!(@geometries, Path.join(dir, "r.gpkg"))
    box = {6, 0, 9, 3}
    kept = ids(GeoFrame.read_file!(path, bbox: box))
    assert kept == [1, 2, 4, 5, 9, 11, 13]
    assert path |> GeoFrame.read_file!(bbox: box, rows: 3) |> ids() == Enum.take(kept, 3)
    assert path |> GeoFrame.read_file!(bbox: box, rows: 2..4) |> ids() == [4, 5, 9]
  end

  test "bbox with the layer option", %{tmp_dir: dir} do
    path = write!(@geometries, Path.join(dir, "c.gpkg"), layer: "pieces")
    read = GeoFrame.read_file!(path, bbox: {6, 0, 9, 3}, layer: "pieces")
    assert ids(read) == [1, 2, 4, 5, 9, 11, 13]
  end

  test "no reader is left open", %{tmp_dir: dir} do
    before = Native.open_readers()

    for {_format, path, _key} <- files(dir) do
      GeoFrame.read_file!(path, bbox: {6, 0, 9, 3})
    end

    assert Native.open_readers() == before
  end

  test "a bad bbox raises", %{tmp_dir: dir} do
    for {format, path, _key} <- files(dir) do
      assert_raise ArgumentError, fn -> GeoFrame.read_file!(path, bbox: {1, 2, 3}) end

      # FlatGeobuf (as before this change) takes an inverted box as one that matches nothing.
      if format != "FlatGeobuf" do
        for bad <- [{5, 0, 1, 1}, {0, 5, 1, 1}] do
          assert_raise ArgumentError, fn -> GeoFrame.read_file!(path, bbox: bad) end
        end
      end
    end
  end

  test "Feather has no bbox (GeoPandas read_feather has none, and pyogrio has no Arrow driver)",
       %{tmp_dir: dir} do
    path = write!(@geometries, Path.join(dir, "t.feather"))
    assert_raise ArgumentError, ~r/bbox/, fn -> GeoFrame.read_file!(path, bbox: {0, 0, 1, 1}) end
  end

  test "GeoParquet keeps envelope semantics (the covering box, as GeoPandas read_parquet)",
       %{tmp_dir: dir} do
    path =
      write!(@geometries, Path.join(dir, "t.parquet"), write_covering_bbox: true)

    # Row 0's envelope meets the box although the line does not.
    assert 0 in ids(GeoFrame.read_file!(path, bbox: {6, 0, 9, 3}))
  end
end
