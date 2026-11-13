if Code.ensure_loaded?(Kino.JS.Live) do
  defmodule GeoFrame.Kino.Deck do
    @moduledoc """
    A deck.gl map for Livebook that draws geometry from binary buffers, not GeoJSON.

    Build one with `GeoFrame.Kino.deck/2`. This module is compiled only when `:kino` is
    available.

    ## How data reaches the browser

    The geometry is reprojected to EPSG:4326 (a series without a CRS is assumed to be
    longitude/latitude already), then Rust flattens it into typed-array buffers: interleaved
    `Float64` coordinates, `Int32` part offsets and row indices, and, for polygons, `Uint32`
    triangle indices computed natively with ear clipping (holes supported). All buffers travel in
    **one** `Kino.JS.Live` binary payload, each 8-byte aligned so JavaScript maps them with zero
    parsing; deck.gl reads them as binary attributes, so no per-feature JavaScript object is ever
    created. Z and M are dropped.

    * **Precision:** coordinates stay `Float64`. Deck.gl splits them into high and low 32-bit
      halves on the GPU, so positions remain exact at street scale and no origin offset is
      needed. The price is 16 bytes per vertex on the wire (1 million points = 16 MB).
    * **Triangulation:** done in Rust at build time, so deck.gl runs `SolidPolygonLayer` with
      `_normalize: false` and skips its own tessellation. Polygons that cross the antimeridian
      are not cut.
    * **Per-feature styles:** a column color or size is expanded in Rust into one value per point,
      or one per *vertex* for lines and polygons (the layout deck.gl expects for binary path and
      polygon attributes). A fixed color or size costs nothing: it travels as a single value.
    * **Rows:** null and empty rows draw nothing. A multi geometry becomes one part per member.
      Every layer carries the source row of each part, which maps hover tooltips and per-row
      styles back to the table.
    * **GeometryCollections:** decomposed into their members (nested collections included),
      each drawn in the layer of its kind and carrying the collection's row, so a hover on any
      member shows the collection's tooltip.

    ## Options

      * `:color` - `{r, g, b}` or `{r, g, b, a}` (0-255), or `{column, :viridis}` (numeric ramp
        between the column's min and max) or `{column, :categorical}` (one palette color per
        distinct value). Missing values are gray. Default: blue.
      * `:radius` - point radius in pixels (number), or a numeric column scaled to 2-20 px so
        that the circle *area* follows the value (square-root scale). Default `4`.
      * `:width` - line and polygon outline width in pixels, or a numeric column scaled to 1-8 px
        (lines only; outlines use the number). Default `2`.
      * `:opacity` - layer opacity, 0 to 1. Default `0.8`.
      * `:tooltip` - list of attribute columns shown on hover (needs a `GeoFrame.GeoDataFrame`).
        Values are sent as JSON, strings cut at #{200} characters, and at most
        #{2_000_000} cells (rows x columns) are allowed.
      * `:popup` - `true` or a list of attribute columns shown in a closable card anchored at
        the clicked feature, like `explore(popup=True)` of GeoPandas (needs a
        `GeoFrame.GeoDataFrame`). `true` lists every attribute column (at most #{20}); a list takes
        at most #{20} columns. It shares the tooltip's cell and string caps and coexists with it:
        the tooltip appears on hover, the popup on click. Values are inserted as plain text, never
        as HTML. Default: off.
      * `:max_features` - maximum number of rows, default #{1_000_000}. A larger input raises
        `ArgumentError`; filter or sample it, or raise the option.
      * `:max_vertices` - maximum number of vertices (coordinates of all non-empty geometries),
        default #{10_000_000} (about 160 MB of positions). The wire size follows vertices, not
        rows, so a few huge polygons cost more than many points. A larger input raises
        `ArgumentError` naming the count; simplify, filter or sample it, or raise the option.
      * `:basemap` - a MapLibre style URL, or `nil` for a blank background. The default is
        Carto Positron (`#{"https://basemaps.cartocdn.com/gl/positron-gl-style/style.json"}`),
        which needs internet access in the browser.
      * `:height` - widget height in pixels, default `480`.
      * `:legend` - `true` (default) or `false`. With a column color (`{column, :viridis}` or
        `{column, :categorical}`) a card in the bottom-right corner explains the colors, like
        GeoPandas `explore(legend=True)`: a gradient bar of the same ramp with the column name
        and three ticks (min, middle, max, three significant digits) for a numeric column;
        swatches in the exact colors and order the widget assigns for a categorical one, capped
        at #{10} entries followed by "N more". A gray entry marks missing values when there are
        any. A fixed color draws no legend. The legend data is computed in Elixir next to the
        colors and travels in the payload; JavaScript only draws it.
      * `:scale` - `true` (default) or `false`. Draws metric and imperial scale bars in the
        bottom-left corner (MapLibre `ScaleControl`), like the `control_scale=True` of GeoPandas
        `explore`.

      * `:name` - the layer's name in the layer control (several layers only), default
        `"Layer N"`.

    ## Several layers

    Pass a list instead of one frame to draw several frames on one map, like
    `m = gdf1.explore(); gdf2.explore(m=m)` plus `folium.LayerControl()` in GeoPandas. Each entry
    is a frame, or `{frame, options}` with its own `:color`, `:radius`, `:width`, `:opacity`,
    `:tooltip`, `:popup`, `:legend` and `:name`:

        GeoFrame.Kino.deck(
          [
            {countries, name: "Countries", color: {"POP_EST", :viridis}, popup: ["NAME"]},
            {cities, name: "Cities", color: {200, 30, 30}, radius: 4, tooltip: ["name"]}
          ],
          height: 600
        )

    Layer options given in the second argument are defaults for every layer; an entry's own
    options win. The options of the whole map (`:basemap`, `:height`, `:scale`, `:max_features`,
    `:max_vertices`) are top-level only. A top-left panel has a checkbox per layer; hiding a layer
    also hides its legend and closes its popup, and each layer keeps its own legend card, tooltip
    and popup. The map fits the union of the layers' bounds. A list of one frame still draws the
    layer control; a bare frame draws none and sends the same payload as before.

    `:max_features` and `:max_vertices` are totals over all layers of the widget, so ten layers
    of 100 000 rows count as 1 000 000 features. The tooltip and popup cell caps apply to each
    layer. All layers travel in the one binary payload, each buffer named `"<layer>.<name>"`.

    deck.gl and MapLibre GL load in the browser from pinned jsDelivr URLs, so the widget needs
    internet access there.
    """

    use Kino.JS, assets_path: "lib/assets/deck"
    use Kino.JS.Live

    alias GeoFrame.GeoDataFrame
    alias GeoFrame.GeoSeries
    alias GeoFrame.Native
    alias Kino.JS.Live, as: LiveKino

    @typedoc "A frame drawn by the widget."
    @type frame :: GeoDataFrame.t() | GeoSeries.t()
    @typedoc "One layer: a frame, or a frame with its own options."
    @type layer :: frame() | {frame(), keyword()}

    @default_max_features 1_000_000
    @default_max_vertices 10_000_000
    @max_tooltip_cells 2_000_000
    @max_tooltip_chars 200
    @max_tooltip_columns 10
    @max_popup_columns 20
    @default_basemap "https://basemaps.cartocdn.com/gl/positron-gl-style/style.json"
    @default_color [35, 120, 200, 255]
    @missing_color [179, 179, 179, 255]
    @radius_range {2.0, 20.0}
    @width_range {1.0, 8.0}
    @options [
      :color,
      :radius,
      :width,
      :opacity,
      :tooltip,
      :popup,
      :max_features,
      :max_vertices,
      :basemap,
      :height,
      :legend,
      :scale,
      :name
    ]
    # Options of the whole map; a layer in `{frame, options}` cannot set them.
    @map_options [:max_features, :max_vertices, :basemap, :height, :scale]
    @legend_max_items 10
    # Three ticks of at most three significant digits fit the 180px bar without overlapping.
    @legend_ticks 3

    # viridis sampled at 0, 1/8, ..., 1.
    @viridis [
      {68, 1, 84},
      {71, 45, 123},
      {59, 82, 139},
      {44, 114, 142},
      {33, 144, 140},
      {39, 173, 129},
      {93, 200, 99},
      {170, 220, 50},
      {253, 231, 37}
    ]
    # Tableau 10.
    @palette [
      {78, 121, 167},
      {242, 142, 43},
      {225, 87, 89},
      {118, 183, 178},
      {89, 161, 79},
      {237, 201, 72},
      {176, 122, 161},
      {255, 157, 167},
      {156, 117, 95},
      {186, 176, 172}
    ]

    @doc """
    Builds the widget kino. See the module docs for the options and for drawing several frames
    as layers.

    Raises `ArgumentError` for invalid options, unknown columns, an input above `:max_features`
    or one with nothing to draw.
    """
    @spec new(frame() | [layer()], keyword()) :: Kino.JS.Live.t()
    def new(frame, opts \\ []), do: LiveKino.new(__MODULE__, payload(frame, opts))

    @impl true
    def init(payload, ctx), do: {:ok, assign(ctx, payload: payload)}

    @impl true
    def handle_connect(ctx), do: {:ok, ctx.assigns.payload, ctx}

    @doc false
    @spec payload(frame() | [layer()], keyword()) :: {:binary, map(), binary()}
    def payload(frames, opts) when is_list(frames), do: layers_payload(frames, opts)

    def payload(frame, opts) do
      opts = validate!(opts)
      geometry = geometry(frame)
      check_size!([geometry], opts)
      geometry = to_lon_lat(geometry)
      {layer, buffers, skipped, bounds} = build_layer(frame, geometry, opts, nil)
      {entries, binary} = pack(buffers)

      info =
        %{
          buffers: entries,
          layers: layer.layers,
          skipped: skipped,
          bounds: bounds,
          tooltip: layer.tooltip,
          legend: layer.legend,
          config: Map.merge(layer.config, map_config(opts))
        }
        |> put_popup(layer)

      {:binary, info, binary}
    end

    # Several layers: one binary holding every layer's buffers, named "<index>.<buffer>", and one
    # group per layer carrying its own counts, styles, legend, tooltip and popup.
    defp layers_payload([], _opts),
      do: raise(ArgumentError, "deck needs at least one layer; got an empty list")

    defp layers_payload(entries, opts) do
      map_opts = validate!(opts)
      defaults = Keyword.delete(opts, :name)

      frames =
        entries
        |> Enum.with_index()
        |> Enum.map(fn {entry, i} -> layer_entry!(entry, i, defaults) end)

      check_size!(Enum.map(frames, fn {frame, _} -> geometry(frame) end), map_opts)

      built =
        frames
        |> Enum.with_index()
        |> Enum.map(fn {{frame, layer_opts}, i} ->
          layer_opts = Keyword.put_new(layer_opts, :name, "Layer #{i + 1}")
          geometry = frame |> geometry() |> to_lon_lat()

          {layer, buffers, skipped, bounds} =
            build_layer(frame, geometry, layer_opts, layer_opts[:name])

          prefix = "#{i}."
          named = for {name, bin} <- buffers, do: {prefix <> name, bin}
          {Map.merge(layer, %{name: layer_opts[:name], prefix: prefix}), named, skipped, bounds}
        end)

      {entries, binary} = built |> Enum.flat_map(&elem(&1, 1)) |> pack()
      bounds = built |> Enum.map(&elem(&1, 3)) |> union_bounds()

      info = %{
        buffers: entries,
        groups: Enum.map(built, fn {layer, _, _, _} -> group(layer) end),
        skipped: built |> Enum.map(&elem(&1, 2)) |> Enum.sum(),
        bounds: bounds,
        config: map_config(map_opts)
      }

      {:binary, info, binary}
    end

    defp group(layer) do
      %{
        name: layer.name,
        prefix: layer.prefix,
        layers: layer.layers,
        tooltip: layer.tooltip,
        legend: layer.legend,
        config: layer.config
      }
      |> put_popup(layer)
    end

    defp put_popup(info, %{popup: nil}), do: info
    defp put_popup(info, %{popup: popup}), do: Map.put(info, :popup, popup)

    defp layer_entry!({frame, layer_opts}, i, defaults) when is_list(layer_opts) do
      unless Keyword.keyword?(layer_opts) do
        raise ArgumentError, "layer #{i + 1}: options must be a keyword list"
      end

      case Keyword.keys(layer_opts) -- (@options -- @map_options) do
        [] ->
          :ok

        [key | _] when key in @map_options ->
          raise ArgumentError,
                "layer #{i + 1}: #{inspect(key)} sets the whole map; pass it as a top-level option"

        [_ | _] ->
          :ok
      end

      opts = validate!(Keyword.merge(defaults, layer_opts))
      layer_frame!(frame, i)
      {frame, opts}
    end

    defp layer_entry!(frame, i, defaults), do: layer_entry!({frame, []}, i, defaults)

    defp layer_frame!(%GeoDataFrame{}, _i), do: :ok
    defp layer_frame!(%GeoSeries{}, _i), do: :ok

    defp layer_frame!(other, i) do
      raise ArgumentError,
            "layer #{i + 1} must be a GeoDataFrame, a GeoSeries or {frame, options}; got: " <>
              inspect(other)
    end

    defp union_bounds(all) do
      [
        all |> Enum.map(&Enum.at(&1, 0)) |> Enum.min(),
        all |> Enum.map(&Enum.at(&1, 1)) |> Enum.min(),
        all |> Enum.map(&Enum.at(&1, 2)) |> Enum.max(),
        all |> Enum.map(&Enum.at(&1, 3)) |> Enum.max()
      ]
    end

    defp map_config(opts),
      do: %{basemap: opts[:basemap], height: opts[:height], scale: opts[:scale]}

    # One layer's buffers and metadata; `geometry` is already in EPSG:4326.
    defp build_layer(frame, geometry, opts, label) do
      bounds =
        case GeoSeries.total_bounds(geometry) do
          nil ->
            raise ArgumentError,
                  "#{if label, do: label <> " has", else: "there is"} nothing to draw: " <>
                    "the geometry has no non-empty geometries"

          {minx, miny, maxx, maxy} ->
            [minx, miny, maxx, maxy]
        end

      {color_config, colors, legend} = color_style(frame, opts[:color])

      {radius_config, radius_sizes} =
        size_style(frame, opts[:radius], 4, @radius_range, :radius, :area)

      {width_config, width_sizes} =
        size_style(frame, opts[:width], 2, @width_range, :width, :linear)

      {buffers, skipped} = build_buffers(geometry, colors, radius_sizes, width_sizes)
      rows = GeoSeries.size(geometry)

      layer = %{
        layers: %{
          points: count(buffers, "points.rows"),
          lines: count(buffers, "lines.rows"),
          polygons: count(buffers, "polygons.rows")
        },
        tooltip: tooltip(frame, opts[:tooltip], rows),
        popup: popup(frame, opts[:popup], rows),
        legend: if(opts[:legend], do: legend),
        config: %{
          color: color_config,
          radius: radius_config,
          width: width_config,
          opacity: opts[:opacity]
        }
      }

      {layer, buffers, skipped, bounds}
    end

    # -- options ---------------------------------------------------------------------------

    @defaults [
      opacity: 0.8,
      max_features: @default_max_features,
      max_vertices: @default_max_vertices,
      basemap: @default_basemap,
      height: 480,
      legend: true,
      scale: true
    ]

    @checks [
      opacity: ":opacity must be a number from 0 to 1",
      max_features: ":max_features must be a positive integer",
      max_vertices: ":max_vertices must be a positive integer",
      height: ":height must be a positive integer (pixels)",
      basemap: ":basemap must be a style URL or nil",
      legend: ":legend must be true or false",
      scale: ":scale must be true or false",
      name: ":name must be a string (the layer's name in the layer control)"
    ]

    defp valid?(:opacity, v), do: is_number(v) and v >= 0 and v <= 1
    defp valid?(key, v) when key in [:basemap, :name], do: is_nil(v) or is_binary(v)
    defp valid?(key, v) when key in [:legend, :scale], do: is_boolean(v)
    defp valid?(_positive_integer, v), do: is_integer(v) and v > 0

    defp validate!(opts) do
      unless Keyword.keyword?(opts), do: raise(ArgumentError, "options must be a keyword list")

      case Keyword.keys(opts) -- @options do
        [] ->
          :ok

        [key | _] ->
          raise ArgumentError, "unknown option #{inspect(key)}; expected #{inspect(@options)}"
      end

      opts = Keyword.merge(@defaults, opts)

      Enum.each(@checks, fn {key, message} -> check!(valid?(key, opts[key]), message) end)

      opts
    end

    defp check!(true, _message), do: :ok
    defp check!(false, message), do: raise(ArgumentError, message)

    defp geometry(%GeoDataFrame{geometry: geometry}), do: geometry
    defp geometry(%GeoSeries{} = series), do: series

    # The caps are totals over every layer of the widget.
    defp check_size!(geometries, opts) do
      size = geometries |> Enum.map(&GeoSeries.size/1) |> Enum.sum()

      if size > opts[:max_features] do
        raise ArgumentError,
              "#{size} features is above the deck.gl limit of #{opts[:max_features]}; " <>
                "filter or sample the frame, or raise :max_features"
      end

      vertices = geometries |> Enum.map(&GeoFrame.Kino.vertex_count/1) |> Enum.sum()

      if vertices > opts[:max_vertices] do
        raise ArgumentError,
              "#{vertices} vertices is above the deck.gl limit of #{opts[:max_vertices]}; " <>
                "simplify, filter or sample the frame, or raise :max_vertices"
      end
    end

    defp to_lon_lat(series) do
      if GeoSeries.crs(series) == nil, do: series, else: GeoSeries.to_crs(series, 4326)
    end

    # -- styling ---------------------------------------------------------------------------

    defp color_style(_frame, nil), do: {@default_color, <<>>, nil}

    defp color_style(_frame, color) when is_tuple(color) and tuple_size(color) in [3, 4] do
      {fixed_color!(Tuple.to_list(color)), <<>>, nil}
    end

    defp color_style(_frame, color) when is_list(color), do: {fixed_color!(color), <<>>, nil}

    defp color_style(frame, {column, ramp}) when ramp in [:viridis, :categorical] do
      values = column_values(frame, column)

      {colors, legend} =
        case ramp do
          :viridis -> {viridis_colors(values, column), gradient_legend(values, column)}
          :categorical -> {categorical_colors(values), categorical_legend(values, column)}
        end

      {nil, IO.iodata_to_binary(colors), legend}
    end

    defp color_style(_frame, other) do
      raise ArgumentError,
            "invalid :color #{inspect(other)}; expected {r, g, b}, {r, g, b, a}, " <>
              "{column, :viridis} or {column, :categorical}"
    end

    defp fixed_color!([r, g, b]), do: fixed_color!([r, g, b, 255])

    defp fixed_color!([r, g, b, a] = color) do
      if Enum.all?(color, &(is_integer(&1) and &1 in 0..255)),
        do: [r, g, b, a],
        else: raise(ArgumentError, "invalid :color; channels must be integers from 0 to 255")
    end

    defp fixed_color!(_), do: raise(ArgumentError, "invalid :color; expected 3 or 4 channels")

    defp viridis_colors(values, column) do
      numbers = numeric!(values, column)
      {min, max} = extent(numbers)

      for value <- numbers do
        case value do
          nil -> rgba(@missing_color)
          v -> v |> unit(min, max) |> viridis() |> rgba()
        end
      end
    end

    defp categorical_colors(values) do
      index =
        values
        |> Enum.reject(&is_nil/1)
        |> Enum.uniq()
        |> Enum.with_index()
        |> Map.new()

      for value <- values do
        case value do
          nil -> rgba(@missing_color)
          v -> @palette |> Enum.at(rem(Map.fetch!(index, v), length(@palette))) |> rgba()
        end
      end
    end

    # -- legend ----------------------------------------------------------------------------

    # Legend data comes from the same value lists and ramp functions that color the features.
    defp gradient_legend(values, column) do
      numbers = numeric!(values, column)

      if Enum.all?(numbers, &is_nil/1) do
        nil
      else
        {min, max} = numbers |> extent() |> then(fn {lo, hi} -> {lo / 1, hi / 1} end)

        %{
          kind: "gradient",
          title: to_string(column),
          min: min,
          max: max,
          stops: for(i <- 0..8, do: (i / 8) |> viridis() |> Enum.take(3)),
          ticks: ticks(min, max),
          missing: Enum.any?(numbers, &is_nil/1),
          missing_color: Enum.take(@missing_color, 3)
        }
      end
    end

    defp ticks(same, same), do: [%{position: 0.5, label: format_tick(same)}]

    defp ticks(min, max) do
      for i <- 0..(@legend_ticks - 1) do
        position = i / (@legend_ticks - 1)
        %{position: position, label: format_tick(min + position * (max - min))}
      end
    end

    defp categorical_legend(values, column) do
      categories = values |> Enum.reject(&is_nil/1) |> Enum.uniq()

      if categories == [] do
        nil
      else
        items =
          categories
          |> Enum.with_index()
          |> Enum.take(@legend_max_items)
          |> Enum.map(fn {value, i} ->
            {r, g, b} = Enum.at(@palette, rem(i, length(@palette)))
            %{label: category_label(value), color: [r, g, b]}
          end)

        %{
          kind: "categorical",
          title: to_string(column),
          items: items,
          more: max(length(categories) - @legend_max_items, 0),
          missing: Enum.any?(values, &is_nil/1),
          missing_color: Enum.take(@missing_color, 3)
        }
      end
    end

    defp category_label(value) when is_binary(value), do: printable(value)
    defp category_label(value), do: text(value)

    # 1500000 -> "1.5M", 614270 -> "614k", 2.5 -> "2.5", 10.0 -> "10", 0.00042 -> "0.00042".
    defp format_tick(value) do
      {divisor, suffix} =
        cond do
          abs(value) >= 1.0e9 -> {1.0e9, "B"}
          abs(value) >= 1.0e6 -> {1.0e6, "M"}
          abs(value) >= 1.0e4 -> {1.0e3, "k"}
          true -> {1.0, ""}
        end

      trim_number(value / divisor) <> suffix
    end

    defp trim_number(number) when number == 0, do: "0"

    defp trim_number(number) do
      # Three significant digits.
      digits = min(max(2 - floor(:math.log10(abs(number))), 0), 12)

      text = number |> Float.round(digits) |> :erlang.float_to_binary(decimals: digits)

      if digits > 0,
        do: text |> String.trim_trailing("0") |> String.trim_trailing("."),
        else: text
    end

    defp viridis(t) do
      scaled = t * (length(@viridis) - 1)
      i = min(trunc(scaled), length(@viridis) - 2)
      frac = scaled - i
      {r1, g1, b1} = Enum.at(@viridis, i)
      {r2, g2, b2} = Enum.at(@viridis, i + 1)
      [lerp(r1, r2, frac), lerp(g1, g2, frac), lerp(b1, b2, frac), 255]
    end

    defp lerp(a, b, t), do: round(a + (b - a) * t)

    defp rgba({r, g, b}), do: <<r, g, b, 255>>
    defp rgba([r, g, b, a]), do: <<r, g, b, a>>

    # {config value, per-row f32 binary}. A number is config; a column is a binary.
    defp size_style(_frame, nil, default, _range, _name, _scale), do: {default, <<>>}

    defp size_style(_frame, value, _default, _range, _name, _scale)
         when is_number(value) and value > 0,
         do: {value, <<>>}

    defp size_style(frame, column, _default, {lo, hi}, _name, scale)
         when is_atom(column) or is_binary(column) do
      numbers = frame |> column_values(column) |> numeric!(column)
      {min, max} = extent(numbers)

      sizes =
        for value <- numbers, into: <<>> do
          size = if value, do: lo + scaled(unit(value, min, max), scale) * (hi - lo), else: lo
          <<size::float-32-little>>
        end

      {nil, sizes}
    end

    defp size_style(_frame, other, _default, _range, name, _scale) do
      raise ArgumentError,
            "invalid #{inspect(name)} #{inspect(other)}; expected a positive number or a column"
    end

    # `:area` makes the symbol area, not its radius, proportional to the value, so a skewed
    # column (population) still shows its small values.
    defp scaled(t, :area), do: :math.sqrt(t)
    defp scaled(t, :linear), do: t

    defp unit(_value, same, same), do: 0.5
    defp unit(value, min, max), do: (value - min) / (max - min)

    defp extent(numbers) do
      case Enum.reject(numbers, &is_nil/1) do
        [] -> {0.0, 0.0}
        present -> Enum.min_max(present)
      end
    end

    # Floats and integers stay; NaN and infinities become nil.
    defp numeric!(values, column) do
      Enum.map(values, fn
        v when is_number(v) -> v
        v when v in [nil, :nan, :infinity, :neg_infinity] -> nil
        _ -> raise ArgumentError, "column #{inspect(column)} must be numeric"
      end)
    end

    defp column_values(%GeoDataFrame{} = frame, column) do
      name = to_string(column)

      unless name in GeoDataFrame.names(frame) do
        raise ArgumentError, "column #{inspect(column)} is missing from the frame"
      end

      frame |> GeoDataFrame.pull(name) |> Explorer.Series.to_list()
    end

    defp column_values(%GeoSeries{}, column) do
      raise ArgumentError,
            "column #{inspect(column)} needs a GeoDataFrame; a GeoSeries has no attributes"
    end

    # -- buffers ---------------------------------------------------------------------------

    # Colors apply to every layer; radius sizes to points, width sizes to lines.
    defp build_buffers(geometry, colors, <<>>, <<>>), do: native(geometry, colors, <<>>)

    defp build_buffers(geometry, colors, radius, <<>>) do
      {buffers, skipped} = native(geometry, colors, radius)
      {Enum.reject(buffers, fn {name, _} -> name == "lines.sizes" end), skipped}
    end

    defp build_buffers(geometry, colors, <<>>, width) do
      {buffers, skipped} = native(geometry, colors, width)
      {Enum.reject(buffers, fn {name, _} -> name == "points.sizes" end), skipped}
    end

    defp build_buffers(geometry, colors, radius, width) do
      {points, skipped} = native(geometry, colors, radius)
      {lines, _} = native(geometry, colors, width)

      merged =
        Enum.reject(points, fn {name, _} -> name == "lines.sizes" end) ++
          Enum.filter(lines, fn {name, _} -> name == "lines.sizes" end)

      {merged, skipped}
    end

    defp native(%GeoSeries{ref: ref}, colors, sizes),
      do: Native.deck_buffers_nif(ref, colors, sizes)

    defp count(buffers, name) do
      case List.keyfind(buffers, name, 0) do
        {_, bin} -> div(byte_size(bin), 4)
        nil -> 0
      end
    end

    # One binary, every buffer starting on an 8-byte boundary.
    defp pack(buffers) do
      {entries, iodata, _size} =
        Enum.reduce(buffers, {%{}, [], 0}, fn {name, bin}, {entries, iodata, offset} ->
          pad = rem(8 - rem(offset, 8), 8)
          start = offset + pad

          {Map.put(entries, name, %{offset: start, bytes: byte_size(bin)}),
           [iodata, <<0::size(pad)-unit(8)>>, bin], start + byte_size(bin)}
        end)

      {entries, IO.iodata_to_binary(iodata)}
    end

    # -- tooltip and popup -----------------------------------------------------------------

    defp tooltip(frame, columns, rows),
      do: attributes(frame, columns, rows, :tooltip, @max_tooltip_columns)

    # `true` shows every attribute column (the geometry is not one of them).
    defp popup(%GeoDataFrame{} = frame, true, rows),
      do:
        attributes(frame, Explorer.DataFrame.names(frame.data), rows, :popup, @max_popup_columns)

    defp popup(frame, true, rows),
      do: attributes(frame, [:popup], rows, :popup, @max_popup_columns)

    defp popup(_frame, false, _rows), do: nil

    defp popup(frame, columns, rows),
      do: attributes(frame, columns, rows, :popup, @max_popup_columns)

    defp attributes(_frame, nil, _rows, _option, _max), do: nil
    defp attributes(_frame, [], _rows, _option, _max), do: nil

    defp attributes(frame, columns, rows, option, max) when is_list(columns) do
      if length(columns) > max,
        do: raise(ArgumentError, ":#{option} takes at most #{max} columns")

      if rows * length(columns) > @max_tooltip_cells do
        raise ArgumentError,
              "#{option} data would be #{rows * length(columns)} cells (limit #{@max_tooltip_cells}); " <>
                "use fewer :#{option} columns or a smaller :max_features"
      end

      values =
        for column <- columns, do: frame |> column_values(column) |> Enum.map(&json_value/1)

      %{columns: Enum.map(columns, &to_string/1), values: values}
    end

    defp attributes(_frame, other, _rows, option, _max) do
      raise ArgumentError,
            "invalid :#{option} #{inspect(other)}; expected a list of columns" <>
              if(option == :popup, do: " or true", else: "")
    end

    defp json_value(v) when is_nil(v) or is_boolean(v) or is_integer(v), do: v
    defp json_value(v) when is_float(v), do: v
    defp json_value(v) when v in [:nan, :infinity, :neg_infinity], do: nil

    defp json_value(v) when is_binary(v),
      do: v |> printable() |> String.slice(0, @max_tooltip_chars)

    defp json_value(v), do: v |> text() |> String.slice(0, @max_tooltip_chars)

    defp text(value) do
      to_string(value)
    rescue
      Protocol.UndefinedError -> inspect(value)
    end

    defp printable(binary), do: if(String.valid?(binary), do: binary, else: inspect(binary))
  end
end
