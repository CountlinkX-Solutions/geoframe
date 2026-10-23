# Dev-only: writes sample files with GeoFrame for the GeoPandas cross-check.
#
#     mix run scripts/write_io_samples.exs <output-dir>
#     ~/.venvs/geopandas-ref/bin/python -I test/reference/check_geoframe_io.py <output-dir>
#
# or run both with `scripts/crosscheck_io.sh`. Writes feather, GeoParquet and Arrow IPC stream
# files plus `expected.json` (CRS, WKT and attributes of each file).

alias Explorer.DataFrame
alias GeoFrame.GeoDataFrame
alias GeoFrame.GeoSeries

[dir] = System.argv()
File.mkdir_p!(dir)

attrs = fn ->
  DataFrame.new(
    id: [1, 2, 3, nil, 5],
    name: ["a", "b", nil, "d", "e"],
    score: [1.5, nil, 3.25, 4.0, -5.5],
    flag: [true, false, nil, true, false]
  )
end

frame = fn wkt, crs ->
  GeoDataFrame.new(attrs.(), GeoSeries.from_wkt!(wkt, crs: crs))
end

mixed =
  frame.(
    [
      "POINT (1 2)",
      "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0), (1 1, 2 1, 2 2, 1 1))",
      nil,
      "LINESTRING (0 0, 3 4, 6 4)",
      "MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))"
    ],
    "EPSG:4326"
  )

mixed_3857 = GeoDataFrame.set_crs(mixed, "EPSG:3857")
mixed_nocrs = GeoDataFrame.set_crs(mixed, nil)
mixed_utm = GeoDataFrame.set_crs(mixed, "EPSG:32617")

points =
  frame.(["POINT (0 0)", "POINT (1 1)", nil, "POINT (3 3)", "POINT (4 4)"], "EPSG:4326")

polys =
  frame.(
    [
      "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))",
      "MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))",
      nil,
      "POLYGON ((5 5, 6 5, 6 6, 5 5))",
      "POLYGON ((7 7, 9 7, 9 9, 7 7))"
    ],
    "EPSG:4326"
  )

# Native polygon columns hold multipolygons, so rows come back as MULTIPOLYGON: for native
# files the expected geometry is what GeoFrame itself reads back (GeoPandas must agree).
expected_wkt = fn gdf, path, kind, native ->
  source =
    cond do
      not native -> gdf
      kind == "arrows" -> GeoDataFrame.from_arrow!(File.read!(path))
      true -> GeoFrame.read_file!(path)
    end

  source |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()
end

files = []

write = fn files, name, gdf, writer, extra ->
  path = Path.join(dir, name)
  :ok = writer.(gdf, path)

  entry =
    Map.merge(
      %{
        "file" => name,
        "kind" => extra[:kind],
        "crs" => GeoDataFrame.crs(gdf),
        "wkt" => expected_wkt.(gdf, path, extra[:kind], Keyword.get(extra, :native, false)),
        "native" => Keyword.get(extra, :native, false),
        "columns" =>
          for(
            col <- DataFrame.names(GeoDataFrame.data(gdf)),
            into: %{},
            do: {col, gdf |> GeoDataFrame.data() |> DataFrame.pull(col) |> Explorer.Series.to_list()}
          )
      },
      Map.new(Keyword.take(extra, [:covering]), fn {k, v} -> {Atom.to_string(k), v} end)
    )

  [entry | files]
end

feather = fn opts -> fn gdf, path -> GeoFrame.write_feather(gdf, path, opts) end end
parquet = fn opts -> fn gdf, path -> GeoFrame.write_parquet(gdf, path, opts) end end

stream = fn opts ->
  fn gdf, path -> File.write!(path, GeoDataFrame.to_arrow(gdf, opts)) end
end

files =
  for compression <- [:lz4, :zstd, :uncompressed], reduce: files do
    acc ->
      write.(acc, "wkb_#{compression}.feather", mixed, feather.(compression: compression),
        kind: "feather"
      )
  end

files = write.(files, "wkb_3857.feather", mixed_3857, feather.([]), kind: "feather")
files = write.(files, "wkb_utm17n.feather", mixed_utm, feather.([]), kind: "feather")
files = write.(files, "wkb_nocrs.feather", mixed_nocrs, feather.([]), kind: "feather")

files =
  write.(files, "geoarrow_points.feather", points, feather.(geometry_encoding: :geoarrow),
    kind: "feather",
    native: true
  )

files =
  write.(files, "geoarrow_polygons.feather", polys, feather.(geometry_encoding: :geoarrow),
    kind: "feather",
    native: true
  )

files =
  for compression <- [:snappy, :zstd, :gzip, :lz4, :brotli, :none], reduce: files do
    acc ->
      write.(acc, "wkb_#{compression}.parquet", mixed, parquet.(compression: compression),
        kind: "parquet"
      )
  end

files =
  write.(
    files,
    "wkb_covering.parquet",
    mixed,
    parquet.(write_covering_bbox: true, row_group_size: 2),
    kind: "parquet",
    covering: true
  )

files =
  write.(files, "wkb_v1_0_0.parquet", mixed, parquet.(schema_version: "1.0.0"), kind: "parquet")

files = write.(files, "wkb_3857.parquet", mixed_3857, parquet.([]), kind: "parquet")
files = write.(files, "wkb_utm17n.parquet", mixed_utm, parquet.([]), kind: "parquet")
files = write.(files, "wkb_nocrs.parquet", mixed_nocrs, parquet.([]), kind: "parquet")

files =
  write.(files, "geoarrow_points.parquet", points, parquet.(geometry_encoding: :geoarrow),
    kind: "parquet",
    native: true
  )

files =
  write.(
    files,
    "geoarrow_points_covering.parquet",
    points,
    parquet.(geometry_encoding: :geoarrow, write_covering_bbox: true, row_group_size: 2),
    kind: "parquet",
    native: true,
    covering: true
  )

files =
  write.(files, "geoarrow_polygons.parquet", polys, parquet.(geometry_encoding: :geoarrow),
    kind: "parquet",
    native: true
  )

files = write.(files, "wkb.arrows", mixed, stream.([]), kind: "arrows")

files =
  write.(files, "geoarrow_points.arrows", points, stream.(geometry_encoding: :geoarrow),
    kind: "arrows",
    native: true
  )

files =
  write.(files, "geoarrow_polygons.arrows", polys, stream.(geometry_encoding: :geoarrow),
    kind: "arrows",
    native: true
  )

File.write!(Path.join(dir, "expected.json"), Jason.encode!(Enum.reverse(files), pretty: true))
IO.puts("wrote #{length(files)} files and expected.json to #{dir}")
