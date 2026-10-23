defmodule GeoFrame.Formats do
  @moduledoc false
  # GeoJSON and FlatGeobuf readers/writers and the extension/driver dispatch behind
  # `GeoFrame.read_file/2` and `GeoFrame.write_file/3`. Public documentation lives on the
  # delegating functions in `GeoFrame`.

  alias GeoFrame.Attributes
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @drivers [:parquet, :geojson, :flatgeobuf]

  @extensions %{
    ".parquet" => :parquet,
    ".geoparquet" => :parquet,
    ".geojson" => :geojson,
    ".json" => :geojson,
    ".fgb" => :flatgeobuf
  }

  # -- GeoJSON --------------------------------------------------------------------------

  def read_geojson(path, opts) do
    validate_options!(opts, [:geometry_column])

    with {:ok, {ref, ipc}} <- Native.read_geojson_file(to_string(path)) do
      assemble(ref, ipc, opts)
    end
  end

  def write_geojson(%GeoDataFrame{} = gdf, path, opts) do
    validate_options!(opts, [:force])
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
    end
  end

  def write_file(%GeoDataFrame{} = gdf, path, opts) do
    {driver, opts} = driver_for(path, opts)

    case driver do
      :parquet -> GeoFrame.write_parquet(gdf, path, opts)
      :geojson -> write_geojson(gdf, path, opts)
      :flatgeobuf -> write_flatgeobuf(gdf, path, opts)
    end
  end

  defp driver_for(path, opts) do
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
                "use .parquet, .geoparquet, .geojson, .json or .fgb, or pass driver: " <>
                "#{Enum.map_join(@drivers, " | ", &inspect/1)}"
    end
  end

  # -- helpers --------------------------------------------------------------------------

  defp assemble(ref, ipc, opts) do
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

  defp validate_options!(opts, allowed) do
    unknown = Keyword.keys(opts) -- allowed

    if unknown != [] do
      raise ArgumentError,
            "unknown option(s) #{inspect(unknown)}; supported options: #{inspect(allowed)}"
    end

    :ok
  end
end
