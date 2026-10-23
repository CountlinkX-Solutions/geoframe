if Code.ensure_loaded?(Kino.SmartCell) do
  defmodule GeoFrame.Kino.ReadFileCell do
    @moduledoc """
    A Livebook Smart Cell that reads a spatial file into a `GeoFrame.GeoDataFrame`.

    Add it from the Livebook cell menu (**Smart** > **Read spatial file**). It is registered
    when the `:geoframe` application starts and Kino is available.

    Fields:

      * **Assign to** - the variable that receives the frame (default `gdf`).
      * **Path** - the file to read; relative paths resolve from the notebook's working
        directory.
      * **Driver** - `auto` picks the format from the extension; otherwise one of `parquet`,
        `geojson`, `flatgeobuf`, `feather`, `shapefile`, `gpkg`. See `GeoFrame.read_file/2`.
      * **Layer** - for a GeoPackage, chosen from `GeoFrame.list_layers/2` once the file exists.
      * **Bounding box** - optional `minx, miny, maxx, maxy`, passed as `bbox:` (GeoParquet and
        FlatGeobuf only; other formats raise on it).
      * **Reproject to** - optional target CRS, such as `EPSG:3857` or `3857`.
      * **Show map** - ends the cell with `GeoFrame.Kino.deck/2`.

    The cell generates code such as:

        gdf = GeoFrame.read_file!("data/places.gpkg", layer: "places")
        gdf = GeoFrame.GeoDataFrame.to_crs(gdf, 3857)
        GeoFrame.Kino.deck(gdf)
    """

    use Kino.JS, assets_path: "lib/assets/read_file"
    use Kino.JS.Live
    use Kino.SmartCell, name: "Read spatial file"

    import GeoFrame.Kino.CellSupport

    @drivers ~w(parquet geojson flatgeobuf feather shapefile gpkg)
    @extensions %{
      ".parquet" => "parquet",
      ".geoparquet" => "parquet",
      ".geojson" => "geojson",
      ".json" => "geojson",
      ".fgb" => "flatgeobuf",
      ".feather" => "feather",
      ".arrow" => "feather",
      ".shp" => "shapefile",
      ".zip" => "shapefile",
      ".gpkg" => "gpkg"
    }
    @fields ~w(variable path driver layer bbox to_crs show_map)

    @impl true
    def init(attrs, ctx) do
      fields = %{
        "variable" => Kino.SmartCell.prefixed_var_name("gdf", attrs["variable"]),
        "path" => attrs["path"] || "",
        "driver" => attrs["driver"] || "auto",
        "layer" => attrs["layer"] || "",
        "bbox" => attrs["bbox"] || "",
        "to_crs" => attrs["to_crs"] || "",
        "show_map" => attrs["show_map"] || false
      }

      layers = layers(fields)
      {:ok, assign(ctx, fields: select_layer(fields, layers), layers: layers)}
    end

    @impl true
    def handle_connect(ctx) do
      {:ok, %{fields: ctx.assigns.fields, layers: ctx.assigns.layers, drivers: @drivers}, ctx}
    end

    @impl true
    def handle_event("update_field", %{"field" => field, "value" => value}, ctx)
        when field in @fields do
      fields = Map.put(ctx.assigns.fields, field, value)

      if field in ["path", "driver"] do
        layers = layers(fields)
        fields = select_layer(fields, layers)
        payload = %{"fields" => Map.take(fields, [field, "layer"]), "layers" => layers}
        broadcast_event(ctx, "update_field", payload)
        {:noreply, assign(ctx, fields: fields, layers: layers)}
      else
        broadcast_event(ctx, "update_field", %{"fields" => %{field => value}})
        {:noreply, assign(ctx, fields: fields)}
      end
    end

    @impl true
    def to_attrs(ctx), do: ctx.assigns.fields

    @impl true
    def to_source(attrs) do
      path = text(attrs["path"])
      variable = attrs["variable"]

      if path == "" or not variable?(variable) do
        ""
      else
        attrs |> statements(path, var(variable)) |> block()
      end
    end

    defp statements(attrs, path, var) do
      read = read_statement(attrs, path, var)

      reproject =
        case text(attrs["to_crs"]) do
          "" ->
            []

          crs ->
            target = if crs =~ ~r/^\d+$/, do: String.to_integer(crs), else: crs

            [
              quote do
                unquote(var) = GeoFrame.GeoDataFrame.to_crs(unquote(var), unquote(target))
              end
            ]
        end

      map =
        if attrs["show_map"] == true,
          do: [quote(do: GeoFrame.Kino.deck(unquote(var)))],
          else: []

      [read] ++ reproject ++ map
    end

    defp read_statement(attrs, path, var) do
      case read_options(attrs, path) do
        [] ->
          quote do
            unquote(var) = GeoFrame.read_file!(unquote(path))
          end

        options ->
          quote do
            unquote(var) = GeoFrame.read_file!(unquote(path), unquote(Macro.escape(options)))
          end
      end
    end

    defp read_options(attrs, path) do
      explicit = explicit_driver(attrs["driver"])
      layer = text(attrs["layer"])

      driver_opt = if explicit, do: [driver: String.to_atom(explicit)], else: []

      layer_opt =
        if effective_driver(path, explicit) == "gpkg" and layer != "",
          do: [layer: layer],
          else: []

      bbox_opt = if bbox = bbox(attrs["bbox"]), do: [bbox: bbox], else: []

      driver_opt ++ layer_opt ++ bbox_opt
    end

    defp explicit_driver(driver) when driver in @drivers, do: driver
    defp explicit_driver(_other), do: nil

    defp effective_driver(path, explicit) do
      explicit || Map.get(@extensions, path |> Path.extname() |> String.downcase())
    end

    # Keeps the chosen layer if it exists, else the first one (or none).
    defp select_layer(fields, layers) do
      if fields["layer"] in layers,
        do: fields,
        else: Map.put(fields, "layer", List.first(layers, ""))
    end

    # The layer names of an existing GeoPackage, else an empty list.
    defp layers(fields) do
      path = text(fields["path"])

      if effective_driver(path, explicit_driver(fields["driver"])) == "gpkg" and
           File.regular?(path) do
        path
        |> GeoFrame.list_layers(driver: :gpkg)
        |> Explorer.DataFrame.to_rows()
        |> Enum.reject(&is_nil(&1["geometry_type"]))
        |> Enum.map(& &1["name"])
      else
        []
      end
    rescue
      _error -> []
    end
  end
end
