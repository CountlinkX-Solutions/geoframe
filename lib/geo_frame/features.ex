defmodule GeoFrame.Features do
  @moduledoc false
  # GeoJSON FeatureCollections as GeoPandas builds them for `to_json` and `__geo_interface__`:
  # one `Feature` per row with a string `id` (the row position), `properties`, the `geometry`
  # (`null` for null or unrepresentable rows) and, with `show_bbox`, a `bbox` per feature and
  # for the whole collection. Shared by `GeoSeries` and `GeoDataFrame`.

  alias Explorer.DataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Native

  @wgs84 "EPSG:4326"
  @missing_floats [:nan, :infinity, :neg_infinity]

  @doc """
  Builds the collection. `format` is `:json` (a JSON string, keys in GeoPandas' order, the
  `crs` member added for a CRS other than EPSG:4326) or `:map` (string-keyed maps, no `crs`).
  `data` is the attribute frame, or `nil` for a series (empty `properties`).
  """
  @spec build(GeoSeries.t(), DataFrame.t() | nil, keyword(), :json | :map) :: String.t() | map()
  def build(%GeoSeries{} = geometry, data, opts, format) do
    na = Keyword.fetch!(opts, :na)
    geometry = if opts[:to_wgs84], do: GeoSeries.to_crs(geometry, @wgs84), else: geometry
    geometries = geometries(geometry, format)
    properties = properties(data, length(geometries), na, format)

    boxes =
      if opts[:show_bbox], do: bounds(geometry), else: List.duplicate(:none, length(geometries))

    features =
      [geometries, properties, boxes]
      |> Enum.zip()
      |> Enum.with_index()
      |> Enum.map(fn {{geom, props, box}, index} ->
        feature(index, geom, props, box, opts[:drop_id], format)
      end)

    collection(features, geometry, opts, format)
  end

  @doc "Validates the options (`na` included) and fills in the defaults."
  @spec options!(keyword(), boolean()) :: keyword()
  def options!(opts, show_bbox_default) do
    opts =
      Keyword.validate!(opts,
        na: :null,
        show_bbox: show_bbox_default,
        drop_id: false,
        to_wgs84: false
      )

    check_na!(opts[:na])
    Enum.each([:drop_id, :to_wgs84, :show_bbox], &check_boolean!(opts, &1))
    opts
  end

  defp check_na!(na) when na in [:null, :drop], do: :ok

  defp check_na!(:keep) do
    raise ArgumentError,
          "na: :keep is not supported because JSON has no NaN; use :null or :drop"
  end

  defp check_na!(other) do
    raise ArgumentError, "na must be :null or :drop, got: #{inspect(other)}"
  end

  defp check_boolean!(opts, key) do
    unless is_boolean(Keyword.fetch!(opts, key)) do
      raise ArgumentError, "#{key} must be a boolean, got: #{inspect(opts[key])}"
    end
  end

  # -- pieces ---------------------------------------------------------------------------

  defp geometries(%GeoSeries{ref: ref}, :json) do
    ref |> Native.to_geojson() |> Enum.map(&(&1 && Jason.Fragment.new(&1)))
  end

  defp geometries(%GeoSeries{} = series, :map), do: GeoSeries.to_geojson(series)

  defp bounds(%GeoSeries{ref: ref}) do
    ref |> Native.bounds() |> Enum.map(&(&1 && Tuple.to_list(&1)))
  end

  defp properties(nil, count, _na, format), do: List.duplicate(object([], format), count)

  defp properties(%DataFrame{} = data, count, na, format) do
    case DataFrame.names(data) do
      [] -> List.duplicate(object([], format), count)
      names -> data |> DataFrame.to_rows() |> Enum.map(&row_properties(&1, names, na, format))
    end
  end

  defp row_properties(row, names, na, format) do
    names
    |> Enum.map(&{&1, clean(Map.fetch!(row, &1))})
    |> Enum.reject(fn {_, value} -> na == :drop and is_nil(value) end)
    |> object(format)
  end

  defp clean(value) when value in @missing_floats, do: nil
  defp clean(value), do: value

  defp feature(index, geometry, properties, box, drop_id, format) do
    [
      {"id", Integer.to_string(index)},
      {"type", "Feature"},
      {"properties", properties},
      {"geometry", geometry},
      {"bbox", box}
    ]
    |> Enum.reject(fn
      {"id", _} -> drop_id
      {"bbox", :none} -> true
      _ -> false
    end)
    |> object(format)
  end

  defp collection(features, geometry, opts, format) do
    pairs =
      [{"type", "FeatureCollection"}, {"features", features}] ++
        total_bbox(geometry, opts[:show_bbox]) ++ crs_member(geometry, format)

    case format do
      :json -> pairs |> object(:json) |> Jason.encode!()
      :map -> object(pairs, :map)
    end
  end

  defp total_bbox(_geometry, false), do: []

  defp total_bbox(geometry, true) do
    case GeoSeries.total_bounds(geometry) do
      nil -> []
      box -> [{"bbox", Tuple.to_list(box)}]
    end
  end

  defp crs_member(geometry, :json) do
    with crs when is_binary(crs) <- GeoSeries.crs(geometry),
         false <- crs == @wgs84,
         [authority, code] <- String.split(crs, ":", parts: 2),
         true <- authority =~ ~r/^[A-Za-z]+$/ and code =~ ~r/^[A-Za-z0-9_]+$/ do
      urn = "urn:ogc:def:crs:#{authority}::#{code}"
      [{"crs", object([{"type", "name"}, {"properties", object([{"name", urn}], :json)}], :json)}]
    else
      _ -> []
    end
  end

  defp crs_member(_geometry, :map), do: []

  defp object(pairs, :json), do: Jason.OrderedObject.new(pairs)
  defp object(pairs, :map), do: Map.new(pairs)
end
