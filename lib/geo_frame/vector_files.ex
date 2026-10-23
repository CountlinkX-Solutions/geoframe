defmodule GeoFrame.VectorFiles do
  @moduledoc false
  # Shapefile and GeoPackage readers/writers, `list_layers/2` and `read_file_info/2`.
  # Public documentation lives on the delegating functions in `GeoFrame`.

  require Logger

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.Attributes
  alias GeoFrame.Formats
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @modes [:w, :a]

  # -- Shapefile ------------------------------------------------------------------------

  def read_shapefile(path, opts) do
    Formats.validate_options!(opts, [:encoding])
    encoding = opts |> Keyword.get(:encoding) |> optional_string()

    with {:ok, {ref, ipc}} <- Native.read_shapefile_file(to_string(path), encoding) do
      Formats.assemble(ref, ipc, [])
    end
  end

  def write_shapefile(%GeoDataFrame{} = gdf, path, opts) do
    Formats.validate_options!(opts, [:encoding])
    encoding = opts |> Keyword.get(:encoding) |> optional_string()
    check_single_shape_type!(gdf)

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, warnings} <-
           Native.write_shapefile_file(gdf.geometry.ref, ipc, to_string(path), encoding) do
      log_warnings(warnings, "shapefile")
    end
  end

  # A shapefile holds one shape type; GDAL refuses a mixed frame, so does GeoFrame.
  defp check_single_shape_type!(%GeoDataFrame{geometry: geometry}) do
    kinds =
      geometry
      |> GeoSeries.geom_type()
      |> Series.to_list()
      |> Enum.reject(&is_nil/1)
      |> Enum.map(&shape_kind/1)
      |> Enum.uniq()

    case kinds do
      [] ->
        :ok

      [:collection] ->
        raise ArgumentError, "a shapefile cannot store a GeometryCollection"

      [_single] ->
        :ok

      mixed ->
        raise ArgumentError,
              "a shapefile holds a single geometry type, but the geometries mix " <>
                Enum.map_join(mixed, " and ", &Atom.to_string/1) <>
                "; split the frame by geometry type or use write_gpkg/3"
    end
  end

  defp shape_kind("Point"), do: :point
  defp shape_kind("MultiPoint"), do: :multipoint
  defp shape_kind(type) when type in ["LineString", "MultiLineString", "LinearRing"], do: :line
  defp shape_kind(type) when type in ["Polygon", "MultiPolygon"], do: :polygon
  defp shape_kind(_), do: :collection

  # -- GeoPackage -----------------------------------------------------------------------

  def read_gpkg(path, opts) do
    Formats.validate_options!(opts, [:layer])
    layer = opts |> Keyword.get(:layer) |> optional_string()

    with {:ok, {ref, ipc, _layer}} <- Native.read_gpkg_file(to_string(path), layer) do
      Formats.assemble(ref, ipc, [])
    end
  end

  def write_gpkg(%GeoDataFrame{} = gdf, path, opts) do
    Formats.validate_options!(opts, [:layer, :mode, :spatial_index])
    path = to_string(path)
    layer = Keyword.get(opts, :layer, path |> Path.basename() |> Path.rootname())
    mode = Keyword.get(opts, :mode, :w)
    index = Keyword.get(opts, :spatial_index, true)

    unless is_binary(layer) and layer != "" do
      raise ArgumentError, ":layer must be a non-empty string, got: #{inspect(layer)}"
    end

    unless mode in @modes do
      raise ArgumentError, "invalid :mode #{inspect(mode)}; expected one of #{inspect(@modes)}"
    end

    unless is_boolean(index) do
      raise ArgumentError, ":spatial_index must be a boolean, got: #{inspect(index)}"
    end

    with {:ok, ipc} <- Attributes.dump(gdf.data),
         {:ok, warnings} <-
           Native.write_gpkg_file(gdf.geometry.ref, ipc, path, layer, mode == :a, index) do
      log_warnings(warnings, "GeoPackage")
    end
  end

  # -- layers and file info -------------------------------------------------------------

  def list_layers(path, driver) do
    path = to_string(path)

    {names, types} =
      case driver do
        :gpkg ->
          path |> Native.gpkg_layer_list() |> unwrap!() |> Enum.unzip()

        :shapefile ->
          info = path |> Native.shapefile_file_info(nil) |> unwrap!()
          {[info.name], [info.geometry_type]}

        _other ->
          gdf = read_any!(path, driver, [])
          {[stem(path)], [common_type(gdf)]}
      end

    DataFrame.new(
      name: Series.from_list(names, dtype: :string),
      geometry_type: Series.from_list(types, dtype: :string)
    )
  end

  def read_file_info(path, driver, opts) do
    path = to_string(path)

    case driver do
      :gpkg ->
        layer = opts |> Keyword.get(:layer) |> optional_string()
        path |> Native.gpkg_layer_info(layer) |> unwrap!() |> format_info()

      :shapefile ->
        encoding = opts |> Keyword.get(:encoding) |> optional_string()
        path |> Native.shapefile_file_info(encoding) |> unwrap!() |> format_info()

      other ->
        gdf = read_any!(path, other, [])
        data = GeoDataFrame.data(gdf)

        %{
          driver: driver_name(other),
          layer: stem(path),
          crs: GeoDataFrame.crs(gdf),
          encoding: "UTF-8",
          geometry_type: common_type(gdf),
          features: DataFrame.n_rows(data),
          total_bounds: GeoSeries.total_bounds(GeoDataFrame.geometry(gdf)),
          fields: DataFrame.names(data),
          dtypes: data |> DataFrame.dtypes() |> Map.values() |> Enum.map(&dtype_name/1)
        }
    end
  end

  defp format_info(info) do
    %{
      driver: info.driver,
      layer: info.name,
      crs: info.crs,
      encoding: info.encoding,
      geometry_type: info.geometry_type,
      features: info.features,
      total_bounds: info.bounds,
      fields: Enum.map(info.fields, &elem(&1, 0)),
      dtypes: Enum.map(info.fields, &elem(&1, 1))
    }
  end

  defp read_any!(path, driver, opts) do
    case Formats.read_file(path, Keyword.put(opts, :driver, driver)) do
      {:ok, gdf} -> gdf
      {:error, message} -> raise ArgumentError, message
    end
  end

  defp common_type(gdf) do
    types =
      gdf
      |> GeoDataFrame.geometry()
      |> GeoSeries.geom_type()
      |> Series.to_list()
      |> Enum.reject(&is_nil/1)
      |> Enum.uniq()

    case types do
      [] -> nil
      [single] -> single
      _mixed -> "Unknown"
    end
  end

  defp stem(path), do: path |> Path.basename() |> Path.rootname()

  defp driver_name(:parquet), do: "Parquet"
  defp driver_name(:geojson), do: "GeoJSON"
  defp driver_name(:flatgeobuf), do: "FlatGeobuf"
  defp driver_name(:feather), do: "Arrow"

  defp dtype_name({:s, bits}), do: "int#{bits}"
  defp dtype_name({:u, bits}), do: "uint#{bits}"
  defp dtype_name({:f, bits}), do: "float#{bits}"
  defp dtype_name(:boolean), do: "bool"
  defp dtype_name(:date), do: "datetime64[D]"
  defp dtype_name({:naive_datetime, unit}), do: "datetime64[#{unit_suffix(unit)}]"
  defp dtype_name({:datetime, unit, _zone}), do: "datetime64[#{unit_suffix(unit)}]"
  defp dtype_name(_other), do: "object"

  defp unit_suffix(:millisecond), do: "ms"
  defp unit_suffix(:microsecond), do: "us"
  defp unit_suffix(:nanosecond), do: "ns"

  # -- helpers --------------------------------------------------------------------------

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, message}), do: raise(ArgumentError, message)

  defp optional_string(nil), do: nil
  defp optional_string(value), do: to_string(value)

  # Native warnings are conditions the write recovered from (a truncated name, a CRS without
  # a definition); the write still succeeded.
  defp log_warnings(warnings, format) do
    Enum.each(warnings, &Logger.warning("GeoFrame #{format}: #{&1}"))
    :ok
  end
end
