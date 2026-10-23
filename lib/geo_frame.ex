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

  `read_file/2` and `write_file/3` pick the format from the extension or from the
  `:driver` option. All three formats share one data path: geometry stays in Rust, and the
  other columns cross the NIF boundary as an Arrow IPC stream. Shapefile is not supported.

  ## GeoParquet

  `read_parquet/2` and `write_parquet/3` split a file in two halves. The geometry column
  lives in Rust memory (a `GeoFrame.GeoSeries`); every other column crosses the NIF
  boundary as an Arrow IPC stream and is loaded into (or dumped from) an
  `Explorer.DataFrame`. Explorer has no zero-copy NIF access to Polars memory, so each
  direction costs one serialisation and one copy of the attribute data. That is acceptable
  by design: geometry, the bulky part, is never copied into Elixir.

  Files are read into memory whole (on a dirty IO scheduler); batched or streaming reads
  are a known gap. Read supports WKB and the GeoArrow-native GeoParquet encodings. Write
  produces GeoParquet 1.1.0 with WKB geometry, Snappy compression and no covering columns.

  ### Coordinate reference system

  GeoParquet 1.1 semantics apply when reading: a missing `crs` key means `"OGC:CRS84"`,
  an explicit `null` means unknown (`nil`). PROJJSON that carries an identifier is turned
  into an `"AUTHORITY:CODE"` string, other PROJJSON is kept as JSON text. When writing,
  `"OGC:CRS84"` omits the key, `nil` writes an explicit `null`, `"EPSG:4326"` gets a full
  PROJJSON definition, other `"AUTHORITY:CODE"` strings are written as a PROJJSON stub with
  only a name and an identifier (the pure-Rust build has no CRS database), and a PROJJSON
  string is written as given. Any other CRS string is rejected.

  ### Column order

  The geometry column is written last and read back as the geometry of the frame, so its
  original position among the columns is not preserved.
  """

  alias GeoFrame.Attributes
  alias GeoFrame.Formats
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

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
  Reads a GeoParquet file into a `GeoFrame.GeoDataFrame`.

  The primary geometry column named in the file's `geo` metadata becomes the geometry;
  all other columns become attributes.

  Options: `:columns` - attribute column names to keep (the geometry is always read);
  they keep their file order.

  Returns `{:error, message}` when the file cannot be opened, is not Parquet, has no `geo`
  metadata, or cannot be decoded.
  """
  @spec read_parquet(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_parquet(path, opts \\ []) do
    columns = opts |> Keyword.get(:columns) |> normalize_columns()

    with {:ok, {ref, geometry_column, ipc}} <- Native.read_parquet(to_string(path), columns),
         {:ok, data} <- Attributes.load(ipc) do
      {:ok, GeoDataFrame.new(data, %GeoSeries{ref: ref}, geometry_column: geometry_column)}
    end
  end

  @doc "Like `read_parquet/2` but raises `ArgumentError` on failure."
  @spec read_parquet!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_parquet!(path, opts \\ []), do: unwrap!(read_parquet(path, opts))

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a GeoParquet 1.1 file (WKB geometry).

  Returns `:ok` or `{:error, message}`; a partially written file is removed.
  """
  @spec write_parquet(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_parquet(%GeoDataFrame{} = gdf, path, _opts \\ []) do
    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, _} <-
           Native.write_parquet(gdf.geometry.ref, gdf.geometry_column, ipc, to_string(path)) do
      :ok
    end
  end

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
  Coordinates are 2D. A frame with no rows has no properties, so its columns are lost.

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

    * `:bbox` - `{minx, miny, maxx, maxy}`: only features whose bounding box intersects it
      are read, using the file's spatial index (an error if the file has none).
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
  multi types). Coordinates are 2D and null geometries are stored as empty ones.

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
  Reads a vector file, choosing the format from the extension (`.parquet`, `.geoparquet`,
  `.geojson`, `.json`, `.fgb`; case-insensitive) or from `driver:` (`:parquet`, `:geojson`,
  `:flatgeobuf`), which wins over the extension.

  The remaining options go to the format's reader (`read_parquet/2`, `read_geojson/2`,
  `read_flatgeobuf/2`); unknown options for GeoJSON and FlatGeobuf raise `ArgumentError`.
  An unrecognised extension or driver raises `ArgumentError`; read failures are
  `{:error, message}`.
  """
  @spec read_file(Path.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read_file(path, opts \\ []), do: Formats.read_file(path, opts)

  @doc "Like `read_file/2` but raises `ArgumentError` on failure."
  @spec read_file!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_file!(path, opts \\ []), do: unwrap!(read_file(path, opts))

  @doc """
  Writes a vector file, choosing the format from the extension or `driver:`; see
  `read_file/2`. The remaining options go to `write_parquet/3`, `write_geojson/3` or
  `write_flatgeobuf/3`.
  """
  @spec write_file(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_file(%GeoDataFrame{} = gdf, path, opts \\ []), do: Formats.write_file(gdf, path, opts)

  defp unwrap!({:ok, gdf}), do: gdf
  defp unwrap!({:error, message}), do: raise(ArgumentError, message)

  defp normalize_columns(nil), do: nil
  defp normalize_columns(columns) when is_list(columns), do: Enum.map(columns, &to_string/1)

  @doc "Group-wise union with attribute aggregation; see `GeoFrame.Dissolve.dissolve/2`."
  @spec dissolve(GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  defdelegate dissolve(gdf, opts \\ []), to: GeoFrame.Dissolve
end
