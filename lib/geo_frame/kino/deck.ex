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
      * `:max_features` - maximum number of rows, default #{1_000_000}. A larger input raises
        `ArgumentError`; filter or sample it, or raise the option.
      * `:basemap` - a MapLibre style URL, or `nil` for a blank background. The default is
        Carto Positron (`#{"https://basemaps.cartocdn.com/gl/positron-gl-style/style.json"}`),
        which needs internet access in the browser.
      * `:height` - widget height in pixels, default `480`.

    deck.gl and MapLibre GL load in the browser from pinned jsDelivr URLs, so the widget needs
    internet access there.
    """

    use Kino.JS, assets_path: "lib/assets/deck"
    use Kino.JS.Live

    alias GeoFrame.GeoDataFrame
    alias GeoFrame.GeoSeries
    alias GeoFrame.Native
    alias Kino.JS.Live, as: LiveKino

    @default_max_features 1_000_000
    @max_tooltip_cells 2_000_000
    @max_tooltip_chars 200
    @max_tooltip_columns 10
    @default_basemap "https://basemaps.cartocdn.com/gl/positron-gl-style/style.json"
    @default_color [35, 120, 200, 255]
    @missing_color [179, 179, 179, 255]
    @radius_range {2.0, 20.0}
    @width_range {1.0, 8.0}
    @options [:color, :radius, :width, :opacity, :tooltip, :max_features, :basemap, :height]

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
    Builds the widget kino. See the module docs for the options.

    Raises `ArgumentError` for invalid options, unknown columns, an input above `:max_features`
    or one with nothing to draw.
    """
    @spec new(GeoDataFrame.t() | GeoSeries.t(), keyword()) :: Kino.JS.Live.t()
    def new(frame, opts \\ []), do: LiveKino.new(__MODULE__, payload(frame, opts))

    @impl true
    def init(payload, ctx), do: {:ok, assign(ctx, payload: payload)}

    @impl true
    def handle_connect(ctx), do: {:ok, ctx.assigns.payload, ctx}

    @doc false
    @spec payload(GeoDataFrame.t() | GeoSeries.t(), keyword()) ::
            {:binary, map(), binary()}
    def payload(frame, opts) do
      opts = validate!(opts)
      geometry = geometry(frame)
      check_size!(geometry, opts)
      geometry = to_lon_lat(geometry)

      bounds =
        case GeoSeries.total_bounds(geometry) do
          nil -> raise ArgumentError, "nothing to draw: the geometry has no non-empty geometries"
          {minx, miny, maxx, maxy} -> [minx, miny, maxx, maxy]
        end

      {color_config, colors} = color_style(frame, opts[:color])

      {radius_config, radius_sizes} =
        size_style(frame, opts[:radius], 4, @radius_range, :radius, :area)

      {width_config, width_sizes} =
        size_style(frame, opts[:width], 2, @width_range, :width, :linear)

      {buffers, skipped} = build_buffers(geometry, colors, radius_sizes, width_sizes)
      tooltip = tooltip(frame, opts[:tooltip], GeoSeries.size(geometry))
      {entries, binary} = pack(buffers)

      info = %{
        buffers: entries,
        layers: %{
          points: count(buffers, "points.rows"),
          lines: count(buffers, "lines.rows"),
          polygons: count(buffers, "polygons.rows")
        },
        skipped: skipped,
        bounds: bounds,
        tooltip: tooltip,
        config: %{
          color: color_config,
          radius: radius_config,
          width: width_config,
          opacity: opts[:opacity],
          basemap: opts[:basemap],
          height: opts[:height]
        }
      }

      {:binary, info, binary}
    end

    # -- options ---------------------------------------------------------------------------

    @defaults [
      opacity: 0.8,
      max_features: @default_max_features,
      basemap: @default_basemap,
      height: 480
    ]

    defp validate!(opts) do
      unless Keyword.keyword?(opts), do: raise(ArgumentError, "options must be a keyword list")

      case Keyword.keys(opts) -- @options do
        [] ->
          :ok

        [key | _] ->
          raise ArgumentError, "unknown option #{inspect(key)}; expected #{inspect(@options)}"
      end

      opts = Keyword.merge(@defaults, opts)

      check!(
        is_number(opts[:opacity]) and opts[:opacity] >= 0 and opts[:opacity] <= 1,
        ":opacity must be a number from 0 to 1"
      )

      check!(
        is_integer(opts[:max_features]) and opts[:max_features] > 0,
        ":max_features must be a positive integer"
      )

      check!(
        is_integer(opts[:height]) and opts[:height] > 0,
        ":height must be a positive integer (pixels)"
      )

      check!(
        is_nil(opts[:basemap]) or is_binary(opts[:basemap]),
        ":basemap must be a style URL or nil"
      )

      opts
    end

    defp check!(true, _message), do: :ok
    defp check!(false, message), do: raise(ArgumentError, message)

    defp geometry(%GeoDataFrame{geometry: geometry}), do: geometry
    defp geometry(%GeoSeries{} = series), do: series

    defp check_size!(geometry, opts) do
      size = GeoSeries.size(geometry)

      if size > opts[:max_features] do
        raise ArgumentError,
              "#{size} features is above the deck.gl limit of #{opts[:max_features]}; " <>
                "filter or sample the frame, or raise :max_features"
      end
    end

    defp to_lon_lat(series) do
      if GeoSeries.crs(series) == nil, do: series, else: GeoSeries.to_crs(series, 4326)
    end

    # -- styling ---------------------------------------------------------------------------

    defp color_style(_frame, nil), do: {@default_color, <<>>}

    defp color_style(_frame, color) when is_tuple(color) and tuple_size(color) in [3, 4] do
      {fixed_color!(Tuple.to_list(color)), <<>>}
    end

    defp color_style(_frame, color) when is_list(color), do: {fixed_color!(color), <<>>}

    defp color_style(frame, {column, ramp}) when ramp in [:viridis, :categorical] do
      values = column_values(frame, column)

      colors =
        case ramp do
          :viridis -> viridis_colors(values, column)
          :categorical -> categorical_colors(values)
        end

      {nil, IO.iodata_to_binary(colors)}
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

    # -- tooltip ---------------------------------------------------------------------------

    defp tooltip(_frame, nil, _rows), do: nil
    defp tooltip(_frame, [], _rows), do: nil

    defp tooltip(frame, columns, rows) when is_list(columns) do
      if length(columns) > @max_tooltip_columns,
        do: raise(ArgumentError, ":tooltip takes at most #{@max_tooltip_columns} columns")

      if rows * length(columns) > @max_tooltip_cells do
        raise ArgumentError,
              "tooltip data would be #{rows * length(columns)} cells (limit #{@max_tooltip_cells}); " <>
                "use fewer :tooltip columns or a smaller :max_features"
      end

      values =
        for column <- columns, do: frame |> column_values(column) |> Enum.map(&json_value/1)

      %{columns: Enum.map(columns, &to_string/1), values: values}
    end

    defp tooltip(_frame, other, _rows),
      do: raise(ArgumentError, "invalid :tooltip #{inspect(other)}; expected a list of columns")

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
