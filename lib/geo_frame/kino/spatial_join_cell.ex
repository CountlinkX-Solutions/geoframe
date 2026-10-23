if Code.ensure_loaded?(Kino.SmartCell) do
  defmodule GeoFrame.Kino.SpatialJoinCell do
    @moduledoc """
    A Livebook Smart Cell that joins two `GeoFrame.GeoDataFrame` variables spatially.

    Add it from the Livebook cell menu (**Smart** > **Spatial join**). The left and right
    selectors list the `GeoFrame.GeoDataFrame` variables of the notebook, refreshed whenever
    a cell is evaluated. It is registered when the `:geoframe` application starts and Kino is
    available.

    Fields:

      * **Left** and **Right** - the frames to join.
      * **Mode** - `predicate` calls `GeoFrame.sjoin/3`; `nearest` calls
        `GeoFrame.sjoin_nearest/3`.
      * **Predicate** - any of `GeoFrame.GeoSeries.sindex_valid_query_predicates/0`, read as
        `left.predicate(right)`. `dwithin` also needs a **Distance**.
      * **How** - `inner`, `left` or `right`.
      * **Max distance** and **Distance column** - the `sjoin_nearest/3` options of the same
        name, both optional.
      * **Assign to** - the variable that receives the result (default `joined`).
      * **Show map** - ends the cell with `GeoFrame.Kino.deck/2`.

    The cell generates code such as:

        joined = GeoFrame.sjoin(points, zones, how: :inner, predicate: :within)
        GeoFrame.Kino.deck(joined)
    """

    use Kino.JS, assets_path: "lib/assets/spatial_join"
    use Kino.JS.Live
    use Kino.SmartCell, name: "Spatial join"

    import GeoFrame.Kino.CellSupport

    alias GeoFrame.GeoDataFrame
    alias GeoFrame.GeoSeries

    @hows ~w(inner left right)
    @fields ~w(left right result mode predicate how distance max_distance distance_col show_map)
    @defaults %{
      "left" => "",
      "right" => "",
      "result" => "joined",
      "mode" => "predicate",
      "predicate" => "intersects",
      "how" => "inner",
      "distance" => "",
      "max_distance" => "",
      "distance_col" => "",
      "show_map" => false
    }

    @impl true
    def init(attrs, ctx) do
      fields =
        @defaults
        |> Map.merge(Map.take(attrs, @fields))
        |> Map.update!("mode", &if(&1 == "nearest", do: "nearest", else: "predicate"))
        |> Map.put("result", Kino.SmartCell.prefixed_var_name("joined", attrs["result"]))

      {:ok, assign(ctx, fields: fields, variables: [])}
    end

    @impl true
    def handle_connect(ctx) do
      {:ok,
       %{
         fields: ctx.assigns.fields,
         variables: ctx.assigns.variables,
         predicates: predicates()
       }, ctx}
    end

    @impl true
    def handle_event("update_field", %{"field" => field, "value" => value}, ctx)
        when field in @fields do
      broadcast_event(ctx, "update_field", %{"fields" => %{field => value}})
      {:noreply, update(ctx, :fields, &Map.put(&1, field, value))}
    end

    @impl true
    def handle_info({:variables, variables}, ctx) do
      broadcast_event(ctx, "variables", %{variables: variables})
      ctx = assign(ctx, variables: variables)

      case default_frames(ctx.assigns.fields, variables) do
        defaults when defaults == %{} ->
          {:noreply, ctx}

        defaults ->
          broadcast_event(ctx, "update_field", %{"fields" => defaults})
          {:noreply, update(ctx, :fields, &Map.merge(&1, defaults))}
      end
    end

    @impl true
    def scan_binding(pid, binding, _env) do
      variables =
        for {name, %GeoDataFrame{}} <- binding, do: Atom.to_string(name)

      send(pid, {:variables, Enum.sort(variables)})
    end

    @impl true
    def to_attrs(ctx), do: ctx.assigns.fields

    @impl true
    def to_source(attrs) do
      [left, right, result] = Enum.map(~w(left right result), &attrs[&1])

      if Enum.all?([left, right, result], &variable?/1) do
        call = join_call(attrs, var(left), var(right))
        assign = quote(do: unquote(var(result)) = unquote(call))

        map =
          if attrs["show_map"] == true,
            do: [quote(do: GeoFrame.Kino.deck(unquote(var(result))))],
            else: []

        block([assign | map])
      else
        ""
      end
    end

    defp join_call(%{"mode" => "nearest"} = attrs, left, right) do
      options =
        [how: how(attrs)] ++
          optional(:max_distance, number(attrs["max_distance"])) ++
          optional(:distance_col, blank_to_nil(text(attrs["distance_col"])))

      quote do
        GeoFrame.sjoin_nearest(unquote(left), unquote(right), unquote(options))
      end
    end

    defp join_call(attrs, left, right) do
      predicate = predicate(attrs["predicate"])

      options =
        [how: how(attrs), predicate: predicate] ++
          if predicate == :dwithin, do: optional(:distance, number(attrs["distance"])), else: []

      quote do
        GeoFrame.sjoin(unquote(left), unquote(right), unquote(options))
      end
    end

    defp optional(_key, nil), do: []
    defp optional(key, value), do: [{key, value}]

    defp blank_to_nil(""), do: nil
    defp blank_to_nil(value), do: value

    defp how(attrs) do
      if attrs["how"] in @hows, do: String.to_existing_atom(attrs["how"]), else: :inner
    end

    defp predicate(name) do
      Enum.find(
        GeoSeries.sindex_valid_query_predicates(),
        :intersects,
        &(Atom.to_string(&1) == name)
      )
    end

    defp predicates, do: Enum.map(GeoSeries.sindex_valid_query_predicates(), &Atom.to_string/1)

    # Picks the first two frames for empty selections; keeps a selection that still exists.
    defp default_frames(fields, variables) do
      [first | rest] = variables ++ [nil, nil]

      %{}
      |> put_default(fields, "left", first, variables)
      |> put_default(fields, "right", List.first(rest), variables)
    end

    defp put_default(defaults, fields, key, candidate, variables) do
      if fields[key] in ["", nil] and candidate in variables,
        do: Map.put(defaults, key, candidate),
        else: defaults
    end
  end
end
