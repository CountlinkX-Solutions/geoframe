defmodule GeoFrame do
  @moduledoc """
  GeoPandas-style geospatial dataframes for Elixir, powered by GeoArrow and Rust.

  This module holds file I/O. See `GeoFrame.GeoDataFrame` for the dataframe and
  `GeoFrame.GeoSeries` for geometry columns.

  ## Formats

  | Format      | Read                | Write                | Extensions                      |
  |-------------|---------------------|----------------------|---------------------------------|
  | GeoParquet  | `read_parquet/2`    | `write_parquet/3`    | `.parquet`, `.geoparquet`       |
  | GeoJSON     | `read_geojson/2`    | `write_geojson/3`    | `.geojson`, `.json`             |
  | FlatGeobuf  | `read_flatgeobuf/2` | `write_flatgeobuf/3` | `.fgb`                          |
  | Feather v2  | `read_feather/2`    | `write_feather/3`    | `.feather`, `.arrow`            |
  | Shapefile   | `read_shapefile/2`  | `write_shapefile/3`  | `.shp`, `.zip`                  |
  | GeoPackage  | `read_gpkg/2`       | `write_gpkg/3`       | `.gpkg`                         |

  PostGIS (`read_postgis/3`, `to_postgis/4`, optional `:postgrex`) is a database rather than a
  file; see `GeoFrame.PostGIS`.

  `read_file/2` and `write_file/3` pick the format from the extension or from the
  `:driver` option. All formats share one data path: geometry stays in Rust, and the
  other columns cross the NIF boundary as an Arrow IPC stream. No format needs GDAL.
  `list_layers/2` and `read_file_info/2` describe a file without reading all of it (GeoPackage
  and Shapefile) and work for every format.

  ## Shapefile and GeoPackage

  Shapefile is read with the pure-Rust `shapefile` crate and written by GeoFrame itself; the
  GeoPackage format is SQLite, through `rusqlite` with the **bundled** SQLite amalgamation.
  That SQLite is the only C code GeoFrame compiles (with `cc`, no system library needed). See
  `read_shapefile/2`, `write_shapefile/3`, `read_gpkg/2` and `write_gpkg/3` for the details
  and the deviations from GDAL.

  `GeoFrame.GeoSeries.to_arrow/2`, `GeoFrame.GeoDataFrame.to_arrow/2` and the matching
  `from_arrow` functions exchange an Arrow IPC **stream** as a binary, with GeoArrow
  extension metadata (`ARROW:extension:name`, `ARROW:extension:metadata` with the CRS as
  PROJJSON) on the geometry field. Elixir has no Arrow C Data Interface binding, so IPC bytes
  are the interchange (one serialisation and one copy per direction); Explorer and ADBC
  can load the same bytes, and `pyarrow` plus `GeoDataFrame.from_arrow` read them.

  ## GeoParquet

  A file with several geometry columns reads as a frame with one active geometry (the
  `primary_column`) and secondary `GeoFrame.GeoSeries` columns (`GeoDataFrame.geometry_columns/1`,
  `GeoDataFrame.set_geometry/3`); `write_parquet/3` writes all of them with their own CRS.
  Feather and Arrow IPC (`write_feather/3`, `read_feather/2`, `GeoDataFrame.to_arrow/2`) keep
  every geometry column too. The formats with one geometry per feature (GeoPackage, GeoJSON,
  FlatGeobuf, Shapefile, PostGIS) raise `ArgumentError` for a frame with secondary geometry
  columns, like GeoPandas `to_file`: drop them, convert them to WKT or use Parquet/Feather.

  `read_parquet/2` and `write_parquet/3` split a file in two halves. The geometry column
  lives in Rust memory (a `GeoFrame.GeoSeries`); every other column crosses the NIF
  boundary as an Arrow IPC stream and is loaded into (or dumped from) an
  `Explorer.DataFrame`. Explorer has no zero-copy NIF access to Polars memory, so each
  direction costs one serialisation and one copy of the attribute data. That is acceptable
  by design: geometry, the bulky part, is never copied into Elixir.

  `read_file/2` reads a file into memory whole (on a dirty IO scheduler); `stream_file/2`
  reads GeoParquet and FlatGeobuf in batches. Read supports WKB and the GeoArrow-native GeoParquet encodings, honours
  the covering `bbox` column (which is never surfaced as an attribute) and filters rows with
  `bbox:`. Write produces GeoParquet 1.1.0 with WKB geometry and Snappy compression by
  default; see `write_parquet/3` for the compression, row group, native encoding, covering
  bbox and schema version options.

  ### Coordinate reference system

  GeoParquet 1.1 semantics apply when reading: a missing `crs` key means `"OGC:CRS84"`,
  an explicit `null` means unknown (`nil`). PROJJSON that carries an identifier is turned
  into an `"AUTHORITY:CODE"` string, other PROJJSON is kept as JSON text. When writing,
  `"OGC:CRS84"` omits the key, `nil` writes an explicit `null`, an EPSG code (`"EPSG:4326"`,
  `"EPSG:3035"`, a UTM zone, ...) gets a full PROJJSON definition that `geopandas.read_parquet`
  parses (every EPSG code in the bundled definition tables: the `crs-definitions` table and the
  supplementary one generated from the PROJ database; geographic, geocentric, projected and
  compound CRS), any other `"AUTHORITY:CODE"` string (an EPSG code outside both tables,
  another authority) is written as a PROJJSON stub with only a name and an identifier (the
  pure-Rust build has no CRS database), and a PROJJSON string is written as given, which is how
  to write a CRS the tables lack. Any other CRS string is rejected.

  ### Column order

  The geometry column is written last and read back as the geometry of the frame, so its
  original position among the columns is not preserved.
  """

  alias GeoFrame.Formats
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.VectorFiles

  @doc """
  Concatenates a list of `GeoFrame.GeoSeries` or a list of `GeoFrame.GeoDataFrame` row-wise.

  See `GeoFrame.GeoSeries.concat/1` and `GeoFrame.GeoDataFrame.concat_rows/1`. A list mixing
  series and frames raises `ArgumentError`.
  """
  @spec concat([GeoSeries.t()] | [GeoDataFrame.t()]) :: GeoSeries.t() | GeoDataFrame.t()
  def concat([%GeoSeries{} | _] = series), do: GeoSeries.concat(series)
  def concat([%GeoDataFrame{} | _] = frames), do: GeoDataFrame.concat_rows(frames)

  def concat(other),
    do:
      raise(
        ArgumentError,
        "concat expects a non-empty list of GeoSeries or of GeoDataFrames, got: #{inspect(other)}"
      )

  @doc """
  Alias of `GeoFrame.GeoDataFrame.join/3` (GeoPandas `GeoDataFrame.merge`).
  """
  @spec merge(GeoDataFrame.t(), Explorer.DataFrame.t() | GeoDataFrame.t(), keyword()) ::
          GeoDataFrame.t()
  defdelegate merge(left, right, opts \\ []), to: GeoDataFrame, as: :join

  @doc """
  Alias of `GeoFrame.GeoSeries.from_xy/3` (GeoPandas `points_from_xy`).
  """
  @spec points_from_xy(
          [number() | nil] | Explorer.Series.t(),
          [number() | nil] | Explorer.Series.t(),
          keyword()
        ) :: GeoSeries.t()
  defdelegate points_from_xy(x, y, opts \\ []), to: GeoSeries, as: :from_xy

  @doc """
  Spatial join of two `GeoFrame.GeoDataFrame`s. See `GeoFrame.SpatialJoin`.
  """
  @spec sjoin(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  defdelegate sjoin(left, right, opts \\ []), to: GeoFrame.SpatialJoin

  @doc """
  Nearest-neighbour spatial join of two `GeoFrame.GeoDataFrame`s. See
  `GeoFrame.SpatialJoin.sjoin_nearest/3`.
  """
  @spec sjoin_nearest(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  defdelegate sjoin_nearest(left, right, opts \\ []), to: GeoFrame.SpatialJoin

  @doc """
  Clips a frame or series to a polygon (or bounding box) mask. See `GeoFrame.Overlay.clip/3`.
  """
  @spec clip(GeoDataFrame.t() | GeoSeries.t(), term(), keyword()) ::
          GeoDataFrame.t() | GeoSeries.t()
  defdelegate clip(data, mask, opts \\ []), to: GeoFrame.Overlay

  @doc """
  Spatial overlay of two `GeoFrame.GeoDataFrame`s (`intersection`, `union`, `identity`,
  `symmetric_difference`, `difference`). See `GeoFrame.Overlay.overlay/3`.
  """
  @spec overlay(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  defdelegate overlay(df1, df2, opts \\ []), to: GeoFrame.Overlay

  @doc """
  Runs a SQL query on a Postgrex connection and returns the result as a
  `GeoFrame.GeoDataFrame` (GeoPandas `read_postgis`).

  `conn` is a `Postgrex` connection (pid or registered name). The optional `:postgrex`
  dependency must be present; start the connection with `types: GeoFrame.Postgrex.Types` so
  `geometry` columns decode to raw EWKB:

      {:ok, conn} = Postgrex.start_link(hostname: "localhost", database: "gis", types: GeoFrame.Postgrex.Types)
      {:ok, gdf} = GeoFrame.read_postgis(conn, "SELECT id, name, geom FROM places WHERE id > $1", params: [10])

  Without that types module, select the geometry as `ST_AsEWKB(geom) AS geom` (bytea) or as
  hex text; both are detected automatically.

  Options:

    * `:geom_col` - the geometry column of the result (default `"geom"`, as in GeoPandas).
    * `:crs` - overrides the CRS. Otherwise the EWKB SRID becomes `"EPSG:<srid>"` (SRID 0 or
      none gives `nil`); rows with different SRIDs are an error without an override.
    * `:params` - query parameters (default `[]`).

  Postgres types map to Explorer dtypes: integers to `{:s, 64}`, floats and `numeric` to
  `{:f, 64}`, `text` to `:string`, `boolean`, `date`, `timestamp` to `{:naive_datetime,
  :microsecond}`, `timestamptz` to `{:datetime, :microsecond, "Etc/UTC"}`, `json`/`jsonb` to
  JSON text, `bytea` to `:binary`. Nulls stay null. Returns `{:error, message}` on a query or
  decoding failure.
  """
  @spec read_postgis(term(), String.t(), keyword()) ::
          {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_postgis(conn, sql, opts \\ []), do: GeoFrame.PostGIS.read(conn, sql, opts)

  @doc "Like `read_postgis/3` but raises `ArgumentError` on failure."
  @spec read_postgis!(term(), String.t(), keyword()) :: GeoDataFrame.t()
  def read_postgis!(conn, sql, opts \\ []) do
    case read_postgis(conn, sql, opts) do
      {:ok, gdf} -> gdf
      {:error, message} -> raise ArgumentError, message
    end
  end

  @doc """
  Writes a `GeoFrame.GeoDataFrame` to a PostGIS table (GeoPandas `to_postgis`).

  The table gets one typed column per attribute plus the geometry column
  (`geometry(<Type>, <srid>)`, or `geometry(Geometry, <srid>)` for mixed types), named after
  the frame's `geometry_column`. The SRID comes from an `"EPSG:<code>"` CRS (`OGC:CRS84` is
  4326, no CRS is 0); other CRS strings raise. Geometry is sent as EWKB parameters, in batches,
  inside one transaction: a failure rolls everything back. Identifiers are always quoted and
  values are always bind parameters. The PostGIS extension must already be installed.

  Options:

    * `:if_exists` - `:fail` (default), `:replace` (drop and recreate) or `:append`. Appending
      to a missing table creates it.
    * `:schema` - the schema (default: the connection's search path).
    * `:index` - must be `false` (there is no index to write).
    * `:dtype` - a map of column name to SQL type (for example `%{"id" => "integer"}`), only
      plain type names are accepted.
    * `:spatial_index` - `true` creates a GiST index on the geometry column (off by default,
      like GeoPandas).
    * `:chunksize` - rows per `INSERT` (default 1000, capped by the 65 535 parameter limit).

  Returns `:ok` or `{:error, message}`; invalid options raise `ArgumentError`.
  """
  @spec to_postgis(GeoDataFrame.t(), term(), String.t(), keyword()) :: :ok | {:error, String.t()}
  def to_postgis(%GeoDataFrame{} = gdf, conn, table, opts \\ []),
    do: GeoFrame.PostGIS.write(gdf, conn, table, opts)

  @doc "Like `to_postgis/4` but raises `ArgumentError` on failure."
  @spec to_postgis!(GeoDataFrame.t(), term(), String.t(), keyword()) :: :ok
  def to_postgis!(%GeoDataFrame{} = gdf, conn, table, opts \\ []) do
    case to_postgis(gdf, conn, table, opts) do
      :ok -> :ok
      {:error, message} -> raise ArgumentError, message
    end
  end

  @doc """
  Reads a GeoParquet file into a `GeoFrame.GeoDataFrame`.

  The primary geometry column named in the file's `geo` metadata becomes the geometry;
  all other columns become attributes.

  Options:

    * `:columns` - attribute column names to keep (the geometry is always read); they keep
      their file order.
    * `:bbox` - `{minx, miny, maxx, maxy}` in the file's CRS: keep only the rows whose
      geometry bounding box intersects it (touching counts). With a covering `bbox` column
      or a native geometry encoding, row groups outside the box are skipped using the file
      statistics and rows are filtered on the covering values; otherwise the geometries are
      decoded and filtered after the read.

  Returns `{:error, message}` when the file cannot be opened, is not Parquet, has no `geo`
  metadata, or cannot be decoded.
  """
  @spec read_parquet(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_parquet(path, opts \\ []), do: Formats.read_parquet(path, opts)

  @doc "Like `read_parquet/2` but raises `ArgumentError` on failure."
  @spec read_parquet!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_parquet!(path, opts \\ []), do: unwrap!(read_parquet(path, opts))

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a GeoParquet file (WKB geometry, Snappy, version
  1.1.0 by default).

  Options (GeoPandas `to_parquet` names):

    * `:compression` - `:snappy` (default), `:zstd`, `:gzip`, `:lz4`, `:brotli` or `:none`.
    * `:compression_level` - an integer for `:zstd` (1..22), `:gzip` (0..10) or `:brotli`
      (0..11); an error for the other codecs. Default: the codec's default level.
    * `:row_group_size` - maximum rows per row group (positive integer).
    * `:geometry_encoding` - `:wkb` (default) or `:geoarrow`. The native encoding needs a
      single geometry kind like GeoPandas (a type and its multi variant may mix and are
      written as the multi type) and is `{:error, _}` for mixed geometries or collections.
    * `:write_covering_bbox` - `true` adds a `bbox` struct column (`xmin`, `ymin`, `xmax`,
      `ymax`) and the `covering` metadata, which makes `bbox:` reads prune row groups (for the
      active geometry only).
      Default `false`.
    * `:schema_version` - `"1.1.0"` (default) or `"1.0.0"`. Version 1.0.0 allows only WKB
      geometry and no covering column.

  Returns `:ok` or `{:error, message}`; a partially written file is removed. Invalid option
  values raise `ArgumentError`.
  """
  @spec write_parquet(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_parquet(%GeoDataFrame{} = gdf, path, opts \\ []),
    do: Formats.write_parquet(gdf, path, opts)

  @doc """
  Reads a Feather v2 file (Arrow IPC file) into a `GeoFrame.GeoDataFrame`, including the
  files GeoPandas `to_feather` writes (WKB plus `geo` metadata) and native GeoArrow columns.

  The geometry column is the primary column of the `geo` metadata, else the first
  `geoarrow.*` extension column, else a binary (WKB) or string (WKT) column named
  `"geometry"`; every other geometry column (declared in `geo` or a `geoarrow.*` extension
  column) is a secondary geometry column of the frame. The CRS comes from the `geo` metadata,
  else from the extension metadata.
  LZ4 and Zstandard compressed files read.

  Options: `:columns` - attribute column names to keep; `:geometry` - the name of the
  geometry column when it cannot be detected.
  """
  @spec read_feather(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_feather(path, opts \\ []), do: Formats.read_feather(path, opts)

  @doc "Like `read_feather/2` but raises `ArgumentError` on failure."
  @spec read_feather!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_feather!(path, opts \\ []), do: unwrap!(read_feather(path, opts))

  @doc """
  Writes a Feather v2 file (Arrow IPC file) like GeoPandas `to_feather`: the geometry is a
  `geoarrow.wkb` column (or a native GeoArrow column) whose extension metadata carries the
  CRS as PROJJSON, and the schema metadata has the GeoParquet-style `geo` entry.

  Options: `:compression` - `:lz4` (default, as pyarrow), `:zstd` or `:uncompressed`;
  `:geometry_encoding` - `:wkb` (default) or `:geoarrow` (see `write_parquet/3`);
  `:record_batch_size` - rows per Arrow record batch (positive integer, default 65 536, the
  `chunksize` of pyarrow's `write_feather`, which GeoPandas `to_feather` goes through): small
  batches let `stream_file/2` read the file in bounded memory, which a single record batch
  cannot. Secondary geometry columns are written after the active one with their own CRS and
  `geo` entry.

  Returns `:ok` or `{:error, message}`; a partially written file is removed.
  """
  @spec write_feather(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_feather(%GeoDataFrame{} = gdf, path, opts \\ []),
    do: Formats.write_feather(gdf, path, opts)

  @doc """
  Reads a GeoJSON file into a `GeoFrame.GeoDataFrame`.

  A `FeatureCollection` gives one row per feature (a lone `Feature` or geometry object
  gives one row). The CRS is `"OGC:CRS84"`, as RFC 7946 defines; a legacy `crs` member is
  ignored.

  Properties become attribute columns in the order they are first seen, typed from the
  whole file: booleans stay boolean, integers `s64`, integers mixed with other numbers
  `f64`, and anything else (strings, mixed kinds, nested objects and arrays) a string
  column. Nested objects and arrays are kept as compact JSON text and scalars in a mixed
  column as their JSON text (`true`, `7`). A property missing from a feature, or `null`, is
  `nil`; a property that is `null` everywhere becomes an all-`nil` string column. A feature
  `id` becomes a leading `"id"` column when every feature has one and no property is called
  `"id"`.

  Options: `:geometry_column` - name for the geometry (default `"geometry"`, or
  `"geometry_1"` and so on when a property already uses that name).

  Returns `{:error, message}` when the file cannot be read or is not GeoJSON.
  """
  @spec read_geojson(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_geojson(path, opts \\ []), do: Formats.read_geojson(path, opts)

  @doc "Like `read_geojson/2` but raises `ArgumentError` on failure."
  @spec read_geojson!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_geojson!(path, opts \\ []), do: unwrap!(read_geojson(path, opts))

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a GeoJSON `FeatureCollection`.

  GeoJSON (RFC 7946) can only hold WGS 84 longitude/latitude, so the CRS must be
  `"OGC:CRS84"` or `"EPSG:4326"`; any other CRS (including an unknown one) returns an error
  unless `force: true`, which writes the coordinates as they are. This differs from
  GeoPandas, which always writes whatever coordinates it has. Reproject with
  `GeoFrame.GeoDataFrame.to_crs/2` first when in doubt.

  Every attribute becomes a feature property, in column order (no feature `id` is
  written). `NaN` and infinite floats become `null`; dates, timestamps and categories are
  written as text; binary and nested columns are an error. Rows whose geometry is null (or
  cannot be expressed in GeoJSON, such as an empty point) get `"geometry": null`.
  Z is written as the third ordinate (RFC 7946 has no M, so M is dropped). A frame with no
  rows has no properties, so its columns are lost.

  Options: `:force` - skip the CRS check (default `false`).

  Returns `:ok` or `{:error, message}`; a partially written file is removed.
  """
  @spec write_geojson(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_geojson(%GeoDataFrame{} = gdf, path, opts \\ []),
    do: Formats.write_geojson(gdf, path, opts)

  @doc """
  Reads a FlatGeobuf file into a `GeoFrame.GeoDataFrame`.

  Column types map to Explorer dtypes one to one (`Byte` to `s8`, `UInt` to `u32`, `Float`
  to `f32`, and so on); `String`, `Json` and `DateTime` columns are string columns and
  `Binary` columns binary. The CRS comes from the header as `"ORG:code"` (organisation
  defaults to EPSG), `nil` when absent. Curve geometries are not supported.

  Options:

    * `:bbox` - `{minx, miny, maxx, maxy}`: only features whose geometry intersects it
      are read (as GDAL's spatial filter, so GeoPandas `read_file(bbox=...)`: the index
      narrows the search by bounding box, then each geometry is checked; an error if the file
      has no index). Features without a geometry never match.
    * `:geometry_column` - as in `read_geojson/2`.

  Returns `{:error, message}` when the file cannot be read or is not FlatGeobuf.
  """
  @spec read_flatgeobuf(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_flatgeobuf(path, opts \\ []), do: Formats.read_flatgeobuf(path, opts)

  @doc "Like `read_flatgeobuf/2` but raises `ArgumentError` on failure."
  @spec read_flatgeobuf!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_flatgeobuf!(path, opts \\ []), do: unwrap!(read_flatgeobuf(path, opts))

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a FlatGeobuf file.

  Integer, float, boolean, string and binary columns keep their width; dates, timestamps
  and categories are written as text, nested columns are an error. The header geometry type
  is the type shared by all rows, or unknown for a mix (geometries are never promoted to
  multi types). Z and M are kept (the header flags say which) and null geometries are stored as empty ones.

  The CRS is stored in the header: `"EPSG:n"` and other `"AUTHORITY:CODE"` strings as
  organisation and code, WKT as WKT, `nil` as no CRS. FlatGeobuf has no CRS84, so
  `"OGC:CRS84"` is stored as `"EPSG:4326"` (and reads back as that). Other CRS strings, such
  as PROJJSON, are an error.

  Options:

    * `:index` - write the packed Hilbert R-tree spatial index (default `true`). **With the
      index, rows are stored in Hilbert order, so row order is not preserved** (as in GDAL);
      pass `index: false` to keep it. `read_flatgeobuf/2` with `:bbox` needs the index.
    * `:name` - the dataset name in the header (default: the file name without extension).

  Returns `:ok` or `{:error, message}`.
  """
  @spec write_flatgeobuf(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_flatgeobuf(%GeoDataFrame{} = gdf, path, opts \\ []),
    do: Formats.write_flatgeobuf(gdf, path, opts)

  @doc """
  Reads a Shapefile (a `.shp` path or a `.zip` holding exactly one) into a
  `GeoFrame.GeoDataFrame`.

  The `.shx`, `.dbf`, `.prj` and `.cpg` next to the `.shp` (any letter case) are used when
  present; without a `.dbf` only the geometry is read. Geometry: a polyline with one part is
  a `LineString` and with several a `MultiLineString`; a polygon shape with one outer ring a
  `Polygon` and with several a `MultiPolygon`; null shapes are null geometries; Z and M values
  are kept (`PointZ`, `PolyLineM`, ... read as XYZ, XYM or XYZM).

  Attributes follow the dBase types: `C` strings (trailing blanks trimmed; an empty value is
  `nil`, as in GDAL), `N` with no decimals as `s32` (width up to 9) or `s64` (up to 18),
  other `N`/`F` as `f64`, `L` booleans, `D` dates. Deleted records are skipped. Memo and
  other binary field types are an error.

  The CRS is taken from the `.prj`: an `AUTHORITY["EPSG", ...]` or one of the well-known
  ESRI names (`GCS_WGS_1984`, `WGS_1984_Web_Mercator_Auxiliary_Sphere`, WGS 84 and NAD83 UTM
  zones, ...) becomes `"EPSG:<code>"`, any other definition is kept as its WKT text, and
  no `.prj` means `nil`.

  Options:

    * `:encoding` - the text encoding of the `.dbf` (`"UTF-8"`, `"ASCII"`, `"ISO-8859-1"`,
      `"CP1252"`). Defaults to the `.cpg` file, then to UTF-8 (the language-driver byte of
      the `.dbf` is ignored, as in pyogrio).

  Returns `{:error, message}` when the file cannot be read.
  """
  @spec read_shapefile(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_shapefile(path, opts \\ []), do: VectorFiles.read_shapefile(path, opts)

  @doc "Like `read_shapefile/2` but raises `ArgumentError` on failure."
  @spec read_shapefile!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_shapefile!(path, opts \\ []), do: unwrap!(read_shapefile(path, opts))

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a Shapefile (`.shp`, `.shx`, `.dbf`, `.cpg` and, when
  the CRS has a definition, `.prj`); a path ending in `.zip` writes one zip archive holding
  those files.

  A shapefile holds one geometry type. `Polygon` and `MultiPolygon` rows are both polygon
  shapes, `LineString` and `MultiLineString` both polyline shapes; any other mix, or a
  `GeometryCollection`, raises `ArgumentError` (GDAL refuses it too). Null and empty
  geometries become null shapes; Z and M are written with the Z or M shape types; polygon rings are written in the
  format's orientation (exterior clockwise).

  Column types: integers `N(9,0)` (8 to 32 bits) or `N(18,0)` (64 bits and unsigned),
  floats `N(24,15)` (fewer decimals for large values, `NaN` and infinity become null),
  booleans `L`, dates `D`, strings `C` sized to the longest value (at most 254 bytes, longer
  values are truncated with a warning). Timestamps and other types are written as text;
  binary columns are an error. Null strings and empty strings are not distinguished.

  **Field names hold 10 bytes.** Longer names are truncated at a character boundary and a
  clash gets `_1`, `_2`, ... (stem shortened to fit): `population_total` and
  `population_urban` become `population` and `populati_1`, exactly as GDAL does. Each changed
  name is logged with `Logger.warning/1`. A frame without attributes gets a `FID` column, as
  in GDAL.

  The `.prj` is the OGC WKT definition of the CRS for any EPSG code in the `crs-definitions`
  table (the codes only in the supplementary table, see `GeoFrame.GeoSeries.to_crs/2`, have no
  WKT1 here; GDAL writes the ESRI flavour; GDAL, GeoPandas and pyproj read both); a WKT CRS string is
  written as given. A CRS GeoFrame has no definition for (an unknown code, `ESRI:` codes) writes
  no `.prj` and logs a warning. A stale `.prj` from an earlier write is removed when there is
  no CRS.

  Options:

    * `:encoding` - `"UTF-8"` (default), `"ASCII"`, `"ISO-8859-1"` or `"CP1252"`; recorded in
      the `.cpg`. Characters the encoding lacks are written as `?` with a warning.

  Returns `:ok` or `{:error, message}`.
  """
  @spec write_shapefile(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_shapefile(%GeoDataFrame{} = gdf, path, opts \\ []),
    do: VectorFiles.write_shapefile(gdf, path, opts)

  @doc """
  Reads one layer of a GeoPackage into a `GeoFrame.GeoDataFrame`.

  Without `:layer` a file with a single feature layer reads that layer; with several layers
  the call fails and names them (GeoPandas warns and takes the first one instead). Geometry is
  decoded from the GeoPackageBinary blobs (header, optional envelope, WKB); empty-flagged
  geometries are `nil`. The integer primary key (`fid`) is not an attribute. Column types
  follow the declared SQL type (`BOOLEAN`, `TINYINT` to `INTEGER`, `FLOAT`, `REAL`/`DOUBLE`,
  `TEXT`, `BLOB`; `DATE` as dates and `DATETIME` as naive microsecond timestamps, unparseable
  text as `nil`). The CRS comes from `gpkg_spatial_ref_sys`: `"EPSG:<code>"`, another
  `"ORG:code"`, a custom definition as WKT, `nil` for the undefined SRS ids.

  Options: `:layer` - the layer (table) name.
  """
  @spec read_gpkg(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_gpkg(path, opts \\ []), do: VectorFiles.read_gpkg(path, opts)

  @doc "Like `read_gpkg/2` but raises `ArgumentError` on failure."
  @spec read_gpkg!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_gpkg!(path, opts \\ []), do: unwrap!(read_gpkg(path, opts))

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a layer of an OGC GeoPackage 1.3 file.

  Creates the file with its metadata tables, or adds the layer to an existing GeoPackage
  (a file that is not a GeoPackage is never overwritten). The feature table has an `fid`
  integer primary key, the geometry column `geom` and one typed column per attribute
  (`BOOLEAN`, `TINYINT`, `SMALLINT`, `MEDIUMINT`, `INTEGER`, `FLOAT`, `REAL`, `TEXT`, `BLOB`,
  `DATE`, `DATETIME`); a frame column named `fid` is an error. Geometry is stored as
  GeoPackageBinary with an envelope. The layer geometry type is the common type of the rows,
  or `GEOMETRY` for a mix; unlike GDAL, single and multi geometries are **not** promoted to
  multi, so rows read back as written. Z and M are kept (the `z` and `m` flags of the layer are 0, 1 or 2).

  The CRS must be an `"AUTHORITY:CODE"` string, `"OGC:CRS84"` (stored as EPSG 4326), PROJJSON
  with an identifier, or WKT. An EPSG code outside the `crs-definitions` table is stored with the
  definition `undefined` and a warning; `nil` is stored as GDAL's "Undefined SRS" (id 99999), which GDAL and GeoPandas read back as no CRS.

  Options:

    * `:layer` - the layer name (default: the file name without extension).
    * `:mode` - `:w` (default) creates the file or replaces the layer of that name, keeping
      the file's other layers; `:a` appends rows to an existing layer after checking that its
      columns contain the frame's and that the CRS matches (a missing layer is created).
    * `:spatial_index` - create the `rtree_<layer>_geom` R-tree index and its triggers for a new
      layer (default `true`).

  Returns `:ok` or `{:error, message}`.
  """
  @spec write_gpkg(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_gpkg(%GeoDataFrame{} = gdf, path, opts \\ []),
    do: VectorFiles.write_gpkg(gdf, path, opts)

  @doc """
  The layers of a file as an `Explorer.DataFrame` with the string columns `name` and
  `geometry_type` (GeoPandas `list_layers`).

  A GeoPackage lists its feature and attribute tables in file order (`geometry_type` is `nil`
  for an attribute table); geometry types are pyogrio's names (`"Point"`, `"MultiPolygon"`,
  `"Unknown"` for a mix, with ` Z`/` M` suffixes). Every other format has one layer named
  after the file: Shapefile reports the type in its header, the rest are read to find the
  common geometry type (`"Unknown"` for a mix).

  Options: `:driver` - as in `read_file/2`. Raises `ArgumentError` if the file cannot be read.
  """
  @spec list_layers(Path.t(), keyword()) :: Explorer.DataFrame.t()
  def list_layers(path, opts \\ []) do
    {driver, rest} = Formats.driver_for(path, opts)
    Formats.validate_options!(rest, [])
    VectorFiles.list_layers(path, driver)
  end

  @doc """
  Facts about a vector file, shaped after pyogrio's `read_info`, as a map:
  `:driver`, `:layer`, `:crs`, `:encoding`, `:geometry_type`, `:features` (row count),
  `:total_bounds` (`{minx, miny, maxx, maxy}` or `nil`), `:fields` (attribute names) and
  `:dtypes` (pandas-style dtype names such as `"int64"`, `"object"`).

  GeoPackage and Shapefile answer from their metadata (a GeoPackage without `:layer` describes
  its first feature layer); other formats are read in full. Raises `ArgumentError` if the file
  cannot be read.

  Options: `:driver` as in `read_file/2`; `:layer` (GeoPackage); `:encoding` (Shapefile).
  """
  @spec read_file_info(Path.t(), keyword()) :: map()
  def read_file_info(path, opts \\ []) do
    {driver, rest} = Formats.driver_for(path, opts)

    allowed =
      case driver do
        :gpkg -> [:layer]
        :shapefile -> [:encoding]
        _other -> []
      end

    Formats.validate_options!(rest, allowed)
    VectorFiles.read_file_info(path, driver, rest)
  end

  @doc """
  Reads a vector file, choosing the format from the extension (`.parquet`, `.geoparquet`,
  `.geojson`, `.json`, `.fgb`, `.feather`, `.arrow`, `.shp`, `.zip`, `.gpkg`;
  case-insensitive) or from `driver:` (`:parquet`, `:geojson`, `:flatgeobuf`, `:feather`,
  `:shapefile`, `:gpkg`), which wins over the extension. A `.zip` is read as a zipped Shapefile.

  The remaining options go to the format's reader (`read_parquet/2`, `read_geojson/2`,
  `read_flatgeobuf/2`, `read_feather/2`, `read_shapefile/2`, `read_gpkg/2` - so `layer:`
  selects a GeoPackage layer); unknown options raise `ArgumentError`.

  `bbox: {minx, miny, maxx, maxy}` (in the file's CRS) filters rows as GeoPandas'
  `read_file(bbox=...)` does, for GeoParquet, FlatGeobuf, Shapefile, GeoPackage and GeoJSON
  (Feather has none: `read_feather` has no `bbox`, and pyogrio has no Arrow driver). GeoParquet
  keeps the rows whose geometry bounding box intersects the box (the covering column, as
  GeoPandas `read_parquet`); the GDAL formats keep the rows whose *geometry* intersects the
  box, so a diagonal line whose bounding box meets the box without touching it is dropped,
  and rows without a geometry never match. Shapefile, GeoPackage and GeoJSON are read through
  the `stream_file/2` readers (the same filter) and concatenated, so the result has the
  dtypes and CRS of an unfiltered read, and an empty match is an empty frame with them.
  `rows:` counts the rows that pass the filter. `mask:` is not supported.
  An unrecognised extension or driver raises `ArgumentError`; read failures are
  `{:error, message}`.

  `rows:` reads only some rows, as GeoPandas' `read_file(rows=...)` does: an integer `n` is
  the first `n` rows, a range `first..last` is the rows at positions `first` to `last`
  (inclusive: `5..14` is ten rows, GeoPandas' `slice(5, 15)`). The result equals
  `read_file(path)` followed by `GeoDataFrame.slice/3` (a window past the end is cut short, an
  empty window gives an empty frame with the file's columns and CRS), but the file is read
  through the same incremental readers as `stream_file/2`, so the first rows of a huge file
  are cheap and memory is bounded by the window: GeoParquet skips whole row groups (and
  pages) before the window and stops after it, FlatGeobuf skips features without decoding
  them, Feather skips whole Arrow record batches, and the other formats read past the rows
  in bounded chunks. With `bbox:` (where the format accepts it) the window counts the rows
  that pass the filter. A GeoJSON file is still scanned once whole to infer the column
  types, so `rows:` saves its decoding and memory, not that pass. Unlike pyogrio, `rows: 0`
  is an empty frame (pyogrio treats a zero limit as no limit); negative values, a range with
  a step other than 1 or a decreasing range raise `ArgumentError` (GeoPandas raises
  `ValueError` for a negative start or a step). `rows:` accepts the options `stream_file/2`
  accepts for the format.
  """
  @spec read_file(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_file(path, opts \\ []), do: Formats.read_file(path, opts)

  @doc "Like `read_file/2` but raises `ArgumentError` on failure."
  @spec read_file!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_file!(path, opts \\ []), do: unwrap!(read_file(path, opts))

  @doc """
  Reads a vector file lazily, as a `Stream` of `GeoFrame.GeoDataFrame` batches, so a file
  larger than memory can be processed (GeoPandas has no equivalent; pyogrio's `read_arrow` /
  `open_arrow` batches are the nearest).

  The format comes from the extension or `driver:`, as in `read_file/2`. Streaming is
  available for GeoParquet (`.parquet`, `.geoparquet`), FlatGeobuf (`.fgb`), Arrow IPC /
  Feather (`.feather`, `.arrow`), Shapefile (`.shp`, `.zip`), GeoPackage (`.gpkg`) and
  GeoJSON (`.geojson`, `.json`).

  Every batch holds `:batch_size` rows (the last one fewer), whatever the row group or record
  batch size of the file, and has the CRS, the attribute dtypes and the geometry column name
  `read_file/2` gives, so concatenating the batches equals `read_file/2` with the same
  options. A file without rows (or a `:bbox` that matches none) yields no batches.

  Options (an option a format does not support raises `ArgumentError`):

    * `:batch_size` - rows per batch, a positive integer (default `65_536`).
    * `:columns` - GeoParquet, Feather and GeoPackage: attribute column names to read (the
      geometry is always read), as in `read_parquet/2` and `read_feather/2`. GeoParquet never
      decodes the other columns; Feather decodes only these from each record batch; a
      GeoPackage selects only these columns.
    * `:bbox` - `{minx, miny, maxx, maxy}` in the file's CRS (GeoParquet, FlatGeobuf,
      Shapefile, GeoPackage, GeoJSON), with the semantics of `read_file/2`'s `bbox:`: GeoParquet
      keeps the rows whose geometry bounding box intersects it, as `read_parquet/2` does (row
      groups are skipped from a covering `bbox` column or native encoding); the other formats
      keep the rows whose geometry itself intersects the box, as GDAL's spatial filter does.
      FlatGeobuf uses the packed R-tree index (an error without one) and then checks the
      geometry. A GeoPackage layer uses its R-tree when it has one and checks every row either
      way. A Shapefile has no index, so each record is filtered after it is decoded, and so is
      each GeoJSON feature. Rows without a geometry (or an empty one) never match.
    * `:layer` - GeoPackage: the layer, as in `read_gpkg/2`.
    * `:encoding` - Shapefile: the `.dbf` encoding, as in `read_shapefile/2`.
    * `:geometry` - Feather: the geometry column, as in `read_feather/2`.
    * `:driver` and `:geometry_column` - as in `read_file/2` (`:geometry_column` is a
      FlatGeobuf and GeoJSON option).

  Memory is bounded by the batch for GeoParquet (one row group at a time), FlatGeobuf, Shapefile
  (the `.shp` record by record and the `.dbf` in matching chunks) and GeoPackage (pages of
  `batch_size` rows through a primary key cursor, O(n) in total). A Feather file is read one
  Arrow record batch at a time, so it only stays small when the file was written in small
  record batches; `write_feather/3` writes batches of `record_batch_size:` rows (65 536 by
  default), files written before 0.2.0 hold a single batch. A `.zip`
  shapefile is extracted to a scratch directory in the system temp dir (the `.shp` and `.dbf`
  members, each capped as in `read_file/2`), which is removed when the stream ends, halts or
  fails.

  A GeoJSON `FeatureCollection` is parsed feature by feature, so memory is one batch of
  features. `read_file/2` infers each property's type over every feature, which a stream
  cannot see ahead, so the file is read **twice**: a first pass (O(n) time, O(1) memory)
  collects the property names, their types and whether every feature has an `id`, and the
  second pass produces the batches. Every batch then has the columns and dtypes `read_file/2`
  gives, even when a late feature decides a type or is the first to have a property (earlier
  batches hold nulls); a malformed file raises when the stream is enumerated, before the
  first batch. `:bbox` does not narrow the first pass. A lone `Feature` or bare geometry
  file is one row and is read whole. Newline-delimited GeoJSON (GeoJSONSeq) is not
  supported by `read_file/2` either.

  The file is opened when the stream is first enumerated (each enumeration reads it again)
  and closed when the stream finishes, halts early (`Enum.take/2`) or its consumer raises;
  a stream that is dropped without being run holds nothing. Batches are read on a dirty IO
  scheduler. A file that cannot be opened or decoded raises `ArgumentError` while the stream
  is enumerated; invalid options raise as soon as the stream is built.

  ## Examples

      iex> path = Path.join(System.tmp_dir!(), "geoframe_stream_file_doc.parquet")
      iex> geometry = GeoFrame.GeoSeries.from_wkt!(
      ...>   Enum.map(0..4, &"POINT (\#{&1} 0)"),
      ...>   crs: "EPSG:4326"
      ...> )
      iex> gdf = GeoFrame.GeoDataFrame.new(Explorer.DataFrame.new(id: Enum.to_list(0..4)), geometry)
      iex> :ok = GeoFrame.write_file(gdf, path)
      iex> batches = path |> GeoFrame.stream_file(batch_size: 2) |> Enum.to_list()
      iex> Enum.map(batches, &GeoFrame.GeoDataFrame.n_rows/1)
      [2, 2, 1]
      iex> GeoFrame.GeoDataFrame.crs(hd(batches))
      "EPSG:4326"
      iex> [first] = path |> GeoFrame.stream_file(batch_size: 2) |> Enum.take(1)
      iex> first |> GeoFrame.GeoDataFrame.geometry() |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(0 0)", "POINT(1 0)"]
      iex> File.rm!(path)
      :ok
  """
  @spec stream_file(Path.t(), keyword()) :: Enumerable.t(GeoDataFrame.t())
  def stream_file(path, opts \\ []), do: Formats.stream_file(path, opts)

  @doc """
  Writes a vector file, choosing the format from the extension or `driver:`; see
  `read_file/2`. The remaining options go to `write_parquet/3`, `write_geojson/3`,
  `write_flatgeobuf/3`, `write_feather/3`, `write_shapefile/3` or `write_gpkg/3`.
  """
  @spec write_file(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_file(%GeoDataFrame{} = gdf, path, opts \\ []), do: Formats.write_file(gdf, path, opts)

  @doc """
  The `geo` file metadata of a GeoParquet file as a decoded map (string keys): `"version"`,
  `"primary_column"` and `"columns"` with each column's `"encoding"`, `"geometry_types"`,
  `"crs"`, `"bbox"`, `"covering"` and so on, exactly as stored. Only the Parquet footer is
  read. Returns `{:error, message}` when the file cannot be read or has no `geo` metadata
  (plain Parquet).
  """
  @spec geoparquet_metadata(Path.t()) :: {:ok, map()} | {:error, String.t()}
  def geoparquet_metadata(path) do
    with {:ok, json} <- GeoFrame.Native.read_geoparquet_metadata(to_string(path)) do
      {:ok, Jason.decode!(json)}
    end
  end

  @doc "Like `geoparquet_metadata/1` but raises `ArgumentError` on failure."
  @spec geoparquet_metadata!(Path.t()) :: map()
  def geoparquet_metadata!(path) do
    case geoparquet_metadata(path) do
      {:ok, metadata} -> metadata
      {:error, message} -> raise ArgumentError, message
    end
  end

  defp unwrap!({:ok, gdf}), do: gdf
  defp unwrap!({:error, message}), do: raise(ArgumentError, message)

  @doc "Group-wise union with attribute aggregation; see `GeoFrame.Dissolve.dissolve/2`."
  @spec dissolve(GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  defdelegate dissolve(gdf, opts \\ []), to: GeoFrame.Dissolve

  @doc "Marks the groups of a frame; see `GeoFrame.GroupBy.group_by/2`."
  @spec group_by(GeoDataFrame.t(), term()) :: GeoDataFrame.t()
  defdelegate group_by(gdf, columns), to: GeoFrame.GroupBy

  @doc "Aggregates every group into one row, attributes and geometry; see `GeoFrame.GroupBy.summarise/3`."
  @spec summarise(GeoDataFrame.t(), keyword() | map(), keyword()) :: GeoDataFrame.t()
  defdelegate summarise(gdf, aggregations \\ [], opts \\ []), to: GeoFrame.GroupBy

  @doc "Removes the groups; see `GeoFrame.GroupBy.ungroup/2`."
  @spec ungroup(GeoDataFrame.t(), term()) :: GeoDataFrame.t()
  defdelegate ungroup(gdf, columns \\ ..), to: GeoFrame.GroupBy

  @doc "Applies a function per group (pandas `groupby.apply`); see `GeoFrame.GroupBy.group_map/4`."
  @spec group_map(
          GeoDataFrame.t(),
          term(),
          (GeoDataFrame.t() -> GeoDataFrame.t() | Explorer.DataFrame.t()),
          keyword()
        ) :: GeoDataFrame.t() | Explorer.DataFrame.t()
  defdelegate group_map(gdf, by, fun, opts \\ []), to: GeoFrame.GroupBy
end
