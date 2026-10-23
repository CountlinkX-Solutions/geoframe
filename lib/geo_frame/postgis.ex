defmodule GeoFrame.PostGIS do
  @moduledoc """
  PostGIS read and write behind `GeoFrame.read_postgis/3` and `GeoFrame.to_postgis/4`.

  Needs the optional `:postgrex` dependency. Geometry crosses the wire as EWKB and is parsed
  in Rust, never as Elixir structs. See `GeoFrame.Postgrex.Extension` for how a `geometry`
  column reaches Elixir.
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @compile {:no_warn_undefined, [Postgrex, Decimal]}

  # PostgreSQL allows at most 65535 bind parameters per statement.
  @max_params 65_535
  @if_exists [:fail, :replace, :append]
  @identifier_limit 63

  # -- read -----------------------------------------------------------------------------

  @doc false
  @spec read(term(), String.t(), keyword()) :: {:ok, GeoDataFrame.t()} | {:error, String.t()}
  def read(conn, sql, opts) when is_binary(sql) do
    opts = Keyword.validate!(opts, geom_col: "geom", crs: nil, params: [])
    ensure_postgrex!()
    geom_col = to_string(opts[:geom_col])

    with {:ok, %{columns: columns, rows: rows}} <- query(conn, sql, opts[:params]),
         :ok <- check_geometry_column(columns, geom_col) do
      build(columns, rows, geom_col, opts[:crs])
    end
  end

  defp query(conn, sql, params) do
    case Postgrex.query(conn, sql, params) do
      {:ok, result} -> {:ok, result}
      {:error, error} -> {:error, error_message(error)}
    end
  rescue
    error in [ArgumentError, RuntimeError] -> {:error, error_message(error)}
  end

  defp check_geometry_column(columns, geom_col) do
    if geom_col in (columns || []),
      do: :ok,
      else:
        {:error,
         "geometry column #{inspect(geom_col)} not found in the result " <>
           "(columns: #{inspect(columns)}); pass :geom_col"}
  end

  defp build(columns, rows, geom_col, crs) do
    by_column = columns |> Enum.zip(transpose(rows, length(columns))) |> Map.new()

    with {:ok, ewkb} <- ewkb_rows(Map.fetch!(by_column, geom_col)),
         {:ok, geometry} <- GeoSeries.from_wkb(ewkb, crs: crs) do
      attributes =
        for name <- columns, name != geom_col do
          {name, name |> then(&Map.fetch!(by_column, &1)) |> to_series()}
        end

      data = if attributes == [], do: DataFrame.new([]), else: DataFrame.new(attributes)
      {:ok, GeoDataFrame.new(data, geometry, geometry_column: geom_col)}
    end
  end

  defp transpose([], width), do: List.duplicate([], width)
  defp transpose(rows, _width), do: rows |> Enum.zip() |> Enum.map(&Tuple.to_list/1)

  # A binary starting with a WKB byte-order byte (0 or 1) is EWKB; hex text starts with the
  # ASCII digit of that byte, so the two never collide.
  defp ewkb_rows(values) do
    values
    |> Enum.reduce_while({:ok, []}, fn
      nil, {:ok, acc} ->
        {:cont, {:ok, [nil | acc]}}

      <<order, _::binary>> = raw, {:ok, acc} when order in [0, 1] ->
        {:cont, {:ok, [raw | acc]}}

      text, {:ok, acc} when is_binary(text) ->
        case Base.decode16(text, case: :mixed) do
          {:ok, raw} -> {:cont, {:ok, [raw | acc]}}
          :error -> {:halt, {:error, "geometry column holds text that is not hex-encoded EWKB"}}
        end

      other, {:ok, _} ->
        {:halt, {:error, "unsupported geometry value #{inspect(other)}"}}
    end)
    |> case do
      {:ok, acc} -> {:ok, Enum.reverse(acc)}
      error -> error
    end
  end

  defp to_series(values) do
    values = Enum.map(values, &convert/1)
    dtype = infer_dtype(values)
    values = if dtype == {:f, 64}, do: Enum.map(values, &to_float/1), else: values
    Series.from_list(values, dtype: dtype)
  end

  defp convert(%{__struct__: Decimal} = decimal), do: Decimal.to_float(decimal)
  defp convert(:NaN), do: :nan
  defp convert(:inf), do: :infinity
  defp convert(:"-inf"), do: :neg_infinity
  defp convert(value) when is_map(value) and not is_struct(value), do: Jason.encode!(value)
  defp convert(value) when is_list(value), do: Jason.encode!(value)
  defp convert(value), do: value

  defp to_float(value) when is_integer(value), do: value * 1.0
  defp to_float(value), do: value

  defp infer_dtype(values) do
    case Enum.find(values, &(not is_nil(&1))) do
      nil -> :string
      value when is_integer(value) or is_float(value) -> numeric_dtype(values)
      value when is_atom(value) and value not in [true, false] -> {:f, 64}
      value -> scalar_dtype(value, values)
    end
  end

  defp numeric_dtype(values) do
    if Enum.any?(values, &(is_float(&1) or (is_atom(&1) and not is_nil(&1)))),
      do: {:f, 64},
      else: {:s, 64}
  end

  defp scalar_dtype(value, _values) when is_boolean(value), do: :boolean
  defp scalar_dtype(%Date{}, _), do: :date
  defp scalar_dtype(%Time{}, _), do: :time
  defp scalar_dtype(%NaiveDateTime{}, _), do: {:naive_datetime, :microsecond}
  defp scalar_dtype(%DateTime{}, _), do: {:datetime, :microsecond, "Etc/UTC"}

  defp scalar_dtype(value, values) when is_binary(value) do
    if Enum.all?(values, &(is_nil(&1) or String.valid?(&1))), do: :string, else: :binary
  end

  defp scalar_dtype(_other, _values), do: :string

  # -- write ----------------------------------------------------------------------------

  @doc false
  @spec write(GeoDataFrame.t(), term(), String.t(), keyword()) :: :ok | {:error, String.t()}
  def write(%GeoDataFrame{} = gdf, conn, table, opts) do
    opts =
      Keyword.validate!(opts,
        if_exists: :fail,
        schema: nil,
        index: false,
        dtype: %{},
        spatial_index: false,
        chunksize: 1000
      )

    validate_opts!(opts)
    ensure_postgrex!()

    plan = plan(gdf, table, opts)

    case Postgrex.transaction(conn, fn conn -> run(conn, gdf, plan, opts) end) do
      {:ok, :ok} -> :ok
      {:error, message} when is_binary(message) -> {:error, message}
      {:error, other} -> {:error, error_message(other)}
    end
  end

  defp validate_opts!(opts) do
    unless opts[:if_exists] in @if_exists,
      do: raise(ArgumentError, ":if_exists must be one of #{inspect(@if_exists)}")

    if opts[:index],
      do:
        raise(
          ArgumentError,
          "GeoDataFrame has no index; add it as a column instead (index: false)"
        )

    unless is_integer(opts[:chunksize]) and opts[:chunksize] > 0,
      do: raise(ArgumentError, ":chunksize must be a positive integer")

    unless is_map(opts[:dtype]) or Keyword.keyword?(opts[:dtype]),
      do: raise(ArgumentError, ":dtype must be a map of column name to SQL type")
  end

  defp plan(gdf, table, opts) do
    schema = opts[:schema]

    qualified =
      if schema, do: quote_ident(schema) <> "." <> quote_ident(table), else: quote_ident(table)

    geom = gdf.geometry_column
    srid = srid_of(GeoSeries.crs(gdf.geometry))
    overrides = Map.new(opts[:dtype], fn {k, v} -> {to_string(k), validate_sql_type!(v)} end)

    attributes =
      for {name, dtype} <- DataFrame.dtypes(gdf.data) |> Enum.to_list() |> order(gdf.data) do
        {name, Map.get(overrides, name) || sql_type!(name, dtype)}
      end

    %{
      qualified: qualified,
      table: table,
      geom: geom,
      srid: srid,
      geom_type: geometry_type(gdf.geometry),
      attributes: attributes
    }
  end

  defp order(dtypes, data),
    do: Enum.map(DataFrame.names(data), &{&1, Map.fetch!(Map.new(dtypes), &1)})

  defp run(conn, gdf, plan, opts) do
    with {:ok, exists?} <- table_exists?(conn, plan.qualified),
         :ok <- prepare(conn, plan, exists?, opts[:if_exists]),
         :ok <- insert(conn, gdf, plan, opts[:chunksize]),
         :ok <- spatial_index(conn, plan, opts[:spatial_index]) do
      :ok
    else
      {:error, message} -> Postgrex.rollback(conn, message)
    end
  end

  defp table_exists?(conn, qualified) do
    case Postgrex.query(conn, "SELECT to_regclass($1) IS NOT NULL", [qualified]) do
      {:ok, %{rows: [[exists?]]}} -> {:ok, exists?}
      {:error, error} -> {:error, error_message(error)}
    end
  end

  defp prepare(_conn, plan, true, :fail),
    do: {:error, "table #{plan.qualified} already exists (if_exists: :fail)"}

  defp prepare(_conn, _plan, true, :append), do: :ok

  defp prepare(conn, plan, true, :replace) do
    with :ok <- exec(conn, "DROP TABLE #{plan.qualified}"), do: create(conn, plan)
  end

  defp prepare(conn, plan, false, _if_exists), do: create(conn, plan)

  defp create(conn, plan) do
    columns =
      Enum.map(plan.attributes, fn {name, type} -> "#{quote_ident(name)} #{type}" end) ++
        ["#{quote_ident(plan.geom)} geometry(#{plan.geom_type}, #{plan.srid})"]

    exec(conn, "CREATE TABLE #{plan.qualified} (#{Enum.join(columns, ", ")})")
  end

  defp insert(conn, gdf, plan, chunksize) do
    types = plan.attributes
    names = Enum.map(types, &elem(&1, 0))

    attribute_values =
      for {name, _sql} <- types do
        dtype = DataFrame.dtypes(gdf.data) |> Map.fetch!(name)
        gdf.data |> DataFrame.pull(name) |> Series.to_list() |> Enum.map(&encode(&1, dtype))
      end

    srid = if plan.srid > 0, do: plan.srid
    ewkb = GeoSeries.to_wkb(gdf.geometry, srid: srid)
    width = length(names) + 1
    batch = min(chunksize, div(@max_params, width))
    columns = Enum.map_join(names ++ [plan.geom], ", ", &quote_ident/1)

    (attribute_values ++ [ewkb])
    |> Enum.zip()
    |> Enum.chunk_every(batch)
    |> Enum.reduce_while(:ok, fn rows, :ok ->
      sql =
        "INSERT INTO #{plan.qualified} (#{columns}) VALUES #{placeholders(length(rows), width)}"

      params = Enum.flat_map(rows, &Tuple.to_list/1)

      case exec(conn, sql, params) do
        :ok -> {:cont, :ok}
        error -> {:halt, error}
      end
    end)
  end

  # `($1, $2, ST_GeomFromEWKB($3)), ($4, ...)`: the last parameter of each row is the geometry.
  defp placeholders(rows, width) do
    Enum.map_join(0..(rows - 1)//1, ", ", &row_placeholders(&1 * width, width))
  end

  defp row_placeholders(base, width) do
    inner = Enum.map_join(1..width, ", ", &placeholder(base + &1, &1 == width))
    "(#{inner})"
  end

  defp placeholder(n, true), do: "ST_GeomFromEWKB($#{n})"
  defp placeholder(n, false), do: "$#{n}"

  defp spatial_index(_conn, _plan, false), do: :ok

  defp spatial_index(conn, plan, true) do
    name = String.slice("#{plan.table}_#{plan.geom}_gist", 0, @identifier_limit)

    exec(
      conn,
      "CREATE INDEX IF NOT EXISTS #{quote_ident(name)} ON #{plan.qualified} USING GIST (#{quote_ident(plan.geom)})"
    )
  end

  defp exec(conn, sql, params \\ []) do
    case Postgrex.query(conn, sql, params) do
      {:ok, _} -> :ok
      {:error, error} -> {:error, error_message(error)}
    end
  end

  # -- types ----------------------------------------------------------------------------

  defp sql_type!(_name, {:s, size}) when size in [8, 16], do: "smallint"
  defp sql_type!(_name, {:s, 32}), do: "integer"
  defp sql_type!(_name, {:s, 64}), do: "bigint"
  defp sql_type!(_name, {:u, size}) when size in [8, 16], do: "integer"
  defp sql_type!(_name, {:u, 32}), do: "bigint"
  defp sql_type!(_name, {:u, 64}), do: "numeric(20, 0)"
  defp sql_type!(_name, {:f, 32}), do: "real"
  defp sql_type!(_name, {:f, 64}), do: "double precision"
  defp sql_type!(_name, :boolean), do: "boolean"
  defp sql_type!(_name, type) when type in [:string, :category], do: "text"
  defp sql_type!(_name, :binary), do: "bytea"
  defp sql_type!(_name, :date), do: "date"
  defp sql_type!(_name, :time), do: "time"
  defp sql_type!(_name, {:naive_datetime, _}), do: "timestamp"
  defp sql_type!(_name, {:datetime, _, _}), do: "timestamptz"
  defp sql_type!(_name, {:decimal, _, _}), do: "numeric"
  defp sql_type!(_name, {kind, _}) when kind in [:list, :struct], do: "jsonb"
  defp sql_type!(_name, {kind, _, _}) when kind in [:list, :struct], do: "jsonb"

  defp sql_type!(name, dtype),
    do: raise(ArgumentError, "column #{inspect(name)} has unsupported dtype #{inspect(dtype)}")

  # `dtype:` overrides are spliced into DDL, so only plain type names are accepted.
  defp validate_sql_type!(type) when is_binary(type) do
    if Regex.match?(~r/\A[A-Za-z_][A-Za-z0-9_ ]*(\(\s*\d+(\s*,\s*\d+)?\s*\))?(\[\])*\z/, type),
      do: type,
      else: raise(ArgumentError, "invalid SQL type in :dtype: #{inspect(type)}")
  end

  defp validate_sql_type!(other),
    do: raise(ArgumentError, ":dtype values must be SQL type strings, got: #{inspect(other)}")

  defp encode(:nan, _dtype), do: :NaN
  defp encode(:infinity, _dtype), do: :inf
  defp encode(:neg_infinity, _dtype), do: :"-inf"
  defp encode(value, {:u, 64}) when is_integer(value), do: Decimal.new(value)
  defp encode(value, _dtype), do: value

  defp geometry_type(geometry) do
    types =
      geometry
      |> GeoSeries.geom_type()
      |> Series.to_list()
      |> Enum.reject(&is_nil/1)
      |> Enum.uniq()

    case types do
      [single] -> single
      _ -> "Geometry"
    end
  end

  defp srid_of(nil), do: 0
  defp srid_of("OGC:CRS84"), do: 4326

  defp srid_of("EPSG:" <> code = crs) do
    case Integer.parse(code) do
      {srid, ""} when srid > 0 -> srid
      _ -> raise ArgumentError, "cannot derive a PostGIS SRID from CRS #{inspect(crs)}"
    end
  end

  defp srid_of(crs),
    do: raise(ArgumentError, "cannot derive a PostGIS SRID from CRS #{inspect(crs)}")

  defp quote_ident(name) do
    name = to_string(name)

    if name == "" or String.contains?(name, <<0>>),
      do: raise(ArgumentError, "invalid SQL identifier: #{inspect(name)}")

    ~s("#{String.replace(name, ~s("), ~s(""))}")
  end

  # -- helpers --------------------------------------------------------------------------

  defp error_message(%{postgres: %{message: message}}), do: message
  defp error_message(%{__exception__: true} = error), do: hint(Exception.message(error))
  defp error_message(other), do: inspect(other)

  defp hint(message) do
    if String.contains?(message, "can not be handled by the types module"),
      do:
        message <>
          " -- start Postgrex with `types: GeoFrame.Postgrex.Types`, or select the geometry as " <>
          "ST_AsEWKB(geom) AS geom",
      else: message
  end

  defp ensure_postgrex! do
    unless Code.ensure_loaded?(Postgrex),
      do:
        raise(
          ArgumentError,
          "PostGIS support needs the optional :postgrex dependency; add {:postgrex, \"~> 0.22\"} to your deps"
        )
  end
end
