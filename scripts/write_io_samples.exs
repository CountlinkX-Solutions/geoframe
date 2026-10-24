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

# Projected CRS that GeoPandas can only read when the PROJJSON is complete (issue #88).
files =
  for {code, name} <- [
        {3035, "laea_europe"},
        {32633, "utm33n"},
        {4326, "wgs84"},
        # Geocentric, 3D geographic, datum-shifted and compound CRS (issue #50).
        {3822, "twd97_geocentric"},
        {4480, "cgcs2000_3d"},
        {3823, "twd97_bound"},
        {5707, "lambert_i_ngf"},
        {5498, "nad83_navd88"}
      ],
      reduce: files do
    acc ->
      gdf = GeoDataFrame.set_crs(mixed, "EPSG:#{code}")
      write.(acc, "wkb_#{name}.parquet", gdf, parquet.([]), kind: "parquet")
  end

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
      "wkt" =>
        extra[:wkt] || expected_wkt.(gdf, path, extra[:kind], Keyword.get(extra, :native, false)),
      "columns" => columns,
      "native" => false
    }
    |> Map.merge(
      Map.new(Keyword.take(extra, [:layer, :geometry_types, :zm, :native, :covering]), fn {k, v} ->
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

# -- Z and M (batch Z4) -------------------------------------------------------------------------
# GeoPandas reads the Z files with `read_file` / `read_parquet`; the M files, which Shapely-based
# readers cannot load from GDAL, are checked through pyogrio's Arrow path (see the checker).

zpoints =
  frame.(
    ["POINT Z (1 2 3)", "POINT Z (-80.5 25.25 -1.5)", nil, "POINT Z (3 4 5)", "POINT Z (0 0 0)"],
    "EPSG:4326"
  )

zlines =
  frame.(
    [
      "LINESTRING Z (0 0 1, 3 4 2, 6 4 3)",
      "MULTILINESTRING Z ((0 0 1, 1 1 2), (5 5 3, 6 6 4, 7 5 5))",
      nil,
      "LINESTRING Z (10 10 1, 11 11 2)",
      "LINESTRING Z (20 20 1, 21 22 9)"
    ],
    "EPSG:4326"
  )

zpolys =
  frame.(
    [
      "POLYGON Z ((0 0 1, 0 4 2, 4 4 3, 4 0 4, 0 0 1), (1 1 5, 2 1 5, 2 2 5, 1 1 5))",
      "MULTIPOLYGON Z (((10 10 7, 10 11 7, 11 11 7, 10 10 7)), ((20 20 8, 20 21 8, 21 21 8, 20 20 8)))",
      nil,
      "POLYGON Z ((5 5 1, 5 6 1, 6 6 1, 5 5 1))",
      "POLYGON Z ((7 7 2, 7 9 2, 9 9 2, 7 7 2))"
    ],
    "EPSG:4326"
  )

# One geometry type per layer: pyogrio cannot list a GeoPackage or FlatGeobuf whose type is
# "Unknown with Z" (GDAL itself reads it), which is what a Line/MultiLine mix would be.
zlines_single =
  frame.(
    [
      "LINESTRING Z (0 0 1, 3 4 2, 6 4 3)",
      "LINESTRING Z (0 0 1, 1 1 2)",
      nil,
      "LINESTRING Z (10 10 1, 11 11 2)",
      "LINESTRING Z (20 20 1, 21 22 9)"
    ],
    "EPSG:4326"
  )

zpolys_single =
  frame.(
    [
      "POLYGON Z ((0 0 1, 0 4 2, 4 4 3, 4 0 4, 0 0 1), (1 1 5, 2 1 5, 2 2 5, 1 1 5))",
      "POLYGON Z ((10 10 7, 10 11 7, 11 11 7, 10 10 7))",
      nil,
      "POLYGON Z ((5 5 1, 5 6 1, 6 6 1, 5 5 1))",
      "POLYGON Z ((7 7 2, 7 9 2, 9 9 2, 7 7 2))"
    ],
    "EPSG:4326"
  )

zmpolys_single =
  frame.(
    [
      "POLYGON ZM ((0 0 1 5, 0 4 2 6, 4 4 3 7, 4 0 4 8, 0 0 1 5), (1 1 5 1, 2 1 5 2, 2 2 5 3, 1 1 5 1))",
      "POLYGON ZM ((10 10 7 1, 10 11 7 2, 11 11 7 3, 10 10 7 1))",
      nil,
      "POLYGON ZM ((5 5 1 1, 5 6 1 2, 6 6 1 3, 5 5 1 1))",
      "POLYGON ZM ((7 7 2 1, 7 9 2 2, 9 9 2 3, 7 7 2 1))"
    ],
    "EPSG:4326"
  )

zmultipoints =
  frame.(
    [
      "MULTIPOINT Z ((1 2 3), (3 4 5))",
      "MULTIPOINT Z ((5 6 7))",
      nil,
      "MULTIPOINT Z ((7 8 9), (9 10 11), (11 12 13))",
      "MULTIPOINT Z ((0 0 1), (1 1 2))"
    ],
    "EPSG:4326"
  )

mpoints =
  frame.(
    ["POINT M (1 2 3)", "POINT M (-80.5 25.25 7.5)", nil, "POINT M (3 4 5)", "POINT M (0 0 0)"],
    "EPSG:4326"
  )

mlines =
  frame.(
    [
      "LINESTRING M (0 0 1, 3 4 2, 6 4 3)",
      "MULTILINESTRING M ((0 0 1, 1 1 2), (5 5 3, 6 6 4, 7 5 5))",
      nil,
      "LINESTRING M (10 10 1, 11 11 2)",
      "LINESTRING M (20 20 1, 21 22 9)"
    ],
    "EPSG:4326"
  )

zmpolys =
  frame.(
    [
      "POLYGON ZM ((0 0 1 5, 0 4 2 6, 4 4 3 7, 4 0 4 8, 0 0 1 5), (1 1 5 1, 2 1 5 2, 2 2 5 3, 1 1 5 1))",
      "MULTIPOLYGON ZM (((10 10 7 1, 10 11 7 2, 11 11 7 3, 10 10 7 1)), ((20 20 8 4, 20 21 8 5, 21 21 8 6, 20 20 8 4)))",
      nil,
      "POLYGON ZM ((5 5 1 1, 5 6 1 2, 6 6 1 3, 5 5 1 1))",
      "POLYGON ZM ((7 7 2 1, 7 9 2 2, 9 9 2 3, 7 7 2 1))"
    ],
    "EPSG:4326"
  )

zmpoints =
  frame.(
    [
      "POINT ZM (1 2 3 4)",
      "POINT ZM (-80.5 25.25 -1.5 9)",
      nil,
      "POINT ZM (3 4 5 6)",
      "POINT ZM (0 0 0 0)"
    ],
    "EPSG:4326"
  )

# Rows of different dimensions: GeoPackage and GeoJSON keep each row's own.
zmixed =
  frame.(
    ["POINT Z (1 2 3)", "POINT (4 5)", nil, "POINT Z (6 7 8)", "POINT (9 10)"],
    "EPSG:4326"
  )

zm_flags = fn gdf ->
  geometry = GeoDataFrame.geometry(gdf)

  %{
    "has_z" => geometry |> GeoSeries.has_z() |> Explorer.Series.to_list(),
    "has_m" => geometry |> GeoSeries.has_m() |> Explorer.Series.to_list()
  }
end

zm_vector = fn files, name, gdf, writer, kind, opts ->
  write_vector.(
    files,
    name,
    gdf,
    writer,
    Keyword.merge([kind: kind, zm: zm_flags.(gdf)], opts)
  )
end

# GeoJSON has no CRS member and no M.
geojson = fn opts -> fn gdf, path -> GeoFrame.write_geojson(gdf, path, opts) end end
flatgeobuf = fn opts -> fn gdf, path -> GeoFrame.write_flatgeobuf(gdf, path, opts) end end

shapefile_cases = [
  {"z_points.shp", zpoints},
  {"z_lines.shp", zlines},
  {"z_polygons.shp", zpolys},
  {"z_multipoints.shp", zmultipoints},
  {"m_points.shp", mpoints},
  {"m_lines.shp", mlines},
  {"zm_polygons.shp", zmpolys},
  {"zm_points.shp", zmpoints}
]

files =
  for {name, gdf} <- shapefile_cases, reduce: files do
    acc -> zm_vector.(acc, name, gdf, shapefile.([]), "shapefile", [])
  end

files = zm_vector.(files, "z_points_bundle.zip", zpoints, shapefile.([]), "shapefile", [])

files =
  for {name, gdf, layer} <- [
        {"z_points.gpkg", zpoints, "pts"},
        {"z_lines.gpkg", zlines_single, "lines"},
        {"z_polygons.gpkg", zpolys_single, "polys"},
        {"m_points.gpkg", mpoints, "pts"},
        {"zm_polygons.gpkg", zmpolys_single, "polys"},
        {"zm_points.gpkg", zmpoints, "pts"},
        {"z_mixed.gpkg", zmixed, "mixed"}
      ],
      reduce: files do
    acc -> zm_vector.(acc, name, gdf, gpkg.(layer: layer), "gpkg", layer: layer)
  end

# FlatGeobuf: one geometry type per file, and one dimension (a mixed file is promoted with NaN).
fgb_frames = [
  {"z_points.fgb",
   frame.(
     [
       "POINT Z (1 2 3)",
       "POINT Z (-80.5 25.25 -1.5)",
       "POINT Z (2 2 2)",
       "POINT Z (3 4 5)",
       "POINT Z (0 0 0)"
     ],
     "EPSG:4326"
   )},
  {"z_polygons.fgb",
   frame.(
     [
       "POLYGON Z ((0 0 1, 0 4 2, 4 4 3, 4 0 4, 0 0 1), (1 1 5, 2 1 5, 2 2 5, 1 1 5))",
       "POLYGON Z ((10 10 7, 10 11 7, 11 11 7, 10 10 7))",
       "POLYGON Z ((12 12 7, 12 13 7, 13 13 7, 12 12 7))",
       "POLYGON Z ((5 5 1, 5 6 1, 6 6 1, 5 5 1))",
       "POLYGON Z ((7 7 2, 7 9 2, 9 9 2, 7 7 2))"
     ],
     "EPSG:4326"
   )},
  {"z_lines.fgb",
   frame.(
     [
       "LINESTRING Z (0 0 1, 3 4 2, 6 4 3)",
       "LINESTRING Z (0 0 1, 1 1 2)",
       "LINESTRING Z (3 3 1, 4 4 2)",
       "LINESTRING Z (10 10 1, 11 11 2)",
       "LINESTRING Z (20 20 1, 21 22 9)"
     ],
     "EPSG:4326"
   )},
  {"m_points.fgb",
   frame.(
     [
       "POINT M (1 2 3)",
       "POINT M (-80.5 25.25 7.5)",
       "POINT M (2 2 2)",
       "POINT M (3 4 5)",
       "POINT M (0 0 0)"
     ],
     "EPSG:4326"
   )},
  {"zm_points.fgb",
   frame.(
     [
       "POINT ZM (1 2 3 4)",
       "POINT ZM (-80.5 25.25 -1.5 9)",
       "POINT ZM (2 2 2 2)",
       "POINT ZM (3 4 5 6)",
       "POINT ZM (0 0 0 0)"
     ],
     "EPSG:4326"
   )},
  {"zm_polygons.fgb",
   frame.(
     [
       "POLYGON ZM ((0 0 1 5, 0 4 2 6, 4 4 3 7, 4 0 4 8, 0 0 1 5), (1 1 5 1, 2 1 5 2, 2 2 5 3, 1 1 5 1))",
       "POLYGON ZM ((10 10 7 1, 10 11 7 2, 11 11 7 3, 10 10 7 1))",
       "POLYGON ZM ((12 12 7 1, 12 13 7 2, 13 13 7 3, 12 12 7 1))",
       "POLYGON ZM ((5 5 1 1, 5 6 1 2, 6 6 1 3, 5 5 1 1))",
       "POLYGON ZM ((7 7 2 1, 7 9 2 2, 9 9 2 3, 7 7 2 1))"
     ],
     "EPSG:4326"
   )}
]

files =
  for {name, gdf} <- fgb_frames, reduce: files do
    acc -> zm_vector.(acc, name, gdf, flatgeobuf.(index: false), "flatgeobuf", [])
  end

# A null row is a feature without a geometry, which GDAL reads as null (issue #78). Without
# an index so the rows keep their order.
files = write.(files, "null_rows.fgb", points, flatgeobuf.(index: false), kind: "flatgeobuf")

files = zm_vector.(files, "z_features.geojson", zlines, geojson.([]), "geojson", [])
files = zm_vector.(files, "z_mixed.geojson", zmixed, geojson.([]), "geojson", [])

files =
  for {name, gdf} <- [
        {"z_wkb.parquet", zpolys},
        {"z_mixed_wkb.parquet", zmixed},
        {"zm_wkb.parquet", zmpolys},
        {"m_wkb.parquet", mlines}
      ],
      reduce: files do
    acc -> zm_vector.(acc, name, gdf, parquet.([]), "parquet", [])
  end

# GeoPandas reads native Z; its native M support is not relied on here, so M stays WKB.
files =
  for {name, gdf, extra} <- [
        {"z_native_points.parquet", zpoints, []},
        {"z_native_polygons.parquet", zpolys, []},
        {"z_native_lines.parquet", zlines, []},
        {"z_native_covering.parquet", zpoints, covering: true}
      ],
      reduce: files do
    acc ->
      writer =
        parquet.(
          [geometry_encoding: :geoarrow, row_group_size: 2] ++
            if(extra[:covering], do: [write_covering_bbox: true], else: [])
        )

      zm_vector.(acc, name, gdf, writer, "parquet", [native: true] ++ extra)
  end

files = zm_vector.(files, "z_wkb.feather", zpoints, feather.([]), "feather", [])

files =
  zm_vector.(
    files,
    "z_native.feather",
    zpoints,
    feather.(geometry_encoding: :geoarrow),
    "feather",
    native: true
  )

files =
  zm_vector.(
    files,
    "z_native.arrows",
    zpolys,
    stream.(geometry_encoding: :geoarrow),
    "arrows",
    native: true
  )

File.write!(Path.join(dir, "expected.json"), Jason.encode!(Enum.reverse(files), pretty: true))
IO.puts("wrote #{length(files)} files and expected.json to #{dir}")
