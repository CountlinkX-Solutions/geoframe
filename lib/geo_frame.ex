defmodule GeoFrame do
  @moduledoc """
  GeoPandas-style geospatial dataframes for Elixir, powered by GeoArrow and Rust.

  This module holds file I/O. See `GeoFrame.GeoDataFrame` for the dataframe and
  `GeoFrame.GeoSeries` for geometry columns.

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

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

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
         {:ok, data} <- load_attributes(ipc) do
      {:ok, GeoDataFrame.new(data, %GeoSeries{ref: ref}, geometry_column: geometry_column)}
    end
  end

  @doc "Like `read_parquet/2` but raises `ArgumentError` on failure."
  @spec read_parquet!(Path.t(), keyword()) :: GeoDataFrame.t()
  def read_parquet!(path, opts \\ []) do
    case read_parquet(path, opts) do
      {:ok, gdf} -> gdf
      {:error, message} -> raise ArgumentError, message
    end
  end

  @doc """
  Writes a `GeoFrame.GeoDataFrame` as a GeoParquet 1.1 file (WKB geometry).

  Returns `:ok` or `{:error, message}`; a partially written file is removed.
  """
  @spec write_parquet(GeoDataFrame.t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def write_parquet(%GeoDataFrame{} = gdf, path, _opts \\ []) do
    with {:ok, ipc} <- dump_attributes(gdf.data),
         {:ok, _} <-
           Native.write_parquet(gdf.geometry.ref, gdf.geometry_column, ipc, to_string(path)) do
      :ok
    end
  end

  defp normalize_columns(nil), do: nil
  defp normalize_columns(columns) when is_list(columns), do: Enum.map(columns, &to_string/1)

  defp load_attributes(ipc) do
    case DataFrame.load_ipc_stream(ipc) do
      {:ok, data} -> {:ok, data}
      {:error, reason} -> {:error, "cannot load attribute columns: #{error_message(reason)}"}
    end
  end

  defp dump_attributes(data) do
    case DataFrame.dump_ipc_stream(data) do
      {:ok, ipc} -> {:ok, ipc}
      {:error, reason} -> {:error, "cannot serialise attribute columns: #{error_message(reason)}"}
    end
  end

  defp error_message(reason) when is_binary(reason), do: reason
  defp error_message(%{__exception__: true} = error), do: Exception.message(error)
  defp error_message(reason), do: inspect(reason)

  @doc "Group-wise union with attribute aggregation; see `GeoFrame.Dissolve.dissolve/2`."
  @spec dissolve(GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  defdelegate dissolve(gdf, opts \\ []), to: GeoFrame.Dissolve
end
