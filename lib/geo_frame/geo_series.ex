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

  # -- unary constructive operations ------------------------------------------------------

  @unary_docs [
    convex_hull: """
    The smallest convex polygon containing each geometry. Degenerate inputs collapse: one
    distinct point gives a point, collinear points a line. Empty geometries give `nil`.
    """,
    envelope: """
    The axis-aligned bounding rectangle as a polygon. A point gives a point and a horizontal
    or vertical line gives a line (the degenerate rectangles). Empty geometries give `nil`.
    """,
    boundary: """
    The topological boundary: a polygon's rings as a line (or multi-line with holes), a line's
    endpoints as a multi-point (mod-2 rule; empty for closed lines), and an empty collection
    for points. Geometry collections give `nil` (Shapely raises).
    """,
    make_valid: """
    Repairs invalid polygons and multipolygons (self-intersections, bowties) into valid
    (multi)polygons, using `geo`'s triangulation-based repair, which differs from GEOS'
    `make_valid` in vertex detail but not in the area it yields for simple cases. Valid
    geometries and non-polygons are returned unchanged. A repair that fails gives `nil`.
    """,
    representative_point: """
    A point guaranteed to lie within (or on) each geometry, unlike the centroid of a concave
    polygon. The point chosen may differ from GEOS' choice. Empty geometries give `nil`.
    """,
    exterior: """
    The exterior ring of each polygon as a line. Every non-polygon row gives `nil`, where
    GeoPandas raises. Interior rings are not returned.
    """
  ]

  for {name, text} <- @unary_docs do
    native_name = Atom.to_string(name)

    @doc """
    #{text}
    Returns a `GeoSeries` with the same CRS. Null rows, and rows the operation is undefined
    for, are `nil`. Output is 2D: Z and M ordinates are dropped.
    """
    @spec unquote(name)(t()) :: t()
    def unquote(name)(%__MODULE__{ref: ref}) do
      ref |> Native.unary(unquote(native_name)) |> wrap() |> unwrap!()
    end
  end

  @cap_styles ~w(round flat square)a
  @join_styles ~w(round mitre bevel)a

  @doc """
  Buffers each geometry by `distance` (a number, in CRS units; negative shrinks polygons and
  can empty them). The result is a polygon or multipolygon.

  Options (Shapely names):

    * `:cap_style` - `:round` (default), `:flat` or `:square`; the ends of lines.
    * `:join_style` - `:round` (default), `:mitre` or `:bevel`; corners of lines and of
      expanded polygons.
    * `:quad_segs` - segments per quarter circle for round caps and joins, default `16`.
      The backend clamps the resulting arc step, so values above about 50 or below 2 behave
      as those bounds.
    * `:mitre_limit` - for `:mitre` joins, the longest mitre as a multiple of `distance`,
      default `5.0`, at least `1.0`.

  `single_sided` is not supported. Invalid options raise `ArgumentError`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (0 0)"])
      iex> [area] = s |> GeoFrame.GeoSeries.buffer(1) |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list()
      iex> Float.round(area, 1)
      3.1
  """
  @spec buffer(t(), number(), keyword()) :: t()
  def buffer(%__MODULE__{ref: ref}, distance, opts \\ []) when is_number(distance) do
    opts =
      Keyword.validate!(opts,
        cap_style: :round,
        join_style: :round,
        quad_segs: 16,
        mitre_limit: 5.0
      )

    cap = enum!(opts, :cap_style, @cap_styles)
    join = enum!(opts, :join_style, @join_styles)
    quad_segs = opts[:quad_segs]
    mitre_limit = opts[:mitre_limit]

    unless is_integer(quad_segs) and quad_segs > 0,
      do:
        raise(ArgumentError, ":quad_segs must be a positive integer, got: #{inspect(quad_segs)}")

    unless is_number(mitre_limit) and mitre_limit >= 1,
      do: raise(ArgumentError, ":mitre_limit must be a number >= 1, got: #{inspect(mitre_limit)}")

    ref
    |> Native.buffer(
      distance * 1.0,
      Atom.to_string(cap),
      Atom.to_string(join),
      quad_segs,
      mitre_limit * 1.0
    )
    |> wrap()
    |> unwrap!()
  end

  @doc """
  Simplifies lines and polygon rings with `tolerance`; points are unchanged.

  Option `:preserve_topology` (default `true`) picks the algorithm and, with it, the meaning
  of `tolerance`:

    * `true` - Visvalingam-Whyatt with topology checks (`geo`'s `SimplifyVwPreserve`):
      `tolerance` is a minimum triangle **area** and rings never self-intersect or collapse.
    * `false` - Ramer-Douglas-Peucker (`geo`'s `Simplify`): `tolerance` is a maximum
      **distance** from the original, like Shapely's `preserve_topology=False`; output can
      be invalid.

  Shapely's topology-preserving mode is a distance-based Douglas-Peucker variant, so the
  same `tolerance` yields different vertices here in the default mode.
  """
  @spec simplify(t(), number(), keyword()) :: t()
  def simplify(%__MODULE__{ref: ref}, tolerance, opts \\ []) when is_number(tolerance) do
    opts = Keyword.validate!(opts, preserve_topology: true)

    unless is_boolean(opts[:preserve_topology]),
      do: raise(ArgumentError, ":preserve_topology must be a boolean")

    ref |> Native.simplify(tolerance * 1.0, opts[:preserve_topology]) |> wrap() |> unwrap!()
  end

  # -- affine transforms ----------------------------------------------------------------

  @doc """
  Shifts every coordinate by `xoff` and `yoff`. Output is 2D.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)"])
      iex> s |> GeoFrame.GeoSeries.translate(2, 3) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(3 4)"]
  """
  @spec translate(t(), number(), number()) :: t()
  def translate(%__MODULE__{ref: ref}, xoff, yoff) when is_number(xoff) and is_number(yoff) do
    ref |> Native.translate(xoff * 1.0, yoff * 1.0) |> wrap() |> unwrap!()
  end

  @doc """
  Scales by `xfact` and `yfact` about `:origin` (negative factors mirror).

  `:origin` is `:center` (default; the centre of each geometry's bounding box, as in
  GeoPandas), `:centroid`, or an `{x, y}` pair used for every row. Output is 2D.
  """
  @spec scale(t(), number(), number(), keyword()) :: t()
  def scale(%__MODULE__{ref: ref}, xfact, yfact, opts \\ [])
      when is_number(xfact) and is_number(yfact) do
    {origin, ox, oy} = origin!(opts)
    ref |> Native.scale(xfact * 1.0, yfact * 1.0, origin, ox, oy) |> wrap() |> unwrap!()
  end

  @doc """
  Rotates counter-clockwise by `angle` **degrees** about `:origin`.

  `:origin` accepts the same values as in `scale/4` and defaults to `:center`. Output is 2D.
  """
  @spec rotate(t(), number(), keyword()) :: t()
  def rotate(%__MODULE__{ref: ref}, angle, opts \\ []) when is_number(angle) do
    {origin, ox, oy} = origin!(opts)
    ref |> Native.rotate(angle * 1.0, origin, ox, oy) |> wrap() |> unwrap!()
  end

  defp origin!(opts) do
    opts = Keyword.validate!(opts, origin: :center)

    case opts[:origin] do
      :center ->
        {"center", 0.0, 0.0}

      :centroid ->
        {"centroid", 0.0, 0.0}

      {x, y} when is_number(x) and is_number(y) ->
        {"coord", x * 1.0, y * 1.0}

      other ->
        raise ArgumentError,
              ":origin must be :center, :centroid or {x, y}, got: #{inspect(other)}"
    end
  end

  defp enum!(opts, key, allowed) do
    value = Keyword.fetch!(opts, key)

    if value in allowed do
      value
    else
      raise ArgumentError,
            "#{inspect(key)} must be one of #{inspect(allowed)}, got: #{inspect(value)}"
    end
  end

  # -- accessors ------------------------------------------------------------------------

  @doc """
  X coordinate of each point as an f64 series. Non-point geometries, empty points and nulls
  give `nil` (GeoPandas raises for non-points).
  """
  @spec x(t()) :: Series.t()
  def x(%__MODULE__{ref: ref}), do: ref |> Native.x() |> float_series()

  @doc "Y coordinate of each point; same rules as `x/1`."
  @spec y(t()) :: Series.t()
  def y(%__MODULE__{ref: ref}), do: ref |> Native.y() |> float_series()

  @doc """
  The geometry type name per row, as GeoPandas/Shapely spell it: `"Point"`, `"LineString"`,
  `"Polygon"`, `"MultiPoint"`, `"MultiLineString"`, `"MultiPolygon"` or
  `"GeometryCollection"`; `nil` for null rows.

  Note that a one-member `GEOMETRYCOLLECTION` is stored as that member when parsed, so it
  reports the member's type.
  """
  @spec geom_type(t()) :: Series.t()
  def geom_type(%__MODULE__{ref: ref}) do
    ref |> Native.geom_type() |> Series.from_list(dtype: :string)
  end

  @doc "Whether each geometry is empty (such as `POINT EMPTY`); `nil` for null rows."
  @spec is_empty(t()) :: Series.t()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_empty(%__MODULE__{ref: ref}) do
    Series.from_list(Native.empty_flags(ref), dtype: :boolean)
  end

  @doc """
  Number of coordinates in each geometry as an s64 series (a closed ring counts its closing
  vertex). Empty geometries give `0`; null rows `nil`.
  """
  @spec count_coordinates(t()) :: Series.t()
  def count_coordinates(%__MODULE__{ref: ref}) do
    ref |> Native.count_coordinates() |> Series.from_list(dtype: {:s, 64})
  end

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
  All pairs of rows `{i, j}` for which `left[i].predicate(right[j])` holds, found with an
  R-tree built over the bounding boxes of `right` and refined with the exact predicate.

  Returns `{left_indices, right_indices}`: two parallel lists of zero-based row positions,
  ordered by left then right position. (Plain lists rather than series: they are consumed as
  `take/2` indices, and the pair count is data-dependent.) The result equals evaluating the
  predicate over every pair, only faster.

  `predicate` is one of `:intersects` (default), `:within`, `:contains`, `:touches`,
  `:crosses`, `:overlaps`, `:covers`, `:covered_by`, `:geom_equals` or `:dwithin`. The
  predicate reads `left.predicate(right)`: `:within` means the left geometry is within the
  right one. `:disjoint` is rejected because a spatial index cannot answer it.

  Options: `:distance` - required, non-negative, in CRS units for `:dwithin`.

  Null and empty rows never match. Differing CRS (both set) raise `ArgumentError`, as do
  unknown predicates and a missing distance. Querying is single-threaded.
  """
  @spec sindex_query(t(), t(), atom(), keyword()) :: {[non_neg_integer()], [non_neg_integer()]}
  def sindex_query(left, right, predicate \\ :intersects, opts \\ [])

  def sindex_query(%__MODULE__{ref: left}, %__MODULE__{ref: right}, predicate, opts)
      when is_atom(predicate) and is_list(opts) do
    distance = opts |> Keyword.get(:distance) |> then(&(&1 && &1 * 1.0))

    left
    |> Native.sindex_query(right, Atom.to_string(predicate), distance)
    |> unwrap!()
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

  # -- binary set operations ------------------------------------------------------------

  @set_op_docs [
    intersection: "The part shared by both geometries.",
    union: "All points in either geometry.",
    difference: "The part of the geometry that is not in `other`.",
    symmetric_difference: "Points in exactly one of the two geometries."
  ]

  for {name, doc} <- @set_op_docs do
    @doc """
    #{doc} Element-wise; returns a GeoSeries with the CRS of the left side.

    Pairing, null and CRS rules are those of `intersects/2` (`other` may be a series of the
    same length, or a single geometry / WKT string that broadcasts).

    Supported: polygonal x polygonal in full; line x polygon for `intersection` and
    `difference` (only the one-dimensional part is kept); points against any geometry for
    `intersection` and `difference`, and point x point for all four. Every other combination
    (and `GeometryCollection` input) gives `nil`. Empty results are empty geometries
    (`POLYGON EMPTY`, `LINESTRING EMPTY`, `MULTIPOINT EMPTY`), not `nil`. Lower-dimensional
    results are dropped: polygons sharing only an edge intersect in `POLYGON EMPTY` where
    Shapely returns the shared `LINESTRING`.
    """
    @spec unquote(name)(t(), other()) :: t()
    def unquote(name)(%__MODULE__{ref: left} = series, other) do
      left
      |> Native.set_op(other_ref(series, other), unquote(Atom.to_string(name)))
      |> wrap()
      |> case do
        {:ok, result} -> result
        {:error, message} -> raise ArgumentError, message
      end
    end
  end

  @doc """
  The union of all non-null geometries as a **one-row** GeoSeries (GeoPandas returns a scalar).

  Polygons are merged with a single `unary_union`, points are merged and deduplicated, and
  identical lines are merged into a `MultiLineString`. A mix of geometry families gives a
  `nil` row; no non-null input gives `GEOMETRYCOLLECTION EMPTY`.
  """
  @spec union_all(t()) :: t()
  def union_all(%__MODULE__{ref: ref}) do
    {:ok, result} = ref |> Native.union_all() |> wrap()
    result
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
