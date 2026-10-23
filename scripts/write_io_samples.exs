# Dev-only: writes sample files with GeoFrame for the GeoPandas cross-check.
#
#     mix run scripts/write_io_samples.exs <output-dir>
#     ~/.venvs/geopandas-ref/bin/python -I test/reference/check_geoframe_io.py <output-dir>
#
# or run both with `scripts/crosscheck_io.sh`. Writes feather, GeoParquet, Arrow IPC stream,
# Shapefile (plain and zipped) and GeoPackage files plus `expected.json` (CRS, WKT and
# attributes of each file).

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
            do:
              {col,
               gdf |> GeoDataFrame.data() |> DataFrame.pull(col) |> Explorer.Series.to_list()}
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

# -- Shapefile and GeoPackage ---------------------------------------------------------------

shapefile = fn opts -> fn gdf, path -> GeoFrame.write_shapefile(gdf, path, opts) end end

gpkg = fn opts -> fn gdf, path -> GeoFrame.write_gpkg(gdf, path, opts) end end

# Same as `write`, but the expected columns are given (field names change on the way) and the
# expectation never reads the file back.
write_vector = fn files, name, gdf, writer, extra ->
  path = Path.join(dir, name)
  :ok = writer.(gdf, path)

  columns =
    extra[:columns] ||
      for(
        col <- DataFrame.names(GeoDataFrame.data(gdf)),
        into: %{},
        do: {col, gdf |> GeoDataFrame.data() |> DataFrame.pull(col) |> Explorer.Series.to_list()}
      )

  entry =
    %{
      "file" => name,
      "kind" => extra[:kind],
      "crs" => GeoDataFrame.crs(gdf),
      "wkt" => extra[:wkt] || gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt(),
      "columns" => columns,
      "native" => false
    }
    |> Map.merge(
      Map.new(Keyword.take(extra, [:layer, :geometry_types]), fn {k, v} ->
        {Atom.to_string(k), v}
      end)
    )

  [entry | files]
end

files = write_vector.(files, "points.shp", points, shapefile.([]), kind: "shapefile")

files =
  write_vector.(
    files,
    "points_3857.shp",
    GeoDataFrame.set_crs(points, "EPSG:3857"),
    shapefile.([]),
    kind: "shapefile"
  )

files =
  write_vector.(
    files,
    "points_utm17n.shp",
    GeoDataFrame.set_crs(points, "EPSG:32617"),
    shapefile.([]),
    kind: "shapefile"
  )

files =
  write_vector.(
    files,
    "points_nad83.shp",
    GeoDataFrame.set_crs(points, "EPSG:4269"),
    shapefile.([]),
    kind: "shapefile"
  )

files =
  write_vector.(files, "points_nocrs.shp", GeoDataFrame.set_crs(points, nil), shapefile.([]),
    kind: "shapefile"
  )

files = write_vector.(files, "points_bundle.zip", points, shapefile.([]), kind: "shapefile")

polys_mixed =
  frame.(
    [
      "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0), (1 1, 2 1, 2 2, 1 1))",
      "MULTIPOLYGON (((10 10, 10 11, 11 11, 10 10)), ((20 20, 20 21, 21 21, 20 20)))",
      nil,
      "POLYGON ((5 5, 5 6, 6 6, 5 5))",
      "POLYGON ((7 7, 7 9, 9 9, 7 7))"
    ],
    "EPSG:4326"
  )

files = write_vector.(files, "polygons.shp", polys_mixed, shapefile.([]), kind: "shapefile")

lines =
  frame.(
    [
      "LINESTRING (0 0, 3 4, 6 4)",
      "MULTILINESTRING ((0 0, 1 1), (5 5, 6 6, 7 5))",
      nil,
      "LINESTRING (10 10, 11 11)",
      "LINESTRING (20 20, 21 22)"
    ],
    "EPSG:4326"
  )

files = write_vector.(files, "lines.shp", lines, shapefile.([]), kind: "shapefile")

multipoints =
  frame.(
    [
      "MULTIPOINT (1 2, 3 4)",
      "MULTIPOINT (5 6)",
      nil,
      "MULTIPOINT (7 8, 9 10, 11 12)",
      "MULTIPOINT (0 0, 1 1)"
    ],
    "EPSG:4326"
  )

