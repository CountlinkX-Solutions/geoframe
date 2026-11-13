defmodule GeoFrame.Formats do
  @moduledoc false
  # GeoParquet, Feather, Arrow IPC, GeoJSON and FlatGeobuf readers/writers and the
  # (shapefile and GeoPackage live in `GeoFrame.VectorFiles`)
  # extension/driver dispatch behind `GeoFrame.read_file/2` and `GeoFrame.write_file/3`.
  # Public documentation lives on the delegating functions in `GeoFrame`.

  alias GeoFrame.Attributes
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @drivers [:parquet, :geojson, :flatgeobuf, :feather, :shapefile, :gpkg]

  @extensions %{
    ".parquet" => :parquet,
    ".geoparquet" => :parquet,
    ".geojson" => :geojson,
    ".json" => :geojson,
    ".fgb" => :flatgeobuf,
    ".feather" => :feather,
    ".arrow" => :feather,
    ".shp" => :shapefile,
    ".zip" => :shapefile,
    ".gpkg" => :gpkg
  }

  @parquet_compressions [:snappy, :zstd, :gzip, :lz4, :brotli, :none]
  @feather_compressions [:lz4, :zstd, :uncompressed]
  @encodings [:wkb, :geoarrow]
  @schema_versions ["1.0.0", "1.1.0"]

  # -- GeoParquet -----------------------------------------------------------------------

  def read_parquet(path, opts) do
    validate_options!(opts, [:columns, :bbox])
    columns = opts |> Keyword.get(:columns) |> normalize_columns()
    bbox = opts |> Keyword.get(:bbox) |> normalize_bbox!() |> check_bbox_order!()

    with {:ok, {ref, name, ipc, secondary}} <-
           Native.read_parquet(to_string(path), columns, bbox),
         {:ok, gdf} <- assemble_named(ref, name, ipc) do
      {:ok, put_secondary(gdf, secondary)}
    end
  end

  def write_parquet(%GeoDataFrame{} = gdf, path, opts) do
    validate_options!(opts, [
      :compression,
      :compression_level,
      :row_group_size,
      :geometry_encoding,
      :write_covering_bbox,
      :schema_version
    ])

    compression = enum!(opts, :compression, @parquet_compressions, :snappy)
    level = optional_integer!(opts, :compression_level)
    row_group_size = optional_positive_integer!(opts, :row_group_size)
    encoding = enum!(opts, :geometry_encoding, @encodings, :wkb)
    covering = boolean!(opts, :write_covering_bbox, false)
    version = enum!(opts, :schema_version, @schema_versions, "1.1.0")

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, _} <-
           Native.write_parquet(
             gdf.geometry.ref,
             gdf.geometry_column,
             ipc,
             to_string(path),
             {Atom.to_string(compression), level, row_group_size, Atom.to_string(encoding),
              covering, version},
             secondary_refs(gdf)
           ) do
      :ok
    end
  end

  # -- Feather and Arrow IPC ------------------------------------------------------------

  def read_feather(path, opts) do
    validate_options!(opts, [:columns, :geometry])
    columns = opts |> Keyword.get(:columns) |> normalize_columns()
    geometry = opts |> Keyword.get(:geometry) |> optional_string()

    with {:ok, {ref, name, ipc, secondary}} <-
           Native.read_feather_file(to_string(path), columns, geometry),
         {:ok, gdf} <- assemble_named(ref, name, ipc) do
      {:ok, put_secondary(gdf, secondary)}
    end
  end

  def write_feather(%GeoDataFrame{} = gdf, path, opts) do
    validate_options!(opts, [:compression, :geometry_encoding])
    compression = enum!(opts, :compression, @feather_compressions, :lz4)
    encoding = enum!(opts, :geometry_encoding, @encodings, :wkb)

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, _} <-
           Native.write_feather_file(
             gdf.geometry.ref,
             gdf.geometry_column,
             ipc,
             to_string(path),
             Atom.to_string(encoding),
             Atom.to_string(compression),
             secondary_refs(gdf)
           ) do
      :ok
    end
  end

  def to_arrow(%GeoDataFrame{} = gdf, opts) do
    validate_options!(opts, [:geometry_encoding])
    encoding = enum!(opts, :geometry_encoding, @encodings, :wkb)

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, bytes} <-
           Native.to_arrow_stream(
             gdf.geometry.ref,
             gdf.geometry_column,
             ipc,
             Atom.to_string(encoding),
             secondary_refs(gdf)
           ) do
      bytes
    else
      {:error, message} -> raise ArgumentError, message
    end
  end

  def series_to_arrow(%GeoSeries{ref: ref}, opts) do
    validate_options!(opts, [:geometry_encoding])
    encoding = enum!(opts, :geometry_encoding, @encodings, :wkb)

    case Native.to_arrow_stream(ref, "geometry", <<>>, Atom.to_string(encoding), []) do
      {:ok, bytes} -> bytes
      {:error, message} -> raise ArgumentError, message
    end
  end

  def from_arrow(bytes, opts) when is_binary(bytes) do
    validate_options!(opts, [:columns, :geometry, :crs])
    columns = opts |> Keyword.get(:columns) |> normalize_columns()
    geometry = opts |> Keyword.get(:geometry) |> optional_string()
    crs = Keyword.get(opts, :crs)

    with {:ok, {ref, name, ipc, secondary}} <- Native.from_arrow_stream(bytes, columns, geometry),
         {:ok, gdf} <- assemble_named(ref, name, ipc) do
      {:ok, gdf |> put_secondary(secondary) |> default_crs(crs)}
    end
  end

  def series_from_arrow(bytes, opts) when is_binary(bytes) do
    validate_options!(opts, [:geometry, :crs])
    geometry = opts |> Keyword.get(:geometry) |> optional_string()

    with {:ok, {ref, _name, _ipc, _secondary}} <- Native.from_arrow_stream(bytes, [], geometry) do
      series = %GeoSeries{ref: ref}

      case {GeoSeries.crs(series), Keyword.get(opts, :crs)} do
        {nil, crs} when is_binary(crs) -> {:ok, GeoSeries.set_crs(series, crs)}
        _ -> {:ok, series}
      end
    end
  end

  # An explicit `crs:` only fills in a missing CRS: the table's own metadata wins.
  defp default_crs(%GeoDataFrame{} = gdf, crs) when is_binary(crs) do
    if GeoDataFrame.crs(gdf) == nil, do: GeoDataFrame.set_crs(gdf, crs), else: gdf
  end

  defp default_crs(gdf, _crs), do: gdf

  # -- GeoJSON --------------------------------------------------------------------------

  def read_geojson(path, opts) do
    validate_options!(opts, [:geometry_column])

    with {:ok, {ref, ipc}} <- Native.read_geojson_file(to_string(path)) do
      assemble(ref, ipc, opts)
    end
  end

  def write_geojson(%GeoDataFrame{} = gdf, path, opts) do
    validate_options!(opts, [:force])
    single_geometry!(gdf, "write_geojson")
    force = Keyword.get(opts, :force, false) == true

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, _} <- Native.write_geojson_file(gdf.geometry.ref, ipc, to_string(path), force) do
      :ok
    end
  end

  # -- FlatGeobuf -----------------------------------------------------------------------

  def read_flatgeobuf(path, opts) do
    validate_options!(opts, [:geometry_column, :bbox])
    bbox = opts |> Keyword.get(:bbox) |> normalize_bbox!()

    with {:ok, {ref, ipc}} <- Native.read_flatgeobuf_file(to_string(path), bbox) do
      assemble(ref, ipc, opts)
    end
  end

  def write_flatgeobuf(%GeoDataFrame{} = gdf, path, opts) do
    validate_options!(opts, [:index, :name])
    single_geometry!(gdf, "write_flatgeobuf")
    index = Keyword.get(opts, :index, true) != false
    path = to_string(path)
    name = opts |> Keyword.get(:name, path |> Path.basename() |> Path.rootname()) |> to_string()

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, _} <- Native.write_flatgeobuf_file(gdf.geometry.ref, ipc, path, name, index) do
      :ok
    end
  end

  # -- dispatch -------------------------------------------------------------------------

  def read_file(path, opts) do
    {driver, opts} = driver_for(path, opts)

    case driver do
      :parquet -> GeoFrame.read_parquet(path, opts)
      :geojson -> read_geojson(path, opts)
      :flatgeobuf -> read_flatgeobuf(path, opts)
      :feather -> read_feather(path, opts)
      :shapefile -> GeoFrame.VectorFiles.read_shapefile(path, opts)
      :gpkg -> GeoFrame.VectorFiles.read_gpkg(path, opts)
    end
  end

  def write_file(%GeoDataFrame{} = gdf, path, opts) do
    {driver, opts} = driver_for(path, opts)

    case driver do
      :parquet -> GeoFrame.write_parquet(gdf, path, opts)
      :geojson -> write_geojson(gdf, path, opts)
      :flatgeobuf -> write_flatgeobuf(gdf, path, opts)
      :feather -> write_feather(gdf, path, opts)
      :shapefile -> GeoFrame.VectorFiles.write_shapefile(gdf, path, opts)
      :gpkg -> GeoFrame.VectorFiles.write_gpkg(gdf, path, opts)
    end
  end

  @doc false
  def driver_for(path, opts) do
    case Keyword.pop(opts, :driver) do
      {nil, rest} ->
        {driver_from_extension!(path), rest}

      {driver, rest} when driver in @drivers ->
        {driver, rest}

      {other, _rest} ->
        raise ArgumentError,
              "unknown driver #{inspect(other)}; expected one of #{inspect(@drivers)}"
    end
  end

  defp driver_from_extension!(path) do
    extension = path |> to_string() |> Path.extname() |> String.downcase()

    case Map.fetch(@extensions, extension) do
      {:ok, driver} ->
        driver

      :error ->
        shown = if extension == "", do: "no extension", else: "extension #{extension}"

        raise ArgumentError,
              "cannot infer the format of #{inspect(to_string(path))} (#{shown}); " <>
                "use .parquet, .geoparquet, .geojson, .json, .fgb, .feather, .arrow, .shp, .zip or .gpkg, or pass driver: " <>
                "#{Enum.map_join(@drivers, " | ", &inspect/1)}"
    end
  end

  # -- streaming ------------------------------------------------------------------------

  @default_batch_size 65_536
  @streamable [:parquet, :flatgeobuf, :feather]

  def stream_file(path, opts) do
    {driver, opts} = driver_for(path, opts)

    unless driver in @streamable do
      raise ArgumentError,
            "stream_file/2 does not support the #{inspect(driver)} format yet " <>
              "(it streams #{Enum.map_join(@streamable, ", ", &inspect/1)}); " <>
              "use read_file/2 to read the file whole"
    end

    {batch_size, opts} = Keyword.pop(opts, :batch_size, @default_batch_size)

    unless is_integer(batch_size) and batch_size > 0 do
      raise ArgumentError, "batch_size must be a positive integer, got: #{inspect(batch_size)}"
    end

    path = to_string(path)
    open = stream_opener(driver, path, opts, batch_size)

    Stream.resource(
      fn -> unwrap_open!(open.()) end,
      fn reader ->
        case Native.next_batch(reader) do
          {:ok, nil} -> {:halt, reader}
          {:ok, batch} -> {[unwrap_batch!(batch, driver, opts)], reader}
          {:error, message} -> raise ArgumentError, message
        end
      end,
      &Native.close_reader/1
    )
  end

  defp stream_opener(:parquet, path, opts, batch_size) do
    validate_options!(opts, [:columns, :bbox])
    columns = opts |> Keyword.get(:columns) |> normalize_columns()
    bbox = opts |> Keyword.get(:bbox) |> normalize_bbox!() |> check_bbox_order!()
    fn -> Native.open_parquet_reader(path, columns, bbox, batch_size) end
  end

  defp stream_opener(:flatgeobuf, path, opts, batch_size) do
    validate_options!(opts, [:geometry_column, :bbox])
    bbox = opts |> Keyword.get(:bbox) |> normalize_bbox!()
    fn -> Native.open_flatgeobuf_reader(path, bbox, batch_size) end
  end

  defp stream_opener(:feather, path, opts, batch_size) do
    validate_options!(opts, [:columns, :geometry])
    columns = opts |> Keyword.get(:columns) |> normalize_columns()
    geometry = opts |> Keyword.get(:geometry) |> optional_string()
    fn -> Native.open_feather_reader(path, columns, geometry, batch_size) end
  end

  defp unwrap_open!({:ok, reader}), do: reader
  defp unwrap_open!({:error, message}), do: raise(ArgumentError, message)

  defp unwrap_batch!({ref, nil, ipc, _secondary}, :flatgeobuf, opts) do
    case assemble(ref, ipc, opts) do
      {:ok, gdf} -> gdf
      {:error, message} -> raise ArgumentError, message
    end
  end

  defp unwrap_batch!({ref, name, ipc, secondary}, driver, _opts)
       when driver in [:parquet, :feather] do
    case assemble_named(ref, name, ipc) do
      {:ok, gdf} -> put_secondary(gdf, secondary)
      {:error, message} -> raise ArgumentError, message
    end
  end

  # -- helpers --------------------------------------------------------------------------

  @doc """
  Raises `ArgumentError` when `gdf` has secondary geometry columns: the formats that hold
  one geometry column per feature (GeoPackage, GeoJSON, FlatGeobuf, Shapefile, PostGIS, JSON)
  cannot store them, and GeoPandas `to_file` refuses such a frame the same way.
  """
  @spec single_geometry!(GeoDataFrame.t(), String.t()) :: :ok
  def single_geometry!(%GeoDataFrame{secondary_geometries: []}, _what), do: :ok

  def single_geometry!(%GeoDataFrame{} = gdf, what) do
    names = gdf |> GeoDataFrame.geometry_columns() |> Enum.map_join(", ", &inspect/1)

    raise ArgumentError,
          "GeoDataFrame contains multiple geometry columns (#{names}) but #{what} supports only " <>
            "a single geometry column. Use write_parquet/3 or write_feather/3, drop the " <>
            "additional geometry columns (GeoDataFrame.discard/2) or convert them to a " <>
            "supported format like well-known text (GeoSeries.to_wkt/1)"
  end

  defp secondary_refs(%GeoDataFrame{secondary_geometries: secondary}),
    do: Enum.map(secondary, fn {name, %GeoSeries{ref: ref}} -> {name, ref} end)

  defp put_secondary(gdf, secondary) do
    Enum.reduce(secondary, gdf, fn {name, ref}, acc ->
      GeoDataFrame.put_geometry(acc, name, %GeoSeries{ref: ref})
    end)
  end

  defp assemble_named(ref, name, ipc) do
    with {:ok, data} <- Attributes.load(ipc) do
      {:ok, GeoDataFrame.new(data, %GeoSeries{ref: ref}, geometry_column: name)}
    end
  end

  @doc false
  def assemble(ref, ipc, opts) do
    with {:ok, data} <- Attributes.load(ipc),
         {:ok, column} <- geometry_column(Explorer.DataFrame.names(data), opts) do
      {:ok, GeoDataFrame.new(data, %GeoSeries{ref: ref}, geometry_column: column)}
    end
  end

  # An explicit name must be free; the default steps aside for a property of the same name.
  defp geometry_column(names, opts) do
    case Keyword.fetch(opts, :geometry_column) do
      {:ok, column} ->
        column = to_string(column)

        if column in names,
          do: {:error, "the file has a column named #{inspect(column)} (geometry column)"},
          else: {:ok, column}

      :error ->
        {:ok, Enum.find(["geometry" | Enum.map(1..1000, &"geometry_#{&1}")], &(&1 not in names))}
    end
  end

  defp normalize_bbox!(nil), do: nil

  defp normalize_bbox!({minx, miny, maxx, maxy})
       when is_number(minx) and is_number(miny) and is_number(maxx) and is_number(maxy) do
    {minx * 1.0, miny * 1.0, maxx * 1.0, maxy * 1.0}
  end

  defp normalize_bbox!(other),
    do: raise(ArgumentError, "bbox must be {minx, miny, maxx, maxy}, got: #{inspect(other)}")

  defp check_bbox_order!(nil), do: nil

  defp check_bbox_order!({minx, miny, maxx, maxy} = bbox) do
    if minx <= maxx and miny <= maxy,
      do: bbox,
      else:
        raise(
          ArgumentError,
          "bbox must have minx <= maxx and miny <= maxy, got: #{inspect(bbox)}"
        )
  end

  defp normalize_columns(nil), do: nil
  defp normalize_columns(columns) when is_list(columns), do: Enum.map(columns, &to_string/1)

  defp optional_string(nil), do: nil
  defp optional_string(value), do: to_string(value)

  defp enum!(opts, key, allowed, default) do
    value = Keyword.get(opts, key, default)

    if value in allowed do
      value
    else
      raise ArgumentError,
            "invalid #{inspect(key)} value #{inspect(value)}; expected one of #{inspect(allowed)}"
    end
  end

  defp optional_integer!(opts, key) do
    case Keyword.get(opts, key) do
      nil -> nil
      value when is_integer(value) -> value
      other -> raise ArgumentError, "#{inspect(key)} must be an integer, got: #{inspect(other)}"
    end
  end

  defp optional_positive_integer!(opts, key) do
    case Keyword.get(opts, key) do
      nil ->
        nil

      value when is_integer(value) and value > 0 ->
        value

      other ->
        raise ArgumentError, "#{inspect(key)} must be a positive integer, got: #{inspect(other)}"
    end
  end

  defp boolean!(opts, key, default) do
    case Keyword.get(opts, key, default) do
      value when is_boolean(value) -> value
      other -> raise ArgumentError, "#{inspect(key)} must be a boolean, got: #{inspect(other)}"
    end
  end

  @doc false
  def validate_options!(opts, allowed) do
    GeoFrame.Options.validate!(opts, allowed)
    :ok
  end
end
