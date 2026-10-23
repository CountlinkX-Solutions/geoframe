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
  @typedoc "The right-hand side of a binary operation: a series, or a WKT string that broadcasts."
  @type other :: t() | String.t()

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

  @doc """
  GeoJSON geometry objects (RFC 7946), one decoded map per row.

  Null rows, and rows that cannot be expressed as a `geo` geometry (such as `POINT EMPTY`),
  give `nil`. Coordinates are emitted as stored: GeoJSON has no CRS member, so reproject to
  longitude/latitude (`EPSG:4326`) first if the data is in another CRS.

  The conversion runs in Rust and returns JSON text, which is decoded here with `Jason`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 2)", nil])
      iex> GeoFrame.GeoSeries.to_geojson(s)
      [%{"type" => "Point", "coordinates" => [1.0, 2.0]}, nil]
  """
  @spec to_geojson(t()) :: [map() | nil]
  def to_geojson(%__MODULE__{ref: ref}) do
    ref |> Native.to_geojson() |> Enum.map(&decode_geojson/1)
  end

  defp decode_geojson(nil), do: nil
  defp decode_geojson(json), do: Jason.decode!(json)

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

  # -- binary predicates ----------------------------------------------------------------

  @predicate_docs [
    intersects: """
    Whether the geometries share at least one point (boundary contact counts).
    This is the negation of `disjoint/2`.
    """,
    contains: """
    Whether no point of `other` lies outside the geometry and their interiors share a point.

    Boundary points are **not** contained: a polygon does not `contains` a point on its edge,
    nor a line lying along its boundary (use `covers/2` for that). `contains(a, b)` equals
    `within(b, a)`.
    """,
    within: """
    Whether the geometry lies inside `other` with interiors sharing a point. The converse of
    `contains/2`, so a point on the boundary of `other` is not `within` it (see `covered_by/2`).
    """,
    touches: """
    Whether the geometries share boundary points but no interior points, such as two squares
    sharing an edge or a point on a polygon edge.
    """,
    crosses: """
    Whether the interiors intersect and the intersection has a lower dimension than the larger
    geometry, such as two lines crossing at a point or a line passing through a polygon.
    Polygons never cross polygons.
    """,
    overlaps: """
    Whether the geometries have the same dimension, their interiors intersect, and neither
    contains the other.
    """,
    disjoint: """
    Whether the geometries share no point at all. This is the negation of `intersects/2`.
    """,
    covers: """
    Whether no point of `other` lies outside the geometry. Unlike `contains/2` this is true for
    points and lines on the boundary: a square covers a point on its edge.
    """,
    covered_by: """
    Whether no point of the geometry lies outside `other`. The converse of `covers/2`; unlike
    `within/2` it is true for boundary points.
    """,
    geom_equals: """
    Topological equality: both geometries describe the same set of points, regardless of vertex
    order, start vertex, ring orientation or redundant vertices. Named `geom_equals` after
    GeoPandas' `geom_equals` (the exact-coordinate comparison is not provided).
    """
  ]

  @predicate_intro """
  Element-wise, with GeoPandas semantics, returning a boolean `Explorer.Series`.

  `other` is a `GeoSeries` of the same length, or a single geometry (a WKT string, or a
  one-row `GeoSeries`) that is compared with every row. Rows where either side is null, or is
  an empty geometry, are `nil`. A length mismatch raises `ArgumentError`, as does a CRS
  mismatch between two series that both have a CRS (GeoPandas only warns; GeoFrame fails
  loudly). Series with no CRS are compatible with any CRS.
  """

  for {name, text} <- @predicate_docs do
    native_name = Atom.to_string(name)

    @doc "#{text}
#{@predicate_intro}"
    @spec unquote(name)(t(), other()) :: Series.t()
    def unquote(name)(%__MODULE__{ref: left} = series, other) do
      right = other_ref(series, other)
      left |> Native.predicate(right, unquote(native_name)) |> boolean_series()
    end
  end

  @doc """
  Planar Euclidean distance between each pair, as an f64 series. Zero when they intersect.

  Distances are in CRS units (degrees for `EPSG:4326`); there is no geodesic variant.
  Pairing, null and CRS rules are those of `intersects/2`.
  """
  @spec distance(t(), other()) :: Series.t()
  def distance(%__MODULE__{ref: left} = series, other) do
    left |> Native.distance(other_ref(series, other)) |> unwrap!() |> float_series()
  end

  @doc """
  Whether each pair is within `limit` of each other: `distance(a, b) <= limit` (inclusive).

  Pairing, null and CRS rules are those of `intersects/2`.
  """
  @spec dwithin(t(), other(), number()) :: Series.t()
  def dwithin(%__MODULE__{ref: left} = series, other, limit) when is_number(limit) do
    left |> Native.dwithin(other_ref(series, other), limit * 1.0) |> boolean_series()
  end

  @doc """
  The DE-9IM intersection matrix of each pair as a nine-character string series.

  Characters are ordered interior/boundary/exterior of the left geometry against
  interior/boundary/exterior of `other`, each one of `F` (empty), `0`, `1` or `2` (the
  dimension of the intersection). For example two squares sharing an edge give `"FF2F11212"`.
  Pairing, null and CRS rules are those of `intersects/2`.
  """
  @spec relate(t(), other()) :: Series.t()
  def relate(%__MODULE__{ref: left} = series, other) do
    left
    |> Native.relate(other_ref(series, other))
    |> unwrap!()
    |> Series.from_list(dtype: :string)
  end

  defp other_ref(_series, %__MODULE__{ref: ref}), do: ref

  defp other_ref(%__MODULE__{} = series, wkt) when is_binary(wkt) do
    %__MODULE__{ref: ref} = from_wkt!([wkt], crs: crs(series))
    ref
  end

  defp boolean_series(result), do: result |> unwrap!() |> Series.from_list(dtype: :boolean)

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