files = write_vector.(files, "multipoints.shp", multipoints, shapefile.([]), kind: "shapefile")

latin =
  GeoDataFrame.new(
    DataFrame.new(name: ["café", "ñandú", nil]),
    GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)", "POINT (3 3)"], crs: "EPSG:4326")
  )

files =
  write_vector.(files, "latin1.shp", latin, shapefile.(encoding: "ISO-8859-1"), kind: "shapefile")

long =
  GeoDataFrame.new(
    DataFrame.new(
      population_total: [1, 2],
      population_urban: [3, 4],
      population_rural: [5, 6],
      short: [7, 8]
    ),
    GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)"], crs: "EPSG:4326")
  )

files =
  write_vector.(files, "long_names.shp", long, shapefile.([]),
    kind: "shapefile",
    columns: %{
      "population" => [1, 2],
      "populati_1" => [3, 4],
      "populati_2" => [5, 6],
      "short" => [7, 8]
    }
  )

dates =
  GeoDataFrame.new(
    DataFrame.new(
      day: Explorer.Series.from_list([~D[2020-01-02], nil, ~D[1999-12-31]]),
      stamp: Explorer.Series.from_list([~N[2020-01-02 03:04:05], nil, ~N[2021-03-04 00:00:00]])
    ),
    GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)", "POINT (3 3)"], crs: "EPSG:4326")
  )

files = write_vector.(files, "dates.shp", dates, shapefile.([]), kind: "shapefile")
files = write_vector.(files, "dates.gpkg", dates, gpkg.([]), kind: "gpkg")

files =
  write_vector.(files, "points.gpkg", points, gpkg.(layer: "pts"), kind: "gpkg", layer: "pts")

files =
  write_vector.(
    files,
    "points_3857.gpkg",
    GeoDataFrame.set_crs(points, "EPSG:3857"),
    gpkg.(layer: "pts"),
    kind: "gpkg",
    layer: "pts"
  )

files =
  write_vector.(
    files,
    "points_utm17n.gpkg",
    GeoDataFrame.set_crs(points, "EPSG:32617"),
    gpkg.(layer: "pts"),
    kind: "gpkg",
    layer: "pts"
  )

files =
  write_vector.(
    files,
    "points_nocrs.gpkg",
    GeoDataFrame.set_crs(points, nil),
    gpkg.(layer: "pts"),
    kind: "gpkg",
    layer: "pts"
  )

files =
  write_vector.(files, "points_noindex.gpkg", points, gpkg.(layer: "pts", spatial_index: false),
    kind: "gpkg",
    layer: "pts"
  )

files =
  write_vector.(files, "mixed_types.gpkg", mixed, gpkg.(layer: "mixed"),
    kind: "gpkg",
    layer: "mixed"
  )

files =
  write_vector.(files, "polygons.gpkg", polys_mixed, gpkg.(layer: "polys"),
    kind: "gpkg",
    layer: "polys"
  )

# A two-layer file: both layers are checked.
files =
  write_vector.(files, "multi.gpkg", points, gpkg.(layer: "pts"), kind: "gpkg", layer: "pts")

:ok = GeoFrame.write_gpkg(lines, Path.join(dir, "multi.gpkg"), layer: "lines")

files =
  write_vector.(files, "multi.gpkg", lines, fn _gdf, _path -> :ok end,
    kind: "gpkg",
    layer: "lines"
  )

appended =
  GeoDataFrame.concat_rows([points, points])

files =
  write_vector.(
    files,
    "appended.gpkg",
    appended,
    fn _gdf, path ->
      :ok = GeoFrame.write_gpkg(points, path, layer: "pts")
      GeoFrame.write_gpkg(points, path, layer: "pts", mode: :a)
    end,
    kind: "gpkg",
    layer: "pts"
  )

File.write!(Path.join(dir, "expected.json"), Jason.encode!(Enum.reverse(files), pretty: true))
IO.puts("wrote #{length(files)} files and expected.json to #{dir}")
