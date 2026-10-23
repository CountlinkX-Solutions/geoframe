defmodule GeoFrame.GeoSeries do
  @moduledoc """
  A column of nullable geometries held in Rust as a GeoArrow array.

  The geometry never lives in Elixir terms: a `%GeoSeries{}` wraps a reference to a
  `geoarrow.geometry` column (mixed geometry types, CRS stored as GeoArrow extension
  metadata). Constructors parse WKT/WKB into that column and outputs serialise it back, so
  chained operations stay in native memory.

  Measures are planar (no geodesic maths), return `Explorer.Series`, and propagate nulls:
  `nil` rows, and rows that cannot be expressed as a `geo` geometry (such as `POINT EMPTY`),
  yield `nil`.

  ## Example

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))", nil])
      iex> GeoFrame.GeoSeries.area(s) |> Explorer.Series.to_list()
      [12.0, nil]
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.Native

  @enforce_keys [:ref]
  defstruct [:ref]

  @type t :: %__MODULE__{ref: reference()}
  @type bounds :: {float(), float(), float(), float()}

  @preview_rows 5

  # -- constructors ---------------------------------------------------------------------

  @doc """
  Builds a series from WKT strings (`nil` entries become null geometries).

  Options: `:crs` - a CRS string such as `"EPSG:4326"`, or `nil` (default).

  Returns `{:error, message}` naming the first invalid row.
  """
  @spec from_wkt([String.t() | nil], keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_wkt(rows, opts \\ []) when is_list(rows) do
    rows |> Native.from_wkt(Keyword.get(opts, :crs)) |> wrap()
  end

  @doc "Like `from_wkt/2` but raises `ArgumentError` on invalid input."
  @spec from_wkt!([String.t() | nil], keyword()) :: t()
  def from_wkt!(rows, opts \\ []), do: rows |> from_wkt(opts) |> unwrap!()

  @doc """
  Builds a series from WKB binaries (`nil` entries become null geometries).

  Accepts the same options as `from_wkt/2`.
  """
  @spec from_wkb([binary() | nil], keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_wkb(rows, opts \\ []) when is_list(rows) do
    rows |> Native.from_wkb(Keyword.get(opts, :crs)) |> wrap()
  end

  @doc "Like `from_wkb/2` but raises `ArgumentError` on invalid input."
  @spec from_wkb!([binary() | nil], keyword()) :: t()
  def from_wkb!(rows, opts \\ []), do: rows |> from_wkb(opts) |> unwrap!()

  # -- outputs and metadata -------------------------------------------------------------

  @doc "Returns the geometries as WKT strings (`nil` for nulls)."
  @spec to_wkt(t()) :: [String.t() | nil]
  def to_wkt(%__MODULE__{ref: ref}), do: ref |> Native.to_wkt() |> unwrap!()

  @doc "Returns the geometries as WKB binaries (`nil` for nulls)."
  @spec to_wkb(t()) :: [binary() | nil]
  def to_wkb(%__MODULE__{ref: ref}), do: ref |> Native.to_wkb() |> unwrap!()

  @doc "Number of rows, nulls included."
  @spec size(t()) :: non_neg_integer()
  def size(%__MODULE__{ref: ref}), do: Native.len(ref)

  @doc "The CRS as a string, or `nil` when unknown."
  @spec crs(t()) :: String.t() | nil
  def crs(%__MODULE__{ref: ref}), do: Native.crs(ref)

  @doc "Replaces the CRS metadata without touching coordinates (no reprojection)."
  @spec set_crs(t(), String.t() | nil) :: t()
  def set_crs(%__MODULE__{ref: ref}, crs) when is_binary(crs) or is_nil(crs) do
    %__MODULE__{ref: Native.set_crs(ref, crs)}
  end

  @doc """
  Selects rows by zero-based position; repeats and reordering are allowed.

  Raises `ArgumentError` for out-of-bounds indices.
  """
  @spec take(t(), [non_neg_integer()]) :: t()
  def take(%__MODULE__{ref: ref}, indices) when is_list(indices) do
    ref |> Native.take(indices) |> wrap() |> unwrap!()
  end

  # -- measures -------------------------------------------------------------------------

  @doc "Planar, unsigned area. Zero for points and lines."
  @spec area(t()) :: Series.t()
  def area(%__MODULE__{ref: ref}), do: ref |> Native.area() |> float_series()

  @doc "Planar length: line length, polygon perimeter, zero for points."
  @spec length(t()) :: Series.t()
  def length(%__MODULE__{ref: ref}), do: ref |> Native.length() |> float_series()

  @doc "Boolean series: whether each geometry is valid per the OGC rules."
  @spec is_valid(t()) :: Series.t()
  # GeoPandas-compatible name, deliberately kept despite the predicate naming convention.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_valid(%__MODULE__{ref: ref}) do
    Series.from_list(Native.validity(ref), dtype: :boolean)
  end

  @doc """
  Per-row bounds as a dataframe with `minx`, `miny`, `maxx`, `maxy`.

  Null and empty geometries give nulls.
  """
  @spec bounds(t()) :: DataFrame.t()
  def bounds(%__MODULE__{ref: ref}) do
    rows = Native.bounds(ref)

    columns =
      for {name, index} <- Enum.with_index(~w(minx miny maxx maxy)) do
        {name, Enum.map(rows, &(&1 && elem(&1, index)))}
      end

    DataFrame.new(columns, dtypes: Enum.map(~w(minx miny maxx maxy), &{&1, {:f, 64}}))
  end

  @doc "Bounds over all rows as `{minx, miny, maxx, maxy}`, or `nil` if there is none."
  @spec total_bounds(t()) :: bounds() | nil
  def total_bounds(%__MODULE__{ref: ref}), do: Native.total_bounds(ref)

  @doc "Centroids as a new series of points, keeping the CRS."
  @spec centroid(t()) :: t()
  def centroid(%__MODULE__{ref: ref}), do: ref |> Native.centroid() |> wrap() |> unwrap!()

  # -- helpers --------------------------------------------------------------------------

  defp float_series(values), do: Series.from_list(values, dtype: {:f, 64})

  defp wrap({:ok, ref}), do: {:ok, %__MODULE__{ref: ref}}
  defp wrap({:error, _} = error), do: error

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, message}), do: raise(ArgumentError, message)

  @doc false
  def preview(%__MODULE__{} = series) do
    series |> to_wkt() |> Enum.take(@preview_rows)
  end

  defimpl Inspect do
    import Inspect.Algebra

    alias GeoFrame.GeoSeries

    def inspect(series, _opts) do
      size = GeoSeries.size(series)
      rows = GeoSeries.preview(series)
      suffix = if size > length(rows), do: [", ..."], else: []

      meta = "size: #{size}, crs: #{Kernel.inspect(GeoSeries.crs(series))}"
      shown = Enum.map(rows, &Kernel.inspect/1) |> Enum.intersperse(", ")

      concat([
        "#GeoFrame.GeoSeries<",
        meta,
        line(),
        "[",
        concat(shown ++ suffix),
        "]>"
      ])
      |> nest(2)
    end
  end
end
