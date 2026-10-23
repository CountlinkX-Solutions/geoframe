if Code.ensure_loaded?(Kino) do
  defmodule GeoFrame.Kino do
    @moduledoc """
    Livebook rendering for `GeoFrame.GeoDataFrame` and `GeoFrame.GeoSeries`.

    This module is compiled only when `:kino` is available. Rendering a frame (the last
    expression of a cell) shows three tabs: **Map**, **Table** and **Inspect**; a series shows
    **Map** and **Inspect**.

    ## Map

    The map needs the optional `:kino_maplibre` and `:maplibre` packages. Without them the Map
    tab explains how to add them and the other tabs still work.

    Data is sent to the browser as one GeoJSON source and styled by geometry kind: polygons
    are filled with an outline, lines are stroked, points are circles. The view is fitted to
    the data's `GeoFrame.GeoSeries.total_bounds/1`. The basemap is MapLibre's demo style, which
    needs internet access in the browser. Coordinates are drawn as stored and must be
    longitude/latitude; Z and M are ignored (the Inspect and Table tabs still show them).

    ### Size limit

    GeoJSON is JSON text held in the notebook, so maps are capped at #{50_000} features by
    default. Larger inputs show a notice instead of a map. Change the cap per call with the
    `:max_features` option of `map/2`, or globally with
    `config :geoframe, max_map_features: n`. A binary GeoArrow layer rendered with deck.gl
    will lift this limit in a later phase.

    The Table tab converts every geometry to WKT up front, which is also unsuited to very
    large frames.
    """

    @default_max_features 50_000
    @viewport {640, 420}
    @max_lat 85.0

    alias GeoFrame.GeoDataFrame
    alias GeoFrame.GeoSeries

    @doc """
    The default cap on features drawn on a map.
    """
    @spec max_features() :: pos_integer()
    def max_features, do: Application.get_env(:geoframe, :max_map_features, @default_max_features)

    @doc """
    Builds the map kino for a `GeoFrame.GeoDataFrame` or `GeoFrame.GeoSeries`.

    Returns a MapLibre kino, or a Markdown notice when the input is too large, has nothing to
    draw, or `:kino_maplibre` is not installed.

    Options: `:max_features` - feature cap, defaults to `max_features/0`.
    """
    @spec map(GeoDataFrame.t() | GeoSeries.t(), keyword()) :: Kino.Render.t()
    def map(frame, opts \\ []) do
      geometry = geometry(frame)
      limit = Keyword.get(opts, :max_features, max_features())

      cond do
        not maplibre?() ->
          Kino.Markdown.new(
            "The map needs the optional `:kino_maplibre` and `:maplibre` packages. " <>
              "Add `{:kino_maplibre, \"~> 0.1\"}` to your dependencies."
          )

        GeoSeries.size(geometry) > limit ->
          Kino.Markdown.new(
            "Map not drawn: #{GeoSeries.size(geometry)} features is too many for a GeoJSON " <>
              "map (limit #{limit}). Filter or sample the frame, or raise `:max_features`."
          )

        GeoSeries.total_bounds(geometry) == nil ->
          Kino.Markdown.new("Nothing to draw: the geometry column has no non-empty geometries.")

        true ->
          frame |> maplibre() |> Kino.MapLibre.new()
      end
    end

    @doc false
    @spec render(GeoDataFrame.t() | GeoSeries.t()) :: Kino.Layout.t()
    def render(%GeoDataFrame{} = gdf) do
      Kino.Layout.tabs(
        Map: map(gdf),
        Table: Kino.DataTable.new(gdf),
        Inspect: Kino.Inspect.new(gdf)
      )
    end

    def render(%GeoSeries{} = series) do
      Kino.Layout.tabs(Map: map(series), Inspect: Kino.Inspect.new(series))
    end

    defp geometry(%GeoDataFrame{geometry: geometry}), do: geometry
    defp geometry(%GeoSeries{} = series), do: series

    defp maplibre? do
      Code.ensure_loaded?(MapLibre) and Code.ensure_loaded?(Kino.MapLibre)
    end

    if Code.ensure_loaded?(MapLibre) and Code.ensure_loaded?(Kino.MapLibre) do
      @doc """
      Builds the `MapLibre` style for a frame or series, without wrapping it in a kino.

      Use it to customise the map (extra layers, another basemap) before calling
      `Kino.MapLibre.new/1`. The GeoJSON source is named `"geoframe"` and the layers are
      `"geoframe-fill"`, `"geoframe-outline"`, `"geoframe-line"` and `"geoframe-circle"`.
      Features carry geometry only, no properties, and are drawn in 2D: Z and M are dropped. The size limit is not applied here.
      """
      @spec maplibre(GeoDataFrame.t() | GeoSeries.t()) :: MapLibre.t()
      def maplibre(frame) do
        geometry = geometry(frame)
        {center, zoom} = view(GeoSeries.total_bounds(geometry))

        MapLibre.new(center: center, zoom: zoom)
        |> MapLibre.add_source("geoframe", type: :geojson, data: feature_collection(geometry))
        |> MapLibre.add_layer(
          id: "geoframe-fill",
          type: :fill,
          source: "geoframe",
          filter: ["==", "$type", "Polygon"],
          paint: [fill_color: "#3b82f6", fill_opacity: 0.35]
        )
        |> MapLibre.add_layer(
          id: "geoframe-outline",
          type: :line,
          source: "geoframe",
          filter: ["==", "$type", "Polygon"],
          paint: [line_color: "#1d4ed8", line_width: 1.5]
        )
        |> MapLibre.add_layer(
          id: "geoframe-line",
          type: :line,
          source: "geoframe",
          filter: ["==", "$type", "LineString"],
          paint: [line_color: "#dc2626", line_width: 2.5]
        )
        |> MapLibre.add_layer(
          id: "geoframe-circle",
          type: :circle,
          source: "geoframe",
          filter: ["==", "$type", "Point"],
          paint: [
            circle_color: "#f59e0b",
            circle_radius: 6,
            circle_stroke_color: "#ffffff",
            circle_stroke_width: 1.5
          ]
        )
      end

      defp feature_collection(geometry) do
        features =
          for geom <- geometry |> GeoSeries.force_2d() |> GeoSeries.to_geojson(), geom != nil do
            %{"type" => "Feature", "geometry" => geom, "properties" => %{}}
          end

        %{"type" => "FeatureCollection", "features" => features}
      end
    end

    # Centre and zoom that fit `bounds` into the assumed viewport (Web Mercator, 512px tiles).
    defp view(nil), do: {{0.0, 0.0}, 1}

    defp view({minx, miny, maxx, maxy}) do
      center = {(minx + maxx) / 2, (miny + maxy) / 2}
      dx = maxx - minx
      dy = mercator_y(maxy) - mercator_y(miny)

      if dx == 0 and dy == 0 do
        {center, 14}
      else
        {width, height} = @viewport
        zoom_x = if dx > 0, do: :math.log2(width / 512 * 360 / dx), else: 22.0
        zoom_y = if dy > 0, do: :math.log2(height / 512 * 2 * :math.pi() / dy), else: 22.0
        zoom = (min(zoom_x, zoom_y) - 0.3) |> max(0.0) |> min(18.0) |> Float.round(2)
        {center, zoom}
      end
    end

    defp mercator_y(lat) do
      lat = lat |> max(-@max_lat) |> min(@max_lat)
      :math.log(:math.tan(:math.pi() / 4 + lat * :math.pi() / 360))
    end
  end

  defimpl Kino.Render, for: GeoFrame.GeoDataFrame do
    def to_livebook(gdf), do: gdf |> GeoFrame.Kino.render() |> Kino.Render.to_livebook()
  end

  defimpl Kino.Render, for: GeoFrame.GeoSeries do
    def to_livebook(series), do: series |> GeoFrame.Kino.render() |> Kino.Render.to_livebook()
  end
end
