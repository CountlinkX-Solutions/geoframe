defmodule GeoFrame.PostGIS.Values do
  @moduledoc false
  # Turns the values Postgrex decodes for one result column into an Explorer series.
  #
  #   * `numeric` becomes an exact `{:decimal, precision, scale}` series (Polars decimals hold at
  #     most 38 digits; wider values become exact text, and NaN / Infinity force float64);
  #   * structs Explorer has no dtype for (intervals, ranges, inet, geometric types, ...) are
  #     rendered as text;
  #   * a column without a single non-null value takes its dtype from the Postgres type OID, so
  #     empty and all-NULL results keep their types.

  alias Explorer.Series

  @compile {:no_warn_undefined, [Decimal]}

  @max_decimal_digits 38
  @uuid_oid 2950

  @oid_dtypes %{
    16 => :boolean,
    17 => :binary,
    20 => {:s, 64},
    21 => {:s, 64},
    23 => {:s, 64},
    26 => {:s, 64},
    700 => {:f, 64},
    701 => {:f, 64},
    1082 => :date,
    1083 => :time,
    1114 => {:naive_datetime, :microsecond},
    1184 => {:datetime, :microsecond, "Etc/UTC"},
    1700 => {:decimal, @max_decimal_digits, 0}
  }

  @doc """
  Builds the series for a column. Returns `{:error, message}` for values that cannot be
  represented (for example a `bytea[]` that is not valid JSON).
  """
  @spec to_series(String.t(), non_neg_integer() | nil, [term()]) ::
          {:ok, Series.t()} | {:error, String.t()}
  def to_series(name, oid, values) do
    values |> Enum.map(&convert(&1, oid)) |> series(oid)
  rescue
    error in [Jason.EncodeError, Protocol.UndefinedError, ArgumentError, RuntimeError] ->
      {:error, "column #{inspect(name)} cannot be read: #{Exception.message(error)}"}
  end

  defp series(values, oid) do
    if Enum.any?(values, &decimal?/1),
      do: {:ok, decimal_series(values)},
      else: {:ok, build(values, infer_dtype(values, oid))}
  end

  defp build(values, {:f, 64}),
    do: Series.from_list(Enum.map(values, &to_float/1), dtype: {:f, 64})

  defp build(values, dtype), do: Series.from_list(values, dtype: dtype)

  # -- numeric --------------------------------------------------------------------------

  defp decimal?(%{__struct__: Decimal}), do: true
  defp decimal?(_), do: false

  defp decimal_series(values) do
    finite = for %{__struct__: Decimal} = d <- values, finite?(d), do: d

    cond do
      Enum.any?(values, &special?/1) ->
        Series.from_list(Enum.map(values, &decimal_to_float/1), dtype: {:f, 64})

      (shape = shape(finite)) && elem(shape, 0) <= @max_decimal_digits ->
        {precision, scale} = shape
        Series.from_list(values, dtype: {:decimal, precision, scale})

      true ->
        Series.from_list(
          Enum.map(values, &(&1 && Decimal.to_string(&1, :normal))),
          dtype: :string
        )
    end
  end

  defp finite?(%{coef: coef}), do: is_integer(coef)

  defp special?(%{__struct__: Decimal, coef: coef}), do: not is_integer(coef)
  defp special?(_), do: false

  # {precision, scale} wide enough for every value: the longest integer part plus the finest
  # fraction, so no digit is rounded.
  defp shape(decimals) do
    scale = decimals |> Enum.map(&max(-&1.exp, 0)) |> Enum.max(fn -> 0 end)

    integer_digits =
      decimals
      |> Enum.map(&max(digits(&1.coef) + &1.exp, 0))
      |> Enum.max(fn -> 0 end)

    {max(integer_digits + scale, 1), scale}
  end

  defp digits(0), do: 0
  defp digits(coef), do: coef |> Integer.to_string() |> byte_size()

  defp decimal_to_float(nil), do: nil
  defp decimal_to_float(%{coef: :NaN}), do: :nan
  defp decimal_to_float(%{coef: :inf, sign: 1}), do: :infinity
  defp decimal_to_float(%{coef: :inf}), do: :neg_infinity
  defp decimal_to_float(decimal), do: Decimal.to_float(decimal)

  # -- scalars --------------------------------------------------------------------------

  defp convert(:NaN, _oid), do: :nan
  defp convert(:inf, _oid), do: :infinity
  defp convert(:"-inf", _oid), do: :neg_infinity
  defp convert(<<_::binary-size(16)>> = raw, @uuid_oid), do: format_uuid(raw)
  defp convert(value, _oid) when is_map(value) and not is_struct(value), do: Jason.encode!(value)
  defp convert(value, _oid) when is_list(value), do: Jason.encode!(value)
  defp convert(%{__struct__: Decimal} = decimal, _oid), do: decimal
  defp convert(%Date{} = value, _oid), do: value
  defp convert(%Time{} = value, _oid), do: value
  defp convert(%NaiveDateTime{} = value, _oid), do: value
  defp convert(%DateTime{} = value, _oid), do: value
  # Postgrex is optional, so its structs are matched by name: `%Postgrex.Interval{}` would not
  # compile in a project (or a notebook) that does not depend on it.
  defp convert(%{__struct__: Postgrex.Interval} = value, _oid), do: format_interval(value)
  defp convert(%{__struct__: Postgrex.INET} = value, _oid), do: format_inet(value)
  defp convert(%{__struct__: Postgrex.MACADDR, address: address}, _oid), do: format_mac(address)
  defp convert(%{__struct__: Postgrex.Range} = value, _oid), do: format_range(value)
  defp convert(%{__struct__: Postgrex.Point, x: x, y: y}, _oid), do: "(#{x},#{y})"
  defp convert(value, _oid) when is_struct(value), do: inspect(value)
  defp convert(value, _oid) when is_tuple(value), do: inspect(value)
  defp convert(value, _oid), do: value

  defp to_float(value) when is_integer(value), do: value * 1.0
  defp to_float(value), do: value

  defp infer_dtype(values, oid) do
    case Enum.find(values, &(not is_nil(&1))) do
      nil -> Map.get(@oid_dtypes, oid, :string)
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

  # -- text renderings ------------------------------------------------------------------

  defp format_uuid(<<a::binary-4, b::binary-2, c::binary-2, d::binary-2, e::binary-6>>) do
    [a, b, c, d, e] |> Enum.map_join("-", &Base.encode16(&1, case: :lower))
  end

  # ISO 8601 duration, the same text PostgreSQL prints with `intervalstyle = iso_8601`.
  defp format_interval(%{months: months, days: days, secs: secs, microsecs: micros}) do
    total = secs * 1_000_000 + micros
    {hours, rest} = {div(total, 3_600_000_000), rem(total, 3_600_000_000)}
    {minutes, rest} = {div(rest, 60_000_000), rem(rest, 60_000_000)}

    date = part(div(months, 12), "Y") <> part(rem(months, 12), "M") <> part(days, "D")

    time =
      part(hours, "H") <> part(minutes, "M") <> if(rest == 0, do: "", else: seconds(rest) <> "S")

    case {date, time} do
      {"", ""} -> "PT0S"
      {date, ""} -> "P" <> date
      {date, time} -> "P" <> date <> "T" <> time
    end
  end

  defp part(0, _unit), do: ""
  defp part(n, unit), do: "#{n}#{unit}"

  defp seconds(micros) do
    whole = div(abs(micros), 1_000_000)
    frac = rem(abs(micros), 1_000_000)
    sign = if micros < 0, do: "-", else: ""

    if frac == 0,
      do: "#{sign}#{whole}",
      else:
        "#{sign}#{whole}.#{frac |> Integer.to_string() |> String.pad_leading(6, "0") |> String.trim_trailing("0")}"
  end

  defp format_inet(%{address: address, netmask: netmask}) do
    text = address |> :inet.ntoa() |> List.to_string()
    full = if tuple_size(address) == 4, do: 32, else: 128
    if netmask in [nil, full], do: text, else: "#{text}/#{netmask}"
  end

  defp format_mac(address) do
    address
    |> Tuple.to_list()
    |> Enum.map_join(
      ":",
      &(&1 |> Integer.to_string(16) |> String.pad_leading(2, "0") |> String.downcase())
    )
  end

  defp format_range(%{lower: :empty}), do: "empty"

  defp format_range(%{lower: lower, upper: upper} = range) do
    open = if range.lower_inclusive, do: "[", else: "("
    close = if range.upper_inclusive, do: "]", else: ")"
    "#{open}#{bound(lower)},#{bound(upper)}#{close}"
  end

  defp bound(:unbound), do: ""
  defp bound(:empty), do: "empty"
  defp bound(value) when is_binary(value) or is_number(value), do: to_string(value)
  defp bound(%{__struct__: Decimal} = value), do: Decimal.to_string(value, :normal)

  defp bound(value),
    do: if(String.Chars.impl_for(value), do: to_string(value), else: inspect(value))
end
