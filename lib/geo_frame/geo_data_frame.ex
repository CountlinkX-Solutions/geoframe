defmodule GeoFrame.GeoDataFrame do
  @moduledoc """
  A dataframe of attributes paired with a geometry column.

  A `%GeoDataFrame{}` holds an `Explorer.DataFrame` with the attribute columns only and a
  `GeoFrame.GeoSeries` with the geometries, which live in Rust memory. The geometry column
  is *not* part of the Explorer dataframe; it has a name (`:geometry_column`, default
  `"geometry"`) that is reserved among the attribute names.

  ## Several geometry columns

  Like a GeoPandas frame with more than one `geometry` dtype column, a frame has one *active*
  geometry (`geometry/1`, the one every spatial operation uses) and any number of *secondary*
  geometry columns, each a `GeoFrame.GeoSeries` with its own CRS. `put_geometry/3` adds one,
  `set_geometry/3` makes another column the active one, `pull/2` returns one by name and
  `geometry_columns/1` lists them (active first). Row operations (`take/2`, `filter_with/2`,
  `sort_with/2`, `join/3`, `concat_rows/1`, `explode/2`, ...) keep every geometry column
  aligned; `to_crs/2` and `set_crs/2` act on the active one, as in GeoPandas. GeoParquet,
  Feather and Arrow read and write all of them (`GeoFrame.read_parquet/2`,
  `GeoFrame.write_parquet/3`, `GeoFrame.write_feather/3`, `to_arrow/2`), and the formats with
  one geometry per feature (GeoPackage, GeoJSON, FlatGeobuf, Shapefile, PostGIS, `to_json/2`)
  raise `ArgumentError` for such a frame, as GeoPandas `to_file` raises `ValueError`. `clip/3`, `overlay/3`, `sjoin/3`,
  `sjoin_nearest/3` and `dissolve/2` carry them like GeoPandas (other geometry columns are
  ordinary columns there): kept with their rows and CRS, suffixed on a name clash, null for
  rows without a partner, aggregated by `dissolve/2` with `:first`, `:last` or `:count`.

  ## Row alignment

  Every operation that changes rows computes the row positions once and applies them to
  both sides (`GeoSeries.take/2` and `Explorer.DataFrame.slice/2`), so attribute and
  geometry rows can never drift apart. `filter_with/2` and `sort_with/2` add a temporary
  row-position column to the attributes, let Explorer do the work, read the resulting
  positions back, and remove the column again. The temporary column is invisible in the
  result, and its name is chosen so that it cannot collide with an existing column.

  ## Column order

  `names/1`, `to_explorer/2`, and the `Table.Reader` view list the attribute columns in
  their dataframe order and the geometry column last.

  ## Example

      iex> data = Explorer.DataFrame.new(id: [1, 2])
      iex> geometry = GeoFrame.GeoSeries.from_wkt!(["POINT (0 0)", "POINT (5 5)"])
      iex> gdf = GeoFrame.GeoDataFrame.new(data, geometry)
      iex> GeoFrame.GeoDataFrame.names(gdf)
      ["id", "geometry"]
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @enforce_keys [:data, :geometry]
  defstruct [:data, :geometry, geometry_column: "geometry", secondary_geometries: []]

  @type t :: %__MODULE__{
          data: DataFrame.t(),
          geometry: GeoSeries.t(),
          geometry_column: String.t(),
          secondary_geometries: [{String.t(), GeoSeries.t()}]
        }

  @temp_index "__geoframe_row__"
  @preview_rows 5

  # -- constructors ---------------------------------------------------------------------

  @doc """
  Pairs an attribute dataframe with a geometry series.

  Options: `:geometry_column` - the geometry column name (default `"geometry"`).

  Raises `ArgumentError` when row counts differ or `data` already has a column with the
  geometry column's name. A `data` without columns is accepted for any row count.
  """
  @spec new(DataFrame.t(), GeoSeries.t(), keyword()) :: t()
  def new(%DataFrame{} = data, %GeoSeries{} = geometry, opts \\ []) do
    GeoFrame.Options.validate!(opts, [:geometry_column])
    column = opts |> Keyword.get(:geometry_column, "geometry") |> to_string()
    rows = GeoSeries.size(geometry)

    if has_columns?(data) and DataFrame.n_rows(data) != rows do
      raise ArgumentError,
            "data has #{DataFrame.n_rows(data)} rows but geometry has #{rows} rows"
    end

    if column in DataFrame.names(data) do
      raise ArgumentError, "data already has a column named #{inspect(column)} (geometry column)"
    end

    %__MODULE__{data: data, geometry: geometry, geometry_column: column}
  end

  @doc """
  Moves a WKT string column of `df` into the geometry.

  The geometry column keeps the source column's name unless `:geometry_column` is given.
  Options: `:crs`, `:geometry_column`.
  """
  @spec from_wkt(DataFrame.t(), String.t(), keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_wkt(%DataFrame{} = df, column, opts \\ []), do: build(df, column, opts, :wkt)

  @doc "Like `from_wkt/3` but raises `ArgumentError` on failure."
  @spec from_wkt!(DataFrame.t(), String.t(), keyword()) :: t()
  def from_wkt!(df, column, opts \\ []), do: df |> from_wkt(column, opts) |> unwrap!()

  @doc "Moves a WKB binary column of `df` into the geometry. Same options as `from_wkt/3`."
  @spec from_wkb(DataFrame.t(), String.t(), keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_wkb(%DataFrame{} = df, column, opts \\ []), do: build(df, column, opts, :wkb)

  @doc "Like `from_wkb/3` but raises `ArgumentError` on failure."
  @spec from_wkb!(DataFrame.t(), String.t(), keyword()) :: t()
  def from_wkb!(df, column, opts \\ []), do: df |> from_wkb(column, opts) |> unwrap!()

  @doc """
  Builds a frame from GeoJSON-like features, like GeoPandas `GeoDataFrame.from_features`.

  `features` is a list of feature maps (`%{"type" => "Feature", "properties" => ..,
  "geometry" => ..}`) or a `FeatureCollection` map; string or atom keys both work. A bare
  geometry map counts as one feature. Property types are inferred over all features exactly
  as `GeoFrame.read_geojson/2` does (booleans, `s64`, `f64`, otherwise strings; nested values
  as JSON text; a feature `id` becomes a leading `"id"` column when every feature has one).
  A `nil` geometry is a null row. Property columns follow their first appearance across the
  features (as GeoPandas does) when the properties are *ordered*: a keyword list or a list of
  `{key, value}` pairs, or JSON decoded with `Jason.decode!(json, objects: :ordered_objects)`.
  Plain Elixir maps have no insertion order, so their columns appear sorted by name; use
  `:columns` to choose the order explicitly. (The geometry column comes last, as in every
  `GeoDataFrame` here, where GeoPandas puts it first.)

  Options:

    * `:crs` - the CRS of the geometries, `nil` by default (GeoPandas default; GeoJSON files
      read with `read_geojson/2` get `"OGC:CRS84"` instead).
    * `:columns` - property columns to keep, in that order; an unknown name is an error.
    * `:geometry_column` - name of the geometry column (default `"geometry"`).

  Returns `{:error, message}` for input that is not valid GeoJSON.
  """
  @spec from_features([map()] | map(), keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_features(features, opts \\ []) do
    opts = Keyword.validate!(opts, crs: nil, columns: nil, geometry_column: "geometry")
    text = features |> feature_collection() |> Jason.encode!()

    with {:ok, {ref, ipc}} <- GeoFrame.Native.read_geojson_text(text, opts[:crs]),
         {:ok, data} <- GeoFrame.Attributes.load(ipc),
         {:ok, data} <- select_columns(data, opts[:columns]) do
      geometry_column = to_string(opts[:geometry_column])
      geometry_column = unique_geometry_name(data, geometry_column)
      {:ok, new(data, %GeoSeries{ref: ref}, geometry_column: geometry_column)}
    end
  end

  @doc "Like `from_features/2` but raises `ArgumentError` on failure."
  @spec from_features!([map()] | map(), keyword()) :: t()
  def from_features!(features, opts \\ []), do: features |> from_features(opts) |> unwrap!()

  defp feature_collection(features) when is_list(features),
    do: %{"type" => "FeatureCollection", "features" => Enum.map(features, &ordered_feature/1)}

  defp feature_collection(%{} = map) when not is_struct(map) do
    Map.new(map, fn
      {key, features} when key in ["features", :features] and is_list(features) ->
        {key, Enum.map(features, &ordered_feature/1)}

      pair ->
        pair
    end)
  end

  defp feature_collection(%{} = map), do: map

  # Keyword lists and `{key, value}` pair lists under "properties" keep their order by
  # becoming ordered JSON objects; maps and ordered objects pass through unchanged.
  defp ordered_feature(%{} = feature) when not is_struct(feature) do
    Map.new(feature, fn
      {key, [_ | _] = pairs} when key in ["properties", :properties] ->
        {key, ordered_properties(pairs)}

      pair ->
        pair
    end)
  end

  defp ordered_feature(other), do: other

  defp ordered_properties(pairs) do
    if Enum.all?(pairs, &match?({key, _} when is_binary(key) or is_atom(key), &1)),
      do:
        Jason.OrderedObject.new(Enum.map(pairs, fn {key, value} -> {to_string(key), value} end)),
      else: pairs
  end

  defp select_columns(data, nil), do: {:ok, data}

  defp select_columns(data, columns) when is_list(columns) do
    columns = Enum.map(columns, &to_string/1)

    case columns -- DataFrame.names(data) do
      [] -> {:ok, DataFrame.select(data, columns)}
      missing -> {:error, "columns not found in the features: #{Enum.join(missing, ", ")}"}
    end
  end

  # A property may already use the requested geometry column name; GeoJSON reading appends
  # `_1`, `_2`... in that case, and so do we.
  defp unique_geometry_name(data, name) do
    taken = DataFrame.names(data)

    if name in taken do
      Stream.iterate(1, &(&1 + 1))
      |> Stream.map(&"#{name}_#{&1}")
      |> Enum.find(&(&1 not in taken))
    else
      name
    end
  end

  defp build(df, column, opts, encoding) do
    GeoFrame.Options.validate!(opts, [:crs, :geometry_column])
    column = to_string(column)

    if column in DataFrame.names(df) do
      values = df |> DataFrame.pull(column) |> Series.to_list()
      parse = if encoding == :wkt, do: &GeoSeries.from_wkt/2, else: &GeoSeries.from_wkb/2

      with {:ok, geometry} <- parse.(values, crs: Keyword.get(opts, :crs)) do
        data = DataFrame.discard(df, column)
        geometry_column = Keyword.get(opts, :geometry_column, column)
        {:ok, new(data, geometry, geometry_column: geometry_column)}
      end
    else
      {:error, "column #{inspect(column)} not found"}
    end
  end

  # -- PostGIS --------------------------------------------------------------------------

  @doc "GeoPandas `GeoDataFrame.from_postgis`: alias of `GeoFrame.read_postgis/3`."
  @spec from_postgis(term(), String.t(), keyword()) :: {:ok, t()} | {:error, String.t()}
  defdelegate from_postgis(conn, sql, opts \\ []), to: GeoFrame, as: :read_postgis

  @doc "GeoPandas `GeoDataFrame.to_postgis`: alias of `GeoFrame.to_postgis/4`."
  @spec to_postgis(t(), term(), String.t(), keyword()) :: :ok | {:error, String.t()}
  defdelegate to_postgis(gdf, conn, table, opts \\ []), to: GeoFrame

  # -- Arrow interchange ----------------------------------------------------------------

  @doc """
  Encodes the frame as an Arrow IPC **stream** (a binary): the attribute columns followed by
  the geometry columns (the active one first, then the secondary ones, each with its own CRS),
  whose fields carry GeoArrow extension metadata with the CRS as
  PROJJSON (and a GeoParquet-style `geo` entry in the schema metadata). GeoPandas reads it
  with `GeoDataFrame.from_arrow(pyarrow.ipc.open_stream(bytes).read_all())`.

  Option `:geometry_encoding` - `:wkb` (default) or `:geoarrow`; see
  `GeoFrame.GeoSeries.to_arrow/2`. Failures raise `ArgumentError`.
  """
  @spec to_arrow(t(), keyword()) :: binary()
  def to_arrow(%__MODULE__{} = gdf, opts \\ []), do: GeoFrame.Formats.to_arrow(gdf, opts)

  @doc """
  Decodes an Arrow IPC stream into a frame; see `GeoFrame.GeoSeries.from_arrow/2` for the
  accepted geometry encodings. All other columns become attributes.

  Options: `:columns` - attribute columns to keep; `:geometry` - the geometry column when it
  cannot be detected; `:crs` - a CRS used only when the data carries none.
  """
  @spec from_arrow(binary(), keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_arrow(bytes, opts \\ []) when is_binary(bytes),
    do: GeoFrame.Formats.from_arrow(bytes, opts)

  @doc "Like `from_arrow/2` but raises `ArgumentError` on failure."
  @spec from_arrow!(binary(), keyword()) :: t()
  def from_arrow!(bytes, opts \\ []), do: bytes |> from_arrow(opts) |> unwrap!()

  # -- accessors ------------------------------------------------------------------------

  @doc "The attribute dataframe (without geometry)."
  @spec data(t()) :: DataFrame.t()
  def data(%__MODULE__{data: data}), do: data

  @doc "The geometry series."
  @spec geometry(t()) :: GeoSeries.t()
  def geometry(%__MODULE__{geometry: geometry}), do: geometry

  @doc "Names of the geometry columns: the active one first, then the secondary ones."
  @spec geometry_columns(t()) :: [String.t()]
  def geometry_columns(%__MODULE__{geometry_column: column, secondary_geometries: secondary}),
    do: [column | Enum.map(secondary, &elem(&1, 0))]

  @doc """
  Adds the secondary geometry column `name`, or replaces the one with that name.

  Raises `ArgumentError` when `series` does not have one row per frame row or `name` is an
  attribute or the active geometry column. A secondary column keeps its own CRS.
  """
  @spec put_geometry(t(), String.t() | atom(), GeoSeries.t()) :: t()
  def put_geometry(%__MODULE__{} = gdf, name, %GeoSeries{} = series) do
    name = to_string(name)
    check_geometry_rows!(gdf, series)

    if name == gdf.geometry_column or name in DataFrame.names(gdf.data) do
      raise ArgumentError,
            "column #{inspect(name)} already exists and is not a secondary geometry"
    end

    secondary =
      if List.keymember?(gdf.secondary_geometries, name, 0),
        do: List.keyreplace(gdf.secondary_geometries, name, 0, {name, series}),
        else: gdf.secondary_geometries ++ [{name, series}]

    %{gdf | secondary_geometries: secondary}
  end

  @doc """
  Makes another geometry column the active one, like GeoPandas `set_geometry`.

  `column` is the name of a secondary geometry column, or a `GeoFrame.GeoSeries` that becomes
  a new column named by the `:geometry_column` option (default `"geometry"`, or the active
  name when that is free). The previous active column stays as a secondary one.

  Options: `:drop` - `true` removes the previous active geometry column (default `false`);
  `:geometry_column` - the name for a series. Raises `ArgumentError` for a name that is not a
  geometry column of the frame.
  """
  @spec set_geometry(t(), String.t() | atom() | GeoSeries.t(), keyword()) :: t()
  def set_geometry(gdf, column, opts \\ [])

  def set_geometry(%__MODULE__{} = gdf, %GeoSeries{} = series, opts) do
    opts = Keyword.validate!(opts, drop: false, geometry_column: nil)
    name = to_string(opts[:geometry_column] || gdf.geometry_column)
    check_geometry_rows!(gdf, series)

    if name in DataFrame.names(gdf.data) do
      raise ArgumentError, "column #{inspect(name)} is an attribute, not a geometry column"
    end

    secondary = List.keydelete(gdf.secondary_geometries, name, 0)
    keep_old? = not opts[:drop] and name != gdf.geometry_column
    old = {gdf.geometry_column, gdf.geometry}

    %{
      gdf
      | geometry: series,
        geometry_column: name,
        secondary_geometries: if(keep_old?, do: [old | secondary], else: secondary)
    }
  end

  def set_geometry(%__MODULE__{} = gdf, column, opts) do
    opts = Keyword.validate!(opts, drop: false)
    name = to_string(column)

    cond do
      name == gdf.geometry_column ->
        gdf

      List.keymember?(gdf.secondary_geometries, name, 0) ->
        activate(gdf, name, name, opts[:drop])

      name in DataFrame.names(gdf.data) ->
        raise ArgumentError, "column #{inspect(name)} is not a geometry column"

      true ->
        raise ArgumentError, "column #{inspect(name)} not found"
    end
  end

  defp check_geometry_rows!(gdf, series) do
    if GeoSeries.size(series) != n_rows(gdf) do
      raise ArgumentError,
            "geometry has #{GeoSeries.size(series)} rows but the frame has #{n_rows(gdf)} rows"
    end
  end

  # Moves the secondary column `from` into the active slot under `to`; the old active column
  # becomes the first secondary one unless `drop?`.
  defp activate(gdf, from, to, drop?) do
    {^from, series} = List.keyfind(gdf.secondary_geometries, from, 0)
    rest = List.keydelete(gdf.secondary_geometries, from, 0)
    kept = if drop?, do: rest, else: [{gdf.geometry_column, gdf.geometry} | rest]
    %{gdf | geometry: series, geometry_column: to, secondary_geometries: kept}
  end

  @doc "Number of rows."
  @spec n_rows(t()) :: non_neg_integer()
  def n_rows(%__MODULE__{geometry: geometry}), do: GeoSeries.size(geometry)

  @doc "Attribute column names followed by the geometry column names (active first)."
  @spec names(t()) :: [String.t()]
  def names(%__MODULE__{data: data} = gdf) do
    DataFrame.names(data) ++ geometry_columns(gdf)
  end

  @doc "Number of columns, counting the geometry columns."
  @spec n_columns(t()) :: non_neg_integer()
  def n_columns(%__MODULE__{data: data} = gdf),
    do: DataFrame.n_columns(data) + Kernel.length(geometry_columns(gdf))

  @doc "`{rows, columns}`, counting the geometry column."
  @spec shape(t()) :: {non_neg_integer(), non_neg_integer()}
  def shape(gdf), do: {n_rows(gdf), n_columns(gdf)}

  @doc "Column dtypes by name; the geometry column maps to `:geometry`."
  @spec dtypes(t()) :: %{String.t() => atom() | tuple()}
  def dtypes(%__MODULE__{data: data} = gdf) do
    Enum.reduce(geometry_columns(gdf), DataFrame.dtypes(data), &Map.put(&2, &1, :geometry))
  end

  @doc "The CRS of the geometry, or `nil`."
  @spec crs(t()) :: String.t() | nil
  def crs(%__MODULE__{geometry: geometry}), do: GeoSeries.crs(geometry)

  @doc """
  Replaces the CRS metadata without reprojecting; `crs` is a string, an integer EPSG code or
  `nil`. See `GeoFrame.GeoSeries.set_crs/2`.
  """
  @spec set_crs(t(), String.t() | pos_integer() | nil) :: t()
  def set_crs(%__MODULE__{geometry: geometry} = gdf, crs) do
    %{gdf | geometry: GeoSeries.set_crs(geometry, crs)}
  end

  @doc """
  Reprojects the geometry to `target` (`"EPSG:<code>"`, `"OGC:CRS84"` or an integer EPSG
  code); attributes are untouched. See `GeoFrame.GeoSeries.to_crs/2`.
  """
  @spec to_crs(t(), String.t() | pos_integer()) :: t()
  def to_crs(%__MODULE__{geometry: geometry} = gdf, target) do
    %{gdf | geometry: GeoSeries.to_crs(geometry, target)}
  end

  @doc "Whether the CRS is geographic. See `GeoFrame.GeoSeries.is_geographic/1`."
  @spec is_geographic(t()) :: boolean() | nil
  # GeoPandas-compatible name, see GeoSeries.is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_geographic(%__MODULE__{geometry: geometry}), do: GeoSeries.is_geographic(geometry)

  @doc "The UTM CRS for the geometry's extent. See `GeoFrame.GeoSeries.estimate_utm_crs/2`."
  @spec estimate_utm_crs(t(), keyword()) :: String.t()
  def estimate_utm_crs(%__MODULE__{geometry: geometry}, opts \\ []),
    do: GeoSeries.estimate_utm_crs(geometry, opts)

  # -- row-changing operations ----------------------------------------------------------

  @doc "Selects rows by zero-based position; repeats and reordering are allowed."
  @spec take(t(), [non_neg_integer()]) :: t()
  def take(%__MODULE__{data: data, geometry: geometry} = gdf, indices) when is_list(indices) do
    data = if has_columns?(data), do: DataFrame.slice(data, indices), else: data

    %{
      gdf
      | data: data,
        geometry: GeoSeries.take(geometry, indices),
        secondary_geometries: take_secondary(gdf, &GeoSeries.take(&1, indices))
    }
  end

  defp take_secondary(%__MODULE__{secondary_geometries: secondary}, fun),
    do: Enum.map(secondary, fn {name, series} -> {name, fun.(series)} end)

  @doc "Rows from `offset` for `length` rows, with `Explorer.DataFrame.slice/3` semantics."
  @spec slice(t(), integer(), non_neg_integer()) :: t()
  def slice(gdf, offset, length) when is_integer(offset) and is_integer(length) do
    by_positions(gdf, &DataFrame.slice(&1, offset, length))
  end

  @doc "The first `n` rows (default 5)."
  @spec head(t(), non_neg_integer()) :: t()
  def head(gdf, n \\ 5), do: by_positions(gdf, &DataFrame.head(&1, n))

  @doc "The last `n` rows (default 5)."
  @spec tail(t(), non_neg_integer()) :: t()
  def tail(gdf, n \\ 5), do: by_positions(gdf, &DataFrame.tail(&1, n))

  @doc """
  Keeps rows for which `fun` is true, like `Explorer.DataFrame.filter_with/2`.

  `fun` receives the attribute dataframe (as a lazy dataframe) and returns a boolean
  lazy series. The geometry is filtered by the same row positions.
  """
  @spec filter_with(t(), (DataFrame.t() -> Series.t())) :: t()
  def filter_with(%__MODULE__{} = gdf, fun) when is_function(fun, 1) do
    by_positions(gdf, &DataFrame.filter_with(&1, fun), gdf.data)
  end

  @doc """
  Sorts rows like `Explorer.DataFrame.sort_with/3`; `opts` are passed through.
  """
  @spec sort_with(t(), (DataFrame.t() -> term()), keyword()) :: t()
  def sort_with(%__MODULE__{} = gdf, fun, opts \\ []) when is_function(fun, 1) do
    by_positions(gdf, &DataFrame.sort_with(&1, fun, opts), gdf.data)
  end

  @doc """
  Keeps rows matching a query, like `Explorer.DataFrame.filter/2`.

  Expands to `filter_with/2` with `Explorer.Query.query/1`, so `require` this module first.
  The query sees the attribute columns only: the geometry is not a column in it. Use `^`
  to interpolate Elixir values.

      require GeoFrame.GeoDataFrame
      limit = 10
      GeoFrame.GeoDataFrame.filter(gdf, pop > ^limit)
  """
  defmacro filter(gdf, query) do
    quote do
      require Explorer.Query
      GeoFrame.GeoDataFrame.filter_with(unquote(gdf), Explorer.Query.query(unquote(query)))
    end
  end

  @doc """
  Sorts rows by a query, like `Explorer.DataFrame.sort_by/3`; `opts` are passed through.

  Expands to `sort_with/3` with `Explorer.Query.query/1`. The query sees the attribute
  columns only, and the geometry follows its row.
  """
  defmacro sort_by(gdf, query, opts \\ []) do
    quote do
      require Explorer.Query

      GeoFrame.GeoDataFrame.sort_with(
        unquote(gdf),
        Explorer.Query.query(unquote(query)),
        unquote(opts)
      )
    end
  end

  @doc """
  Keeps the first row of each distinct combination of attribute values, like
  `Explorer.DataFrame.distinct/2`; the geometry follows its row.

  `columns` (default all attribute columns) selects the columns compared; the geometry is
  never compared, and naming it is harmless.
  """
  @spec distinct(t(), term()) :: t()
  def distinct(gdf, columns \\ ..) do
    names = distinct_columns(gdf, columns)

    by_positions(gdf, &DataFrame.distinct(&1, names, keep_all: true), gdf.data)
  end

  defp distinct_columns(%__MODULE__{data: data}, ..), do: DataFrame.names(data)

  defp distinct_columns(%__MODULE__{data: data} = gdf, columns) do
    data |> DataFrame.select(attribute_columns(gdf, columns)) |> DataFrame.names()
  end

  @doc """
  Drops rows with a null in any of `columns` (default all attribute columns), like
  `Explorer.DataFrame.drop_nil/2`. A null geometry does not count; check it with
  `GeoFrame.GeoSeries.notna/1` with `take/2` if needed.
  """
  @spec drop_nil(t(), term()) :: t()
  def drop_nil(gdf, columns \\ ..) do
    by_positions(gdf, &DataFrame.drop_nil(&1, columns), gdf.data)
  end

  @doc """
  A random sample of rows, like `Explorer.DataFrame.sample/3`: `n_or_frac` is a row count
  or a fraction. Options (`:replace`, `:shuffle`, `:seed`) are passed through.
  """
  @spec sample(t(), number(), keyword()) :: t()
  def sample(gdf, n_or_frac, opts \\ []) do
    by_positions(gdf, &DataFrame.sample(&1, n_or_frac, opts))
  end

  @doc "Randomly reorders the rows; `opts` (`:seed`) are passed through."
  @spec shuffle(t(), keyword()) :: t()
  def shuffle(gdf, opts \\ []), do: by_positions(gdf, &DataFrame.shuffle(&1, opts))

  # Runs `op` on a dataframe carrying a temporary row-position column and applies the
  # resulting positions to both sides. `source` is the attribute data `op` should see;
  # for position-only operations it is a bare position frame.
  defp by_positions(%__MODULE__{data: data} = gdf, op, source \\ nil) do
    rows = n_rows(gdf)
    column = temp_column(DataFrame.names(data))
    positions = Series.from_list(Enum.to_list(0..(rows - 1)//1), dtype: {:s, 64})

    base =
      if source && has_columns?(source),
        do: DataFrame.put(source, column, positions),
        else: DataFrame.new([{column, positions}])

    indices = base |> op.() |> DataFrame.pull(column) |> Series.to_list()
    take(gdf, indices)
  end

  defp temp_column(names) do
    Enum.reduce_while(Stream.iterate(0, &(&1 + 1)), nil, fn n, _ ->
      name = if n == 0, do: @temp_index, else: "#{@temp_index}#{n}"
      if name in names, do: {:cont, nil}, else: {:halt, name}
    end)
  end

  @doc """
  Joins attributes of `right` onto the rows of `left`, like GeoPandas `GeoDataFrame.merge`.

  `right` is an `Explorer.DataFrame` or a `GeoFrame.GeoDataFrame`; for the latter only the
  attributes are used and its geometry is dropped (a frame has a single geometry column).
  The geometry always comes from `left`: it follows its rows, is repeated for duplicated keys
  and is `nil` for rows that exist only on the right (`how: :right` and `:outer`).

  Options go to `Explorer.DataFrame.join/3` (`:on`, `:how`, `:suffix`, ...); `:how` is one of
  `:inner` (default), `:left`, `:right`, `:outer` or `:cross`. Raises `ArgumentError` when
  `:on` names the geometry column or the result would contain a column with its name.

  Known deviations from pandas: `:outer` lists left-only rows last (pandas sorts them by
  key) and Polars does not coalesce the key, so `:outer` yields `"k"` (nil for right-only
  rows) plus `"k_right"`; `:inner`, `:left` and `:right` match pandas row for row. Each row
  keeps the geometry of its own left row either way.
  """
  @spec join(t(), DataFrame.t() | t(), keyword()) :: t()
  def join(left, right, opts \\ [])

  def join(%__MODULE__{} = left, %__MODULE__{data: right}, opts), do: join(left, right, opts)

  def join(
        %__MODULE__{data: data, geometry: geometry} = left,
        %DataFrame{} = right,
        opts
      ) do
    reject_geometry_key!(opts[:on], geometry_columns(left))
    column = temp_column(DataFrame.names(data) ++ DataFrame.names(right))
    rows = n_rows(left)
    positions = Series.from_list(Enum.to_list(0..(rows - 1)//1), dtype: {:s, 64})

    tagged =
      if has_columns?(data),
        do: DataFrame.put(data, column, positions),
        else: DataFrame.new([{column, positions}])

    joined = DataFrame.join(tagged, right, opts)
    indices = joined |> DataFrame.pull(column) |> Series.to_list()
    attributes = DataFrame.discard(joined, column)

    check_free!(attributes, left)

    %{
      left
      | data: attributes,
        geometry: GeoSeries.take_nullable(geometry, indices),
        secondary_geometries: take_secondary(left, &GeoSeries.take_nullable(&1, indices))
    }
  end

  defp reject_geometry_key!(nil, _geo), do: :ok

  defp reject_geometry_key!(on, geo) do
    names =
      on
      |> List.wrap()
      |> Enum.map(fn
        {l, r} -> [l, r]
        n -> n
      end)
      |> List.flatten()

    if bad = Enum.find(names, &(to_string(&1) in geo)) do
      raise ArgumentError, "cannot join on the geometry column #{inspect(to_string(bad))}"
    end
  end

  @doc """
  Rows whose geometry intersects the box, like GeoPandas `gdf.cx[xmin:xmax, ymin:ymax]`.
  `nil` bounds are unbounded; see `GeoFrame.GeoSeries.cx/5`. Attributes stay aligned.
  """
  @spec cx(t(), number() | nil, number() | nil, number() | nil, number() | nil) :: t()
  def cx(%__MODULE__{geometry: geometry} = gdf, xmin, ymin, xmax, ymax) do
    take(gdf, GeoSeries.cx_positions(geometry, xmin, ymin, xmax, ymax))
  end

  @doc """
  Stacks frames row-wise, like `pandas.concat`/`GeoDataFrame` concatenation.

  Attribute columns follow `Explorer.DataFrame.concat_rows/1` (same names and compatible
  dtypes, else it raises). Every frame must use the same geometry column name and CRS, or
  `ArgumentError` is raised. The CRS check runs before any data is copied.
  """
  @spec concat_rows([t()]) :: t()
  def concat_rows([%__MODULE__{geometry_column: column} | _] = frames) do
    unless Enum.all?(frames, &match?(%__MODULE__{geometry_column: ^column}, &1)) do
      raise ArgumentError,
            "all frames must use the same geometry column name (#{inspect(column)})"
    end

    geometry = frames |> Enum.map(& &1.geometry) |> GeoSeries.concat()
    secondary = concat_secondary(frames)

    data =
      case Enum.filter(frames, &has_columns?(&1.data)) do
        [] ->
          DataFrame.new([])

        with_columns when Kernel.length(with_columns) == Kernel.length(frames) ->
          DataFrame.concat_rows(Enum.map(frames, & &1.data))

        _ ->
          raise ArgumentError, "cannot concatenate frames with and without attribute columns"
      end

    %{new(data, geometry, geometry_column: column) | secondary_geometries: secondary}
  end

  def concat_rows(other),
    do:
      raise(
        ArgumentError,
        "concat_rows expects a non-empty list of GeoDataFrames, got: #{inspect(other)}"
      )

  defp concat_secondary([first | _] = frames) do
    names = Enum.map(first.secondary_geometries, &elem(&1, 0))

    unless Enum.all?(frames, &(Enum.map(&1.secondary_geometries, fn {n, _} -> n end) == names)) do
      raise ArgumentError,
            "all frames must have the same geometry columns, got: " <>
              inspect(Enum.map(frames, &geometry_columns/1))
    end

    for name <- names do
      series = frames |> Enum.map(&(&1.secondary_geometries |> List.keyfind(name, 0) |> elem(1)))
      {name, GeoSeries.concat(series)}
    end
  end

  @doc """
  Splits multi-part geometries and collections into one row per part, like
  `GeoDataFrame.explode`. The attributes of a row are repeated for each of its parts.

  `GeoSeries.explode/2` explains which geometries have parts: rows whose geometry is null or an
  empty multi-geometry disappear.

  pandas keeps the source row in the index (a MultiIndex of source row and part number when
  `index_parts=True`). A frame has no index, so the same information becomes plain columns,
  placed first:

    * `index` - the zero-based position of the source row (`s64`); omitted with
      `ignore_index: true`;
    * `part` - the zero-based number of the part within its source row; only with
      `index_parts: true`, whatever `ignore_index` says.

  Options: `:index_parts` (default `false`), `:ignore_index` (default `false`). Raises
  `ArgumentError` if an attribute is already called `index` or `part` and the column would be
  added.
  """
  @spec explode(t(), keyword()) :: t()
  def explode(%__MODULE__{data: data, geometry: geometry} = gdf, opts \\ []) do
    opts = Keyword.validate!(opts, index_parts: false, ignore_index: false)
    {parts, positions, numbers} = GeoSeries.explode(geometry, index_parts: true)

    added =
      if(opts[:ignore_index], do: [], else: [{"index", positions}]) ++
        if(opts[:index_parts], do: [{"part", numbers}], else: [])

    taken = if has_columns?(data), do: DataFrame.slice(data, positions), else: data
    clashes = Enum.filter(added, fn {name, _} -> name in DataFrame.names(taken) end)

    if clashes != [] do
      raise ArgumentError,
            "explode would add a column named #{inspect(elem(hd(clashes), 0))}, which already " <>
              "exists (rename it first)"
    end

    columns =
      Enum.map(added, fn {name, values} -> {name, Series.from_list(values, dtype: {:s, 64})} end) ++
        Enum.map(DataFrame.names(taken), &{&1, DataFrame.pull(taken, &1)})

    %{
      gdf
      | data: DataFrame.new(columns),
        geometry: parts,
        secondary_geometries: take_secondary(gdf, &GeoSeries.take(&1, positions))
    }
  end

  @doc "Clips to a mask; see `GeoFrame.Overlay.clip/3`."
  @spec clip(t(), term(), keyword()) :: t()
  defdelegate clip(gdf, mask, opts \\ []), to: GeoFrame.Overlay

  @doc "Spatial overlay with another frame; see `GeoFrame.Overlay.overlay/3`."
  @spec overlay(t(), t(), keyword()) :: t()
  defdelegate overlay(df1, df2, opts \\ []), to: GeoFrame.Overlay

  @doc "Spatial join; see `GeoFrame.SpatialJoin.sjoin/3`."
  @spec sjoin(t(), t(), keyword()) :: t()
  defdelegate sjoin(left, right, opts \\ []), to: GeoFrame.SpatialJoin

  @doc "Nearest spatial join; see `GeoFrame.SpatialJoin.sjoin_nearest/3`."
  @spec sjoin_nearest(t(), t(), keyword()) :: t()
  defdelegate sjoin_nearest(left, right, opts \\ []), to: GeoFrame.SpatialJoin

  @doc "Group-wise union with attribute aggregation; see `GeoFrame.Dissolve.dissolve/2`."
  @spec dissolve(t(), keyword()) :: t()
  defdelegate dissolve(gdf, opts \\ []), to: GeoFrame.Dissolve

  # -- attribute operations -------------------------------------------------------------

  @doc """
  Adds or replaces attribute columns from a query, like `Explorer.DataFrame.mutate/2`.

  Expands to `mutate_with/2` with `Explorer.Query.query/1`. The query sees the attribute
  columns only. Raises `ArgumentError` if the result would contain the geometry column's name.
  """
  defmacro mutate(gdf, mutations) do
    quote do
      require Explorer.Query
      GeoFrame.GeoDataFrame.mutate_with(unquote(gdf), Explorer.Query.query(unquote(mutations)))
    end
  end

  @doc """
  Adds or replaces attribute columns via `Explorer.DataFrame.mutate_with/2`.

  Raises `ArgumentError` if the result would contain the geometry column's name.
  """
  @spec mutate_with(t(), (DataFrame.t() -> keyword() | map())) :: t()
  def mutate_with(%__MODULE__{data: data} = gdf, fun) when is_function(fun, 1) do
    check_free!(DataFrame.mutate_with(data, fun), gdf)
  end

  @doc """
  Selects attribute columns (anything `Explorer.DataFrame.select/2` accepts). The active
  geometry is always kept, and naming it in `columns` is harmless. A secondary geometry
  column is kept only when `columns` names it.
  """
  @spec select(t(), term()) :: t()
  def select(%__MODULE__{data: data} = gdf, columns) do
    named = named_columns(columns)

    %{
      gdf
      | data: DataFrame.select(data, attribute_columns(gdf, columns)),
        secondary_geometries:
          Enum.filter(gdf.secondary_geometries, fn {name, _} -> name in named end)
    }
  end

  @doc """
  Drops attribute columns (anything `Explorer.DataFrame.discard/2` accepts), and the
  secondary geometry columns it names. The active geometry is never discarded, and naming it
  in `columns` is harmless, like in `select/2`.
  """
  @spec discard(t(), term()) :: t()
  def discard(%__MODULE__{data: data} = gdf, columns) do
    named = named_columns(columns)

    %{
      gdf
      | data: DataFrame.discard(data, attribute_columns(gdf, columns)),
        secondary_geometries:
          Enum.reject(gdf.secondary_geometries, fn {name, _} -> name in named end)
    }
  end

  # The names in a plain name or list of names; ranges, regexes and the like name nothing.
  defp named_columns(column) when is_binary(column) or is_atom(column),
    do: [to_string(column)]

  defp named_columns(columns) when is_list(columns) do
    if Enum.all?(columns, &(is_binary(&1) or is_atom(&1))),
      do: Enum.map(columns, &to_string/1),
      else: []
  end

  defp named_columns(_other), do: []

  # What goes to Explorer: geometry names are not attribute columns.
  defp attribute_columns(gdf, columns) do
    without_geometry(columns, geometry_columns(gdf))
  end

  # A plain name or list of names loses the geometry names (geometry is not an attribute
  # column); ranges, regexes and the like pass through.
  defp without_geometry(column, geo) when is_binary(column) or is_atom(column),
    do: without_geometry([column], geo)

  defp without_geometry(columns, geo) do
    if is_list(columns) and Enum.all?(columns, &(is_binary(&1) or is_atom(&1))),
      do: Enum.reject(columns, &(to_string(&1) in List.wrap(geo))),
      else: columns
  end

  @doc """
  Adds or replaces the attribute column `name` with a series, list, or tensor, like
  `Explorer.DataFrame.put/4`. Raises `ArgumentError` for the geometry column's name or a
  value whose length differs from the row count.
  """
  @spec put(t(), String.t() | atom(), Series.t() | list() | Nx.Tensor.t()) :: t()
  def put(%__MODULE__{data: data} = gdf, name, values) do
    if to_string(name) in geometry_columns(gdf) do
      raise ArgumentError, "column #{inspect(to_string(name))} is reserved for the geometry"
    end

    if has_columns?(data) do
      %{gdf | data: DataFrame.put(data, name, values)}
    else
      series = if is_list(values), do: Series.from_list(values), else: values

      if Series.size(series) != n_rows(gdf) do
        raise ArgumentError,
              "series has #{Series.size(series)} rows but the frame has #{n_rows(gdf)} rows"
      end

      %{gdf | data: DataFrame.new([{to_string(name), series}])}
    end
  end

  @doc """
  An attribute column as a series. The name of a geometry column (active or secondary)
  returns its `GeoFrame.GeoSeries` instead.
  """
  @spec pull(t(), String.t() | atom() | integer()) :: Series.t() | GeoSeries.t()
  def pull(%__MODULE__{geometry: geometry, geometry_column: geo} = gdf, column) do
    name = if is_binary(column) or is_atom(column), do: to_string(column)

    case name && List.keyfind(gdf.secondary_geometries, name, 0) do
      {^name, series} -> series
      _ when name == geo -> geometry
      _ -> DataFrame.pull(gdf.data, column)
    end
  end

  @doc """
  Renames attribute columns via `Explorer.DataFrame.rename/2`. Raises `ArgumentError` if a
  new name equals the geometry column's name.
  """
  @spec rename(t(), term()) :: t()
  def rename(%__MODULE__{data: data} = gdf, renames) do
    {geometry_renames, attribute_renames} = split_renames(gdf, renames)

    gdf
    |> rename_geometries(geometry_renames)
    |> then(fn renamed -> check_free!(DataFrame.rename(data, attribute_renames), renamed) end)
  end

  # Renames of secondary geometry columns, given as a map or a list of pairs, are applied to
  # the geometry; everything else goes to Explorer.
  defp split_renames(gdf, renames) when is_map(renames) or is_list(renames) do
    pairs = Enum.to_list(renames)

    if Enum.all?(pairs, &match?({_, _}, &1)) do
      {geometry, attributes} =
        Enum.split_with(pairs, fn {old, _new} ->
          List.keymember?(gdf.secondary_geometries, to_string(old), 0)
        end)

      {Map.new(geometry, fn {old, new} -> {to_string(old), to_string(new)} end),
       if(is_map(renames), do: Map.new(attributes), else: attributes)}
    else
      {%{}, renames}
    end
  end

  defp split_renames(_gdf, renames), do: {%{}, renames}

  defp rename_geometries(gdf, renames) when map_size(renames) == 0, do: gdf

  defp rename_geometries(gdf, renames) do
    secondary =
      Enum.map(gdf.secondary_geometries, fn {name, s} -> {Map.get(renames, name, name), s} end)

    %{gdf | secondary_geometries: secondary}
  end

  defp check_free!(data, %__MODULE__{} = gdf) do
    names = geometry_columns(gdf)

    if Enum.uniq(names) != names do
      raise ArgumentError, "two geometry columns have the same name: #{inspect(names)}"
    end

    case Enum.find(names, &(&1 in DataFrame.names(data))) do
      nil -> %{gdf | data: data}
      name -> raise ArgumentError, "column #{inspect(name)} is reserved for the geometry"
    end
  end

  # -- spatial measures -----------------------------------------------------------------

  @doc "Planar area per row as a series."
  @spec area(t()) :: Series.t()
  def area(%__MODULE__{geometry: geometry}), do: GeoSeries.area(geometry)

  @doc "Planar length (perimeter for polygons) per row as a series."
  @spec length(t()) :: Series.t()
  def length(%__MODULE__{geometry: geometry}), do: GeoSeries.length(geometry)

  @doc "Per-row bounds as a dataframe (`minx`, `miny`, `maxx`, `maxy`)."
  @spec bounds(t()) :: DataFrame.t()
  def bounds(%__MODULE__{geometry: geometry}), do: GeoSeries.bounds(geometry)

  @doc "Returns the frame with its geometry replaced by the centroids."
  @spec centroid(t()) :: t()
  def centroid(%__MODULE__{geometry: geometry} = gdf) do
    %{gdf | geometry: GeoSeries.centroid(geometry)}
  end

  # -- conversion -----------------------------------------------------------------------

  @doc """
  Converts to an `Explorer.DataFrame` with the geometry columns last (active first).

  Options: `:encoding` - `:wkb` (default, binary column) or `:wkt` (string column). See
  `to_wkb/2` and `to_wkt/2`.
  """
  @spec to_explorer(t(), keyword()) :: DataFrame.t()
  def to_explorer(%__MODULE__{} = gdf, opts \\ []) do
    GeoFrame.Options.validate!(opts, [:encoding])

    case Keyword.get(opts, :encoding, :wkb) do
      :wkb -> to_wkb(gdf)
      :wkt -> to_wkt(gdf)
    end
  end

  @doc """
  Every geometry column, the active one and the secondary ones, encoded as WKB in an
  `Explorer.DataFrame`; the attribute columns are unchanged and the geometry columns come last
  (active first). Like GeoPandas `GeoDataFrame.to_wkb`; null geometries stay `nil`.

  Options: `:hex` (default `false`) gives upper-case hexadecimal strings instead of binaries,
  and `:srid` (an integer, or `true` for each column's own `EPSG:` CRS) gives PostGIS EWKB; see
  `GeoFrame.GeoSeries.to_wkb/2`.
  """
  @spec to_wkb(t(), keyword()) :: DataFrame.t()
  def to_wkb(%__MODULE__{} = gdf, opts \\ []) do
    opts = Keyword.validate!(opts, hex: false, srid: nil)
    dtype = if opts[:hex], do: :string, else: :binary
    encode_geometries(gdf, &GeoSeries.to_wkb(&1, opts), dtype)
  end

  @doc """
  Every geometry column encoded as WKT strings in an `Explorer.DataFrame`, like GeoPandas
  `GeoDataFrame.to_wkt`; see `to_wkb/2` for the layout. Takes no options (GeoPandas passes
  its keyword arguments to shapely, such as `rounding_precision`, which are not supported).
  """
  @spec to_wkt(t(), keyword()) :: DataFrame.t()
  def to_wkt(%__MODULE__{} = gdf, opts \\ []) do
    [] = Keyword.validate!(opts, [])
    encode_geometries(gdf, &GeoSeries.to_wkt/1, :string)
  end

  defp encode_geometries(gdf, encode, dtype) do
    columns = [{gdf.geometry_column, gdf.geometry} | gdf.secondary_geometries]

    Enum.reduce(columns, gdf.data, fn {name, series}, acc ->
      encoded = Series.from_list(encode.(series), dtype: dtype)

      if has_columns?(acc),
        do: DataFrame.put(acc, name, encoded),
        else: DataFrame.new([{name, encoded}])
    end)
  end

  @doc """
  Renames the active geometry column, like GeoPandas `GeoDataFrame.rename_geometry`.

  Raises `ArgumentError` (`"Column named <name> already exists"`) when the frame already has a
  column with that name, the geometry column itself included.
  """
  @spec rename_geometry(t(), String.t() | atom()) :: t()
  def rename_geometry(%__MODULE__{} = gdf, new_name) do
    name = to_string(new_name)

    if name in names(gdf),
      do: raise(ArgumentError, "Column named #{name} already exists"),
      else: %{gdf | geometry_column: name}
  end

  @doc false
  def preview(%__MODULE__{} = gdf), do: gdf |> head(@preview_rows) |> to_explorer(encoding: :wkt)

  @doc """
  The frame as a GeoJSON `FeatureCollection` map (RFC 7946).

  Each row is a `Feature`: the geometry (see `GeoFrame.GeoSeries.to_geojson/1`, `nil` for
  null or unrepresentable rows) plus the attribute columns as string-keyed `properties`.
  Null attributes are kept as `nil`. A secondary geometry column becomes a property holding
  its GeoJSON geometry map (GeoPandas keeps the shapely object there). Property values are the Elixir values Explorer yields
  (dates, decimals, ...), so they encode with `Jason` for the common column types.
  """
  @spec to_geojson(t()) :: map()
  def to_geojson(%__MODULE__{data: data, geometry: geometry} = gdf) do
    geometries = GeoSeries.to_geojson(geometry)

    properties =
      if DataFrame.names(data) == [],
        do: Enum.map(geometries, fn _ -> %{} end),
        else: DataFrame.to_rows(data)

    properties = GeoFrame.Features.with_geometries(properties, gdf.secondary_geometries)

    features =
      Enum.zip_with(geometries, properties, fn geom, props ->
        %{"type" => "Feature", "geometry" => geom, "properties" => props}
      end)

    %{"type" => "FeatureCollection", "features" => features}
  end

  @doc """
  The frame as a GeoJSON `FeatureCollection` string, like GeoPandas `GeoDataFrame.to_json`.

  Each row is a `Feature` with a string `id` (the row position), the attribute columns as
  `properties`, the `geometry` (`null` for null or unrepresentable rows) and, with
  `show_bbox`, its `bbox`. A frame with secondary geometry columns raises `ArgumentError`, as
  GeoPandas raises `TypeError` (a shape is not JSON serializable). Missing values (`nil`, NaN) are `null`. A CRS other than EPSG:4326
  adds the `crs` member as GeoPandas does; see `GeoFrame.GeoSeries.to_json/2`.

  Options: `:na` - `:null` (default) keeps missing properties as `null`, `:drop` omits them
  (`:keep` is rejected: it would emit NaN, which is not JSON); `:show_bbox` (default
  `false`); `:drop_id` (default `false`); `:to_wgs84` (default `false`).
  """
  @spec to_json(t(), keyword()) :: String.t()
  def to_json(%__MODULE__{data: data, geometry: geometry} = gdf, opts \\ []) do
    GeoFrame.Formats.single_geometry!(gdf, "to_json")
    opts = GeoFrame.Features.options!(opts, false)
    GeoFrame.Features.build(geometry, data, opts, :json)
  end

  @doc """
  The `__geo_interface__` of the frame: a `FeatureCollection` map (string keys) with `id`,
  `properties`, `geometry` and `bbox` per feature, and the collection `bbox`; missing values
  are `nil`. Secondary geometry columns are properties holding GeoJSON geometry maps (GeoPandas
  keeps the shapely objects there, which Elixir has no equivalent of).

  With `opts` it is GeoPandas `to_geo_dict`, whose defaults differ from `__geo_interface__`'s:
  `show_bbox` is `false` unless given. Options: `:na` - `:null` (default) keeps missing
  properties as `nil`, `:drop` omits them, `:keep` leaves values as Explorer yields them (`nil`,
  `:nan`); `:show_bbox` (default `false`); `:drop_id` (default `false`).
  """
  @spec to_geo_interface(t(), keyword()) :: map()
  def to_geo_interface(%__MODULE__{data: data, geometry: geometry} = gdf, opts \\ nil) do
    opts =
      case opts do
        nil ->
          GeoFrame.Features.options!([], true, :map)

        opts ->
          opts
          |> Keyword.validate!(na: :null, show_bbox: false, drop_id: false)
          |> GeoFrame.Features.options!(false, :map)
      end

    GeoFrame.Features.build(geometry, data, opts, :map, gdf.secondary_geometries)
  end

  # -- helpers --------------------------------------------------------------------------

  defp has_columns?(%DataFrame{} = data), do: DataFrame.names(data) != []

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, message}), do: raise(ArgumentError, message)

  defimpl Table.Reader do
    alias Explorer.DataFrame
    alias Explorer.Series
    alias GeoFrame.GeoDataFrame
    alias GeoFrame.GeoSeries

    def init(%GeoDataFrame{data: data, geometry: geometry} = gdf) do
      attributes =
        Enum.map(DataFrame.names(data), &(data |> DataFrame.pull(&1) |> Series.to_list()))

      secondary =
        Enum.map(gdf.secondary_geometries, fn {_name, series} -> GeoSeries.to_wkt(series) end)

      columns = attributes ++ [GeoSeries.to_wkt(geometry) | secondary]
      {:columns, %{columns: GeoDataFrame.names(gdf), count: GeoDataFrame.n_rows(gdf)}, columns}
    end
  end

  defimpl Inspect do
    import Inspect.Algebra

    alias GeoFrame.GeoDataFrame

    def inspect(gdf, opts) do
      header =
        "#GeoFrame.GeoDataFrame<rows: #{GeoDataFrame.n_rows(gdf)}, " <>
          "columns: #{Kernel.inspect(GeoDataFrame.names(gdf))}, " <>
          "crs: #{Kernel.inspect(GeoDataFrame.crs(gdf))}"

      preview = gdf |> GeoDataFrame.preview() |> to_doc(opts)
      concat([header, line(), preview, ">"]) |> nest(2)
    end
  end
end
