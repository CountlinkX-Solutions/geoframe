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
    are filled with an outline, lines are stroked, points are circles. A GeometryCollection is
    drawn by decomposing it into its members (nested collections included), one feature per
    member; every feature carries the `"row"` of its source row in `properties`. The view is fitted to
    the data's `GeoFrame.GeoSeries.total_bounds/1`. The basemap is MapLibre's demo style, which
    needs internet access in the browser. Coordinates are drawn as stored and must be
    longitude/latitude; Z and M are ignored (the Inspect and Table tabs still show them).

    ### Size limit

    GeoJSON is JSON text held in the notebook, so the cost of a map is its vertices, not its
    features. Maps are capped at #{50_000} features **and** #{500_000} vertices by default;
    larger inputs show a notice instead of a map. Change the caps per call with the
    `:max_features` and `:max_vertices` options of `map/2`, or globally with
    `config :geoframe, max_map_features: n, max_map_vertices: n`. For larger inputs use
    `deck/2`, which sends binary buffers and renders them with deck.gl.

    ### Table limit

    The Table tab converts every geometry to WKT and hands every row to the browser, so it is
    capped as well: by default the leading rows that fit #{100_000} rows and #{250_000}
    vertices (all geometry columns together) are shown, above a notice that says how many were
    left out. Use `table/2` with `:max_rows` and `:max_vertices`, or
    `config :geoframe, max_table_rows: n, max_table_vertices: n`.
    """

    @default_max_features 50_000
    @default_max_vertices 500_000
    @default_max_table_rows 100_000
    @default_max_table_vertices 250_000

    alias GeoFrame.GeoDataFrame
    alias GeoFrame.GeoSeries

    @doc """
    The default cap on features drawn on a map.
    """
    @spec max_features() :: pos_integer()
    def max_features, do: Application.get_env(:geoframe, :max_map_features, @default_max_features)

    @doc """
    The default cap on vertices drawn on a map.
    """
    @spec max_vertices() :: pos_integer()
    def max_vertices,
      do: Application.get_env(:geoframe, :max_map_vertices, @default_max_vertices)

    @doc """
    Builds the map kino for a `GeoFrame.GeoDataFrame` or `GeoFrame.GeoSeries`.

    Returns a MapLibre kino, or a Markdown notice when the input is too large, has nothing to
    draw, or `:kino_maplibre` is not installed.

    Options: `:max_features` - feature cap, defaults to `max_features/0`; `:max_vertices` -
    vertex cap, defaults to `max_vertices/0`. GeometryCollections are drawn member by member.
    """
    @spec map(GeoDataFrame.t() | GeoSeries.t(), keyword()) :: Kino.Render.t()
    def map(frame, opts \\ []) do
      geometry = geometry(frame)
      limit = Keyword.get(opts, :max_features, max_features())
      vertex_limit = Keyword.get(opts, :max_vertices, max_vertices())

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

        vertex_count(geometry) > vertex_limit ->
          Kino.Markdown.new(
            "Map not drawn: #{vertex_count(geometry)} vertices is too many for a GeoJSON " <>
              "map (limit #{vertex_limit}). Simplify, filter or sample the frame, raise " <>
              "`:max_vertices`, or use `GeoFrame.Kino.deck/2`."
          )

        GeoSeries.total_bounds(geometry) == nil ->
          Kino.Markdown.new("Nothing to draw: the geometry column has no non-empty geometries.")

        true ->
          maplibre_kino(frame)
      end
    end

    @doc """
    Builds a deck.gl map kino that draws geometry from binary buffers instead of GeoJSON, so it
    scales to a million features.

    See `GeoFrame.Kino.Deck` for the options (`:color`, `:radius`, `:width`, `:opacity`,
    `:tooltip`, `:max_features`, `:max_vertices`, `:basemap`, `:height`) and how the data is transferred. Unlike
    `map/2` it needs no extra packages, only internet access in the browser.
    """
    @spec deck(GeoDataFrame.t() | GeoSeries.t(), keyword()) :: Kino.JS.Live.t()
    defdelegate deck(frame, opts \\ []), to: GeoFrame.Kino.Deck, as: :new

    @doc """
    Builds the table kino for a frame, capped by rows and vertices.

    A frame within both caps gives the plain `Kino.DataTable`. A larger one gives the leading
    rows that fit, below a notice saying how many were left out; the frame itself is untouched.

    Options: `:max_rows` (default `config :geoframe, max_table_rows:`, else #{100_000}) and
    `:max_vertices` (default `config :geoframe, max_table_vertices:`, else #{250_000}), the
    vertices of all geometry columns together.
    """
    @spec table(GeoDataFrame.t(), keyword()) :: Kino.Render.t()
    def table(%GeoDataFrame{} = gdf, opts \\ []) do
      max_rows =
        Keyword.get_lazy(opts, :max_rows, fn ->
          Application.get_env(:geoframe, :max_table_rows, @default_max_table_rows)
        end)

      max_vertices =
        Keyword.get_lazy(opts, :max_vertices, fn ->
          Application.get_env(:geoframe, :max_table_vertices, @default_max_table_vertices)
        end)

      total = GeoDataFrame.n_rows(gdf)
      weights = row_vertices(gdf)
      vertices = Enum.sum(weights)
      kept = rows_within(weights, max_rows, max_vertices)

      if kept == total do
        Kino.DataTable.new(gdf)
      else
        notice =
          "Table shows the first #{kept} of #{total} rows (#{vertices} vertices in all; " <>
            "limits: #{max_rows} rows, #{max_vertices} vertices). Filter or sample the " <>
            "frame, or raise `:max_rows` / `:max_vertices` in `GeoFrame.Kino.table/2`."

        Kino.Layout.grid([
          Kino.Markdown.new(notice),
          Kino.DataTable.new(GeoDataFrame.head(gdf, kept))
        ])
      end
    end

    @doc false
    @spec vertex_count(GeoSeries.t()) :: non_neg_integer()
    def vertex_count(%GeoSeries{} = series) do
      series |> GeoSeries.count_coordinates() |> Explorer.Series.sum() |> Kernel.||(0)
    end

    # Vertices per row over the active and the secondary geometry columns.
    defp row_vertices(%GeoDataFrame{geometry: geometry, secondary_geometries: secondary}) do
      [geometry | Enum.map(secondary, &elem(&1, 1))]
      |> Enum.map(fn series ->
        series |> GeoSeries.count_coordinates() |> Explorer.Series.to_list()
      end)
      |> Enum.zip_with(fn counts -> counts |> Enum.map(&(&1 || 0)) |> Enum.sum() end)
    end

    # How many leading rows fit both caps.
    defp rows_within(weights, max_rows, max_vertices) do
      weights
      |> Enum.take(max_rows)
      |> Enum.reduce_while({0, 0}, fn weight, {rows, vertices} ->
        if vertices + weight > max_vertices,
          do: {:halt, {rows, vertices}},
          else: {:cont, {rows + 1, vertices + weight}}
      end)
      |> elem(0)
    end

    @doc false
    @spec render(GeoDataFrame.t() | GeoSeries.t()) :: Kino.Layout.t()
    def render(%GeoDataFrame{} = gdf) do
      Kino.Layout.tabs(
        Map: map(gdf),
        Table: table(gdf),
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
      @viewport {640, 420}
      @max_lat 85.0

      defp maplibre_kino(frame), do: frame |> maplibre() |> Kino.MapLibre.new()

      @doc """
      Builds the `MapLibre` style for a frame or series, without wrapping it in a kino.

      Use it to customise the map (extra layers, another basemap) before calling
      `Kino.MapLibre.new/1`. The GeoJSON source is named `"geoframe"` and the layers are
      `"geoframe-fill"`, `"geoframe-outline"`, `"geoframe-line"` and `"geoframe-circle"`.
      Features carry their source `"row"` as the only property and are drawn in 2D: Z and M are
      dropped. A GeometryCollection becomes one feature per member (nested collections
      flattened, empty members omitted). The size limits are not applied here.
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
          geometry
          |> GeoSeries.force_2d()
          |> GeoSeries.to_geojson()
          |> Enum.with_index()
          |> Enum.flat_map(fn
            {nil, _row} -> []
            {geom, row} -> for part <- parts(geom), do: feature(part, row)
          end)

        %{"type" => "FeatureCollection", "features" => features}
      end

      defp feature(geometry, row),
        do: %{"type" => "Feature", "geometry" => geometry, "properties" => %{"row" => row}}

      # MapLibre styles by geometry kind, so a collection is drawn through its members.
      defp parts(%{"type" => "GeometryCollection", "geometries" => members}),
        do: Enum.flat_map(members, &parts/1)

      defp parts(%{"coordinates" => []}), do: []
      defp parts(geometry), do: [geometry]

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
    else
      # Unreachable: map/2 checks for the optional packages first.
      defp maplibre_kino(_frame), do: raise("the map needs the optional :kino_maplibre package")
    end
  end

  defimpl Kino.Render, for: GeoFrame.GeoDataFrame do
    def to_livebook(gdf), do: gdf |> GeoFrame.Kino.render() |> Kino.Render.to_livebook()
  end

  defimpl Kino.Render, for: GeoFrame.GeoSeries do
    def to_livebook(series), do: series |> GeoFrame.Kino.render() |> Kino.Render.to_livebook()
  end
end
