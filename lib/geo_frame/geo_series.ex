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
  alias GeoFrame.Features
  alias GeoFrame.Native

  @enforce_keys [:ref]
  defstruct [:ref, precision: 0.0]

  @type t :: %__MODULE__{ref: reference(), precision: float()}
  @typedoc "Four ordinates; a non-finite one is `:nan`, `:infinity` or `:neg_infinity`."
  @type bounds :: {number() | atom(), number() | atom(), number() | atom(), number() | atom()}
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

  @doc """
  Encodes the series as an Arrow IPC **stream** (a binary) holding one table with a
  `geometry` column. The field carries the GeoArrow extension metadata
  (`ARROW:extension:name` = `geoarrow.wkb` or a native `geoarrow.*` name,
  `ARROW:extension:metadata` with the CRS as PROJJSON), so `pyarrow`, GeoPandas
  (`GeoSeries.from_arrow`) and other GeoArrow readers consume it directly.

  There is no Arrow C Data Interface binding on the BEAM, so IPC bytes are the interchange
  (one copy).

  Option `:geometry_encoding` - `:wkb` (default) or `:geoarrow`. The native encoding needs
  one geometry kind (a type and its multi variant may mix and become the multi type) and
  raises `ArgumentError` for mixed kinds or collections. Other failures also raise.
  """
  @spec to_arrow(t(), keyword()) :: binary()
  def to_arrow(%__MODULE__{} = series, opts \\ []),
    do: GeoFrame.Formats.series_to_arrow(series, opts)

  @doc """
  Decodes an Arrow IPC stream with a geometry column into a series.

  Accepts WKB (`geoarrow.wkb` or a plain binary column), WKT (`geoarrow.wkt` or a plain
  string column) and every native GeoArrow extension type (point, linestring, polygon and
  the multi types, interleaved or separated coordinates). The geometry column is found from
  the `geo` schema metadata, else the first GeoArrow extension column, else a column named
  `"geometry"`.

  Options: `:geometry` - the geometry column name when it cannot be detected; `:crs` - a
  CRS used only when the data carries none.

  Returns `{:error, message}` for invalid IPC data or a stream without a geometry column.
  """
  @spec from_arrow(binary(), keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_arrow(bytes, opts \\ []) when is_binary(bytes),
    do: GeoFrame.Formats.series_from_arrow(bytes, opts)

  @doc "Like `from_arrow/2` but raises `ArgumentError` on failure."
  @spec from_arrow!(binary(), keyword()) :: t()
  def from_arrow!(bytes, opts \\ []), do: bytes |> from_arrow(opts) |> unwrap!()

  @doc """
  Reads the geometry column of a vector file as a series (GeoPandas `GeoSeries.from_file`).

  Reads the file with `GeoFrame.read_file/2`, so the format comes from the extension or
  `driver:` and the other options are those of the format's reader; the attributes are
  dropped. Returns `{:ok, series}` or `{:error, message}`.
  """
  @spec from_file(Path.t(), keyword()) :: {:ok, t()} | {:error, String.t()}
  def from_file(path, opts \\ []) do
    with {:ok, %GeoFrame.GeoDataFrame{geometry: geometry}} <- GeoFrame.read_file(path, opts) do
      {:ok, geometry}
    end
  end

  @doc "Like `from_file/2` but raises `ArgumentError` on failure."
  @spec from_file!(Path.t(), keyword()) :: t()
  def from_file!(path, opts \\ []), do: path |> from_file(opts) |> unwrap_series!()

  @doc """
  Writes the series to a vector file (GeoPandas `GeoSeries.to_file`): a frame whose only
  column is the geometry, written by `GeoFrame.write_file/3`, which documents the format
  selection and options. Returns `:ok` or `{:error, message}`.
  """
  @spec to_file(t(), Path.t(), keyword()) :: :ok | {:error, String.t()}
  def to_file(%__MODULE__{} = series, path, opts \\ []) do
    GeoFrame.write_file(GeoFrame.GeoDataFrame.new(DataFrame.new([]), series), path, opts)
  end

  defp unwrap_series!({:ok, series}), do: series
  defp unwrap_series!({:error, message}), do: raise(ArgumentError, message)

  @doc """
  Builds a series of points from coordinate columns, like GeoPandas `points_from_xy`.

  `x` and `y` are lists or `Explorer.Series` of equal length. A row is a null geometry when
  either coordinate is `nil` or not a finite number (including `NaN`).

  Options:

    * `:crs` - a CRS string such as `"EPSG:4326"`, or `nil` (default).
    * `:z` - a list or `Explorer.Series` of the same length: the result is `POINT Z`
      (`nil` or NaN is a NaN Z ordinate, as `points_from_xy(x, y, z)` in GeoPandas). A row
      whose `x` or `y` is missing stays a null geometry.

  Raises `ArgumentError` when the columns differ in length or an option is unknown.

      iex> s = GeoFrame.GeoSeries.from_xy([1, 2], [3, nil])
      iex> GeoFrame.GeoSeries.to_wkt(s)
      ["POINT(1 3)", nil]
      iex> s = GeoFrame.GeoSeries.from_xy([1, 2], [3, 4], z: [5, 6])
      iex> GeoFrame.GeoSeries.to_wkt(s)
      ["POINT Z(1 3 5)", "POINT Z(2 4 6)"]
  """
  @spec from_xy(
          [number() | nil] | Series.t(),
          [number() | nil] | Series.t(),
          keyword()
        ) :: t()
  def from_xy(x, y, opts \\ []) do
    opts = Keyword.validate!(opts, crs: nil, z: nil)
    xs = coordinates!(x, :x)
    ys = coordinates!(y, :y)
    points = xs |> Native.from_xy(ys, opts[:crs]) |> wrap() |> unwrap!()

    case opts[:z] do
      nil -> points
      z -> with_z(points, coordinates!(z, :z), Kernel.length(xs))
    end
  end

  defp with_z(points, zs, size) when Kernel.length(zs) == size do
    points.ref |> Native.force_3d(zs) |> wrap() |> unwrap!()
  end

  defp with_z(_points, _zs, _size),
    do: raise(ArgumentError, "z must have the same length as x and y")

  defp coordinates!(%Series{} = series, name),
    do: series |> Series.to_list() |> coordinates!(name)

  defp coordinates!(values, _name) when is_list(values) do
    Enum.map(values, fn
      value when is_number(value) -> value * 1.0
      _missing_or_nan -> nil
    end)
  end

  defp coordinates!(other, name) do
    raise ArgumentError, "#{name} must be a list or an Explorer.Series, got: #{inspect(other)}"
  end

  @typedoc "A vertex: an `{x, y}` tuple or an `[x, y]` list of numbers."
  @type vertex :: {number(), number()} | [number()]

  @doc """
  Builds line strings from vertex lists, one list per row (GeoPolars `line_string`).

  A row is a list of vertices, `[]` for an empty line or `nil` for a null geometry; a vertex is
  `{x, y}` or `[x, y]`. A line needs at least two vertices. The constructors take 2D vertices only (a vertex with a
  third value is an error); build Z geometries with `from_wkt/2` or `force_3d/2`. Option: `:crs`.

      iex> s = GeoFrame.GeoSeries.line_strings_from_vertices([[{0, 0}, {1, 1}], nil])
      iex> GeoFrame.GeoSeries.to_wkt(s)
      ["LINESTRING(0 0,1 1)", nil]
  """
  @spec line_strings_from_vertices([[vertex()] | nil], keyword()) :: t()
  def line_strings_from_vertices(rows, opts \\ []) do
    from_vertices("line_string", rows, opts, fn row, i -> [[vertices!(row, i)]] end)
  end

  @doc """
  Builds multipoints from vertex lists, one list per row (GeoPolars `multi_point`). See
  `line_strings_from_vertices/2` for the row format.
  """
  @spec multi_points_from_vertices([[vertex()] | nil], keyword()) :: t()
  def multi_points_from_vertices(rows, opts \\ []) do
    from_vertices("multi_point", rows, opts, fn row, i -> [[vertices!(row, i)]] end)
  end

  @doc """
  Builds polygons from rings, one list of rings per row (GeoPolars `polygon`).

  A row is `[shell, hole, ...]` where each ring is a list of vertices; open rings are closed,
  and a ring needs three distinct vertices. `[]` is an empty polygon, `nil` a null geometry.
  Option: `:crs`.

      iex> s = GeoFrame.GeoSeries.polygons_from_rings([[[{0, 0}, {4, 0}, {4, 4}]]])
      iex> GeoFrame.GeoSeries.to_wkt(s)
      ["POLYGON((0 0,4 0,4 4,0 0))"]
  """
  @spec polygons_from_rings([[[vertex()]] | nil], keyword()) :: t()
  def polygons_from_rings(rows, opts \\ []) do
    from_vertices("polygon", rows, opts, fn row, i -> [rings!(row, i)] end)
  end

  @doc """
  Builds multi-line strings from lists of vertex lists, one list of lines per row
  (GeoPolars `multi_line_string`). Option: `:crs`.
  """
  @spec multi_line_strings_from_lines([[[vertex()]] | nil], keyword()) :: t()
  def multi_line_strings_from_lines(rows, opts \\ []) do
    from_vertices("multi_line_string", rows, opts, fn row, i ->
      Enum.map(rings!(row, i), &[&1])
    end)
  end

  @doc """
  Builds multipolygons from lists of polygons (each a list of rings), one per row
  (GeoPolars `multi_polygon`). Option: `:crs`.
  """
  @spec multi_polygons_from_polygons([[[[vertex()]]] | nil], keyword()) :: t()
  def multi_polygons_from_polygons(rows, opts \\ []) do
    from_vertices("multi_polygon", rows, opts, fn row, i ->
      unless is_list(row), do: raise(ArgumentError, "row #{i}: expected a list of polygons")
      Enum.map(row, &rings!(&1, i))
    end)
  end

  defp from_vertices(kind, rows, opts, nest) do
    opts = Keyword.validate!(opts, crs: nil)

    unless is_list(rows) do
      raise ArgumentError, "expected a list of rows, got: #{inspect(rows)}"
    end

    rows =
      rows
      |> Enum.with_index()
      |> Enum.map(fn
        {nil, _index} -> nil
        {row, index} -> nest.(row, index)
      end)

    kind |> Native.from_vertices(rows, opts[:crs]) |> wrap() |> unwrap!()
  end

  defp rings!(rings, row) when is_list(rings), do: Enum.map(rings, &vertices!(&1, row))
  defp rings!(_other, row), do: raise(ArgumentError, "row #{row}: expected a list of rings")

  defp vertices!(vertices, row) when is_list(vertices), do: Enum.map(vertices, &vertex!(&1, row))

  defp vertices!(_other, row),
    do: raise(ArgumentError, "row #{row}: expected a list of vertices")

  defp vertex!({x, y}, _row) when is_number(x) and is_number(y), do: {x * 1.0, y * 1.0}
  defp vertex!([x, y], _row) when is_number(x) and is_number(y), do: {x * 1.0, y * 1.0}

  defp vertex!(other, row) do
    raise ArgumentError, "row #{row}: invalid vertex #{inspect(other)}, expected {x, y} numbers"
  end

  # -- outputs and metadata -------------------------------------------------------------

  @doc "Returns the geometries as WKT strings (`nil` for nulls)."
  @spec to_wkt(t()) :: [String.t() | nil]
  def to_wkt(%__MODULE__{ref: ref}), do: ref |> Native.to_wkt() |> unwrap!()

  @doc """
  Returns the geometries as WKB (`nil` for nulls).

  Options:

    * `:hex` - when `true`, returns upper-case hexadecimal strings instead of binaries,
      like GeoPandas `to_wkb(hex=True)`. Default `false`.
    * `:srid` - an integer, or `true` to take it from an `"EPSG:<code>"` CRS, to emit PostGIS
      EWKB with that SRID (GeoPandas `to_wkb(include_srid=True)`). Default: plain WKB.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 2)"])
      iex> GeoFrame.GeoSeries.to_wkb(s, hex: true)
      ["0101000000000000000000F03F0000000000000040"]
      iex> GeoFrame.GeoSeries.to_wkb(s, hex: true, srid: 4326)
      ["0101000020E6100000000000000000F03F0000000000000040"]
  """
  @spec to_wkb(t(), keyword()) :: [binary() | nil]
  def to_wkb(%__MODULE__{ref: ref} = series, opts \\ []) do
    opts = Keyword.validate!(opts, hex: false, srid: nil)

    unless is_boolean(opts[:hex]), do: raise(ArgumentError, ":hex must be a boolean")

    rows =
      case wkb_srid(series, opts[:srid]) do
        nil -> ref |> Native.to_wkb() |> unwrap!()
        srid -> ref |> Native.to_ewkb(srid) |> unwrap!()
      end

    if opts[:hex],
      do: Enum.map(rows, &(&1 && Base.encode16(&1))),
      else: rows
  end

  defp wkb_srid(_series, nil), do: nil
  defp wkb_srid(_series, false), do: nil
  defp wkb_srid(_series, srid) when is_integer(srid) and srid >= 0, do: srid

  defp wkb_srid(series, true) do
    case crs(series) do
      "EPSG:" <> code ->
        case Integer.parse(code) do
          {srid, ""} -> srid
          _ -> raise ArgumentError, "srid: true needs an EPSG CRS, got #{inspect(crs(series))}"
        end

      other ->
        raise ArgumentError, "srid: true needs an EPSG CRS, got #{inspect(other)}"
    end
  end

  defp wkb_srid(_series, other),
    do:
      raise(ArgumentError, ":srid must be a non-negative integer or true, got: #{inspect(other)}")

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

  @doc """
  The series as a GeoJSON `FeatureCollection` string, like GeoPandas `GeoSeries.to_json`.

  Every row is a `Feature` with a string `id` (the row position), empty `properties`, the
  `geometry` (`null` for null or unrepresentable rows) and its `bbox`; the collection has the
  `bbox` of all rows. A CRS other than EPSG:4326 adds the `crs` member as GeoPandas writes it
  (`urn:ogc:def:crs:EPSG::32617`); CRS values that are not `AUTHORITY:CODE` add none. The JSON
  is compact, where Python's `json.dumps` puts spaces after `,` and `:`.

  Options: `:show_bbox` (default `true`), `:drop_id` (default `false`), `:to_wgs84` (default
  `false`, reprojects to EPSG:4326 first; needs a CRS).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 2)"])
      iex> GeoFrame.GeoSeries.to_json(s, show_bbox: false, drop_id: true)
      ~s({"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1.0,2.0]}}]})
  """
  @spec to_json(t(), keyword()) :: String.t()
  def to_json(%__MODULE__{} = series, opts \\ []) do
    opts = Keyword.validate!(opts, show_bbox: true, drop_id: false, to_wgs84: false)
    Features.build(series, nil, Features.options!(opts, true), :json)
  end

  @doc """
  The `__geo_interface__` of the series: a `FeatureCollection` map (string keys) with an `id`,
  empty `properties`, `geometry` and `bbox` per feature, and the collection `bbox`.
  """
  @spec to_geo_interface(t()) :: map()
  def to_geo_interface(%__MODULE__{} = series) do
    Features.build(series, nil, Features.options!([], true), :map)
  end

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

  `indices` is a list, or a binary of native-endian unsigned 32-bit integers (what
  `Explorer.Series.from_binary(binary, {:u, 32})` reads), which avoids building a list when
  millions of rows are selected.

  Raises `ArgumentError` for out-of-bounds indices.
  """
  @spec take(t(), [non_neg_integer()] | binary()) :: t()
  def take(%__MODULE__{ref: ref}, indices) when is_list(indices) do
    ref |> Native.take(indices) |> wrap() |> unwrap!()
  end

  def take(%__MODULE__{ref: ref}, indices) when is_binary(indices) do
    ref |> Native.take_u32(indices) |> wrap() |> unwrap!()
  end

  # -- measures -------------------------------------------------------------------------

  @geodesic_note """
  Geodesic measures use the WGS 84 ellipsoid (Karney's algorithms). A geographic CRS is read
  as lon/lat as it is, so one on another datum is still measured on WGS 84; a projected CRS
  is reprojected to lon/lat first; a series without a CRS raises `ArgumentError`
  (`set_crs` first).
  """

  @doc """
  Unsigned area. Zero for points and lines.

  Option `:method` - `:planar` (default, CRS units squared) or `:geodesic` (square metres,
  see `area_geodesic/1`).
  """
  @spec area(t(), keyword()) :: Series.t()
  def area(series, opts \\ [])

  def area(%__MODULE__{} = series, opts) do
    case measure_method!(opts, [:planar, :geodesic]) do
      :planar -> series.ref |> Native.area() |> float_series()
      :geodesic -> area_geodesic(series)
    end
  end

  @doc """
  Geodesic, unsigned area in square metres (GeoPolars `area_geodesic`). Holes are
  subtracted, ring orientation does not matter, points and lines are zero.

  #{@geodesic_note}

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"], crs: "EPSG:4326")
      iex> [area] = s |> GeoFrame.GeoSeries.area_geodesic() |> Explorer.Series.to_list()
      iex> round(area / 1.0e9)
      12
  """
  @spec area_geodesic(t()) :: Series.t()
  def area_geodesic(%__MODULE__{ref: ref}),
    do: ref |> Native.area_geodesic() |> unwrap!() |> float_series()

  @doc """
  Length: line length, polygon perimeter, zero for points.

  Option `:method` - `:planar` (default, CRS units) or `:geodesic` (metres, see
  `length_geodesic/1`).
  """
  @spec length(t(), keyword()) :: Series.t()
  def length(series, opts \\ [])

  def length(%__MODULE__{} = series, opts) do
    case measure_method!(opts, [:planar, :geodesic]) do
      :planar -> series.ref |> Native.length() |> float_series()
      :geodesic -> length_geodesic(series)
    end
  end

  @doc """
  Geodesic length in metres: line length, polygon perimeter (holes included), zero for
  points.

  #{@geodesic_note}

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 0 1)"], crs: "EPSG:4326")
      iex> [m] = s |> GeoFrame.GeoSeries.length_geodesic() |> Explorer.Series.to_list()
      iex> Float.round(m, 1)
      110574.4
  """
  @spec length_geodesic(t()) :: Series.t()
  def length_geodesic(%__MODULE__{ref: ref}),
    do: ref |> Native.length_geodesic() |> unwrap!() |> float_series()

  defp measure_method!(opts, allowed) do
    GeoFrame.Options.validate!(opts, [:method])
    method = Keyword.get(opts, :method, :planar)

    if method in allowed do
      method
    else
      raise ArgumentError, "method must be one of #{inspect(allowed)}, got: #{inspect(method)}"
    end
  end

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
    for, are `nil`. `boundary` and `exterior` keep Z and M; the other operations return 2D
    geometries (Shapely drops Z there too).
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
    * `false` - Ramer-Douglas-Peucker as GEOS does it (Shapely's `preserve_topology=False`):
      `tolerance` is a maximum **distance** from the original. A ring that collapses to fewer
      than four vertices is removed: a hole disappears, a polygon whose shell collapses
      becomes empty, multipolygon parts and collection members drop, and a multi geometry
      with one part left is that part. A ring's start vertex is dropped when it lies within the
      tolerance of the chord between its neighbours. Lines keep their two end points at least. The output
      can be invalid; GEOS repairs such a result with `buffer(0)` (a self-crossing ring,
      parts that touch along an edge) and this library does not, so those inputs differ.

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

  # -- reprojection ---------------------------------------------------------------------

  @doc """
  Reprojects to `target`, returning a copy whose CRS is `target`.

  `target` is `"EPSG:<code>"`, `"OGC:CRS84"` (lon/lat WGS 84) or an integer EPSG code
  (`3857` is `"EPSG:3857"`). The series must have a CRS (as in GeoPandas, a naive series
  raises: use `set_crs/2` first). Reprojecting to the CRS the series already has returns an
  unchanged copy.

  The engine is pure Rust (`proj4rs` with the bundled EPSG table), so no PROJ install is
  needed. Axis order is always x = easting/longitude, y = northing/latitude, for every CRS
  including `EPSG:4326` (GeoPandas `always_xy=True`). A source CRS stored as PROJJSON must
  carry a top-level `id` with authority and code. Rows that are null, or whose transform fails
  (outside the projection's domain), become null. Z is kept (unchanged between CRSs that are
  not geocentric; transformed to or from a geocentric CRS such as `EPSG:4978`); M is dropped.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (0 0)"], crs: "EPSG:4326")
      iex> s |> GeoFrame.GeoSeries.to_crs(3857) |> GeoFrame.GeoSeries.crs()
      "EPSG:3857"
  """
  @spec to_crs(t(), String.t() | pos_integer()) :: t()
  def to_crs(%__MODULE__{ref: ref}, target) when is_binary(target) or is_integer(target) do
    ref |> Native.to_crs(crs_label(target)) |> wrap() |> unwrap!()
  end

  @doc """
  The UTM CRS to use for this series, as GeoPandas `estimate_utm_crs`: the first EPSG UTM CRS of
  the datum, by code, whose area of use contains the centre of the total bounds (the answer of
  `pyproj.database.query_utm_crs_info/2`). Returns an `"EPSG:<code>"` string.

  A geographic series is centred on its raw bounds. For any other CRS the bounds are first
  converted to lon/lat (edges densified, a pole inside the box or a box crossing the
  antimeridian handled as PROJ's `transform_bounds` does) and the centre of that box is used.

  The zone exceptions of Norway (32V) and Svalbard (32X, 34X, 36X) are **not** applied: pyproj
  and GeoPandas do not apply them either (EPSG defines plain 6-degree zones), so `5 E, 60 N` is
  `"EPSG:32631"`. A point on a zone edge belongs to both zones and the lower code wins.

  Options:

    * `:datum_name` - the datum in the CRS name, `"WGS 84"` by default; `"NAD83"`, `"NAD27"`,
      `"ETRS89"`, `"SIRGAS 2000"`, ... Spaces are ignored, case is not. A datum with no UTM CRS
      over the centre (WGS 84 covers 80 S to 84 N; `"ETRS89"` only 6 W to 36 E, northern
      Europe) raises, as GeoPandas does.

  Raises `ArgumentError` for a series without a CRS or bounds, or without a UTM CRS for the
  datum at the centre.

  ## Examples

      iex> GeoFrame.GeoSeries.from_wkt!(["POINT (-80.2 25.7)"], crs: "EPSG:4326")
      ...> |> GeoFrame.GeoSeries.estimate_utm_crs()
      "EPSG:32617"

      iex> GeoFrame.GeoSeries.from_wkt!(["POINT (-80.2 25.7)"], crs: "EPSG:4326")
      ...> |> GeoFrame.GeoSeries.estimate_utm_crs(datum_name: "NAD83")
      "EPSG:26917"
  """
  @spec estimate_utm_crs(t(), keyword()) :: String.t()
  def estimate_utm_crs(%__MODULE__{ref: ref}, opts \\ []) do
    GeoFrame.Options.validate!(opts, [:datum_name])
    ref |> Native.estimate_utm_crs(Keyword.get(opts, :datum_name)) |> unwrap!()
  end

  defp crs_label(code) when is_integer(code), do: "EPSG:#{code}"
  defp crs_label(label) when is_binary(label), do: label

  # -- affine transforms ----------------------------------------------------------------

  @doc """
  Shifts every coordinate by `xoff` and `yoff`.

  Each offset is a number, or a list or `Explorer.Series` with one entry per row (a `nil`
  entry nulls that row). Z is kept; the `:zoff` option (a number, default `0`) shifts it.
  M is dropped (Shapely). `:zoff` is not available with per-row offsets.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)"])
      iex> s |> GeoFrame.GeoSeries.translate(2, 3) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(3 4)"]
  """
  @spec translate(t(), param(), param(), keyword()) :: t()
  def translate(%__MODULE__{ref: ref} = series, xoff, yoff, opts \\ []) do
    GeoFrame.Options.validate!(opts, [:zoff])
    zoff = z_factor!(opts, :zoff, 0)

    if is_number(xoff) and is_number(yoff) do
      ref |> Native.translate(xoff * 1.0, yoff * 1.0, zoff) |> wrap() |> unwrap!()
    else
      n = size(series)
      if zoff != 0.0, do: raise(ArgumentError, ":zoff needs scalar xoff and yoff")

      ref
      |> Native.translate_rows(param!(xoff, n, "xoff"), param!(yoff, n, "yoff"))
      |> wrap()
      |> unwrap!()
    end
  end

  @doc """
  Scales by `xfact` and `yfact` about `:origin` (negative factors mirror).

  `:origin` is `:center` (default; the centre of each geometry's bounding box, as in
  GeoPandas), `:centroid`, an `{x, y}` pair used for every row, or a list with one `{x, y}`
  (or `nil`) per row. The factors are numbers, lists or `Explorer.Series` (see `translate/4`).
  Z is kept; the `:zfact` option (a number, default `1`) scales it about zero. M is dropped
  (Shapely). `:zfact` is not available with per-row factors.
  """
  @spec scale(t(), param(), param(), keyword()) :: t()
  def scale(%__MODULE__{ref: ref} = series, xfact, yfact, opts \\ []) do
    zfact = z_factor!(opts, :zfact, 1)

    case origin!(Keyword.delete(opts, :zfact), size(series)) do
      {:scalar, origin, ox, oy} when is_number(xfact) and is_number(yfact) ->
        ref
        |> Native.scale(xfact * 1.0, yfact * 1.0, zfact, origin, ox, oy)
        |> wrap()
        |> unwrap!()

      {_, origin, ox, oy} ->
        n = size(series)
        if zfact != 1.0, do: raise(ArgumentError, ":zfact needs scalar xfact, yfact and origin")

        ref
        |> Native.scale_rows(
          param!(xfact, n, "xfact"),
          param!(yfact, n, "yfact"),
          origin,
          row_coords(ox, n),
          row_coords(oy, n)
        )
        |> wrap()
        |> unwrap!()
    end
  end

  @doc """
  Rotates counter-clockwise by `angle` **degrees** about `:origin`.

  `:origin` accepts the same values as in `scale/4` and defaults to `:center`; `angle` is a
  number, list or `Explorer.Series`. Z is kept; M is dropped.
  """
  @spec rotate(t(), param(), keyword()) :: t()
  def rotate(%__MODULE__{ref: ref} = series, angle, opts \\ []) do
    case origin!(opts, size(series)) do
      {:scalar, origin, ox, oy} when is_number(angle) ->
        ref |> Native.rotate(angle * 1.0, origin, ox, oy) |> wrap() |> unwrap!()

      {_, origin, ox, oy} ->
        n = size(series)

        ref
        |> Native.rotate_rows(
          param!(angle, n, "angle"),
          origin,
          row_coords(ox, n),
          row_coords(oy, n)
        )
        |> wrap()
        |> unwrap!()
    end
  end

  @doc """
  Shears by `xs` along x and `ys` along y about `:origin` (Shapely `skew`; x' = x + tan(xs) *
  (y - y0), y' = y + tan(ys) * (x - x0)).

  Options: `:origin` as in `scale/4` (default `:center`), `:use_radians` (default `false`,
  angles are degrees). `xs` and `ys` are numbers, lists or `Explorer.Series`. Z is kept; M is dropped.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 0 2)"])
      iex> skewed = GeoFrame.GeoSeries.skew(s, 45, 0, origin: {0, 0})
      iex> expected = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 2 2)"])
      iex> skewed |> GeoFrame.GeoSeries.geom_equals_exact(expected, 1.0e-9) |> Explorer.Series.to_list()
      [true]
  """
  @spec skew(t(), param(), param(), keyword()) :: t()
  def skew(%__MODULE__{ref: ref} = series, xs, ys, opts \\ []) do
    {use_radians, opts} = Keyword.pop(opts, :use_radians, false)
    n = size(series)
    {_, origin, ox, oy} = origin!(opts, n)

    ref
    |> Native.skew_rows(
      param!(xs, n, "xs"),
      param!(ys, n, "ys"),
      origin,
      row_coords(ox, n),
      row_coords(oy, n),
      use_radians == true
    )
    |> wrap()
    |> unwrap!()
  end

  @doc """
  Applies an affine matrix to every coordinate (Shapely order).

  The 6-element form `[a, b, d, e, xoff, yoff]` is `x' = a*x + b*y + xoff`,
  `y' = d*x + e*y + yoff` and leaves Z alone. The 12-element form
  `[a, b, c, d, e, f, g, h, i, xoff, yoff, zoff]` also transforms Z:
  `x' = a*x + b*y + c*z + xoff`, `y' = d*x + e*y + f*z + yoff`, `z' = g*x + h*y + i*z + zoff`.
  A geometry without Z only uses `a, b, d, e, xoff, yoff`. M is dropped (Shapely).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 2)"])
      iex> s |> GeoFrame.GeoSeries.affine_transform([0, 1, 1, 0, 0, 0]) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(2 1)"]
  """
  @spec affine_transform(t(), [number()]) :: t()
  def affine_transform(%__MODULE__{ref: ref}, matrix) when is_list(matrix) do
    if Kernel.length(matrix) not in [6, 12] or not Enum.all?(matrix, &is_number/1) do
      raise ArgumentError,
            "affine_transform needs 6 numbers [a, b, d, e, xoff, yoff] or 12 numbers " <>
              "[a, b, c, d, e, f, g, h, i, xoff, yoff, zoff], got: #{inspect(matrix)}"
    end

    ref |> Native.affine_transform(Enum.map(matrix, &(&1 * 1.0))) |> wrap() |> unwrap!()
  end

  @doc """
  Wraps every x into `[-180, 180)` (so `180` becomes `-180`), like GeoPolars
  `wrap_longitude`. Only coordinates move: geometries are not split at the antimeridian.

  The series must have a geographic CRS or none (then degrees are assumed); a projected CRS
  raises `ArgumentError`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (190 10)"], crs: "EPSG:4326")
      iex> s |> GeoFrame.GeoSeries.wrap_longitude() |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(-170 10)"]
  """
  @spec wrap_longitude(t()) :: t()
  def wrap_longitude(%__MODULE__{ref: ref}),
    do: ref |> Native.wrap_longitude() |> wrap() |> unwrap!()

  @doc """
  Whether the series' CRS is geographic (lon/lat): `true` for `EPSG:4326`, `OGC:CRS84` and any
  other longlat EPSG code, `false` for projected CRSs, `nil` without a CRS or when the CRS
  cannot be resolved.
  """
  @spec is_geographic(t()) :: boolean() | nil
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_geographic(%__MODULE__{ref: ref}), do: Native.geographic(ref)

  # A per-row parameter: one number, or a list / series with one entry per row.
  @typedoc "A number, or a list or `Explorer.Series` with one number (or `nil`) per row."
  @type param :: number() | [number() | nil] | Series.t()

  defp param!(value, n, _name) when is_number(value), do: List.duplicate(value * 1.0, n)

  defp param!(%Series{} = series, n, name), do: series |> Series.to_list() |> param!(n, name)

  defp param!(values, n, name) when is_list(values) do
    if Kernel.length(values) != n do
      raise ArgumentError,
            "#{name} has length #{Kernel.length(values)} but the series has #{n} rows (length mismatch)"
    end

    Enum.map(values, fn
      nil -> nil
      v when is_number(v) -> v * 1.0
      other -> raise ArgumentError, "#{name} must hold numbers or nil, got: #{inspect(other)}"
    end)
  end

  defp param!(other, _n, name) do
    raise ArgumentError,
          "#{name} must be a number, a list or an Explorer.Series, got: #{inspect(other)}"
  end

  # Per-row coordinate lists for the row NIFs; a scalar origin is repeated for every row.
  defp row_coords(coords, _n) when is_list(coords), do: coords

  defp row_coords(scalar, n), do: List.duplicate(scalar, n)

  defp z_factor!(opts, key, default) do
    case Keyword.get(opts, key, default) do
      value when is_number(value) -> value * 1.0
      other -> raise ArgumentError, "#{inspect(key)} must be a number, got: #{inspect(other)}"
    end
  end

  defp origin!(opts, n) do
    opts = Keyword.validate!(opts, origin: :center)

    case opts[:origin] do
      :center ->
        {:scalar, "center", 0.0, 0.0}

      :centroid ->
        {:scalar, "centroid", 0.0, 0.0}

      {x, y} when is_number(x) and is_number(y) ->
        {:scalar, "coord", x * 1.0, y * 1.0}

      origins when is_list(origins) ->
        row_origins!(origins, n)

      other ->
        raise ArgumentError,
              ":origin must be :center, :centroid, {x, y} or a list of {x, y}, got: " <>
                inspect(other)
    end
  end

  defp row_origins!(origins, n) do
    if Kernel.length(origins) != n do
      raise ArgumentError,
            "origin has length #{Kernel.length(origins)} but the series has #{n} rows (length mismatch)"
    end

    pairs =
      Enum.map(origins, fn
        nil ->
          {nil, nil}

        {x, y} when is_number(x) and is_number(y) ->
          {x * 1.0, y * 1.0}

        other ->
          raise ArgumentError, "origin entries must be {x, y} or nil, got: #{inspect(other)}"
      end)

    {:rows, "coord", Enum.map(pairs, &elem(&1, 0)), Enum.map(pairs, &elem(&1, 1))}
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

  # -- linear referencing and linestring operations --------------------------------------

  @doc """
  The point at `distance` along each line.

  `distance` is a number, or a list or `Explorer.Series` with one entry per row (a `nil`
  entry nulls that row). A negative distance counts from the end of the line; distances past
  either end clamp to that end. A `MultiLineString` is measured as its parts laid end to end.
  Rows that are not lines (points, polygons) are `nil`, where Shapely raises.

  Options: `:normalized` - when `true`, `distance` is a fraction of the length (default
  `false`).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0)"])
      iex> s |> GeoFrame.GeoSeries.interpolate(0.25, normalized: true) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(2.5 0)"]
  """
  @spec interpolate(t(), param(), keyword()) :: t()
  def interpolate(%__MODULE__{ref: ref} = series, distance, opts \\ []) do
    opts = Keyword.validate!(opts, normalized: false)

    ref
    |> Native.interpolate(param!(distance, size(series), "distance"), opts[:normalized] == true)
    |> wrap()
    |> unwrap!()
  end

  @doc """
  The distance along each line to the point on it nearest `other` (Shapely `project`).

  `other` is a series of points (positional) or a single point (a WKT string or a one-row
  series, broadcast). With `normalized: true` the result is a fraction of the length
  (default `false`). Rows that are not lines, or whose partner is not a point, are `nil`.
  Pairing and CRS rules are those of `intersects/2`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0)"])
      iex> s |> GeoFrame.GeoSeries.project("POINT (4 3)") |> Explorer.Series.to_list()
      [4.0]
  """
  @spec project(t(), other(), keyword()) :: Series.t()
  def project(%__MODULE__{ref: left} = series, other, opts \\ []) do
    opts = Keyword.validate!(opts, normalized: false)

    left
    |> Native.project(other_ref(series, other), opts[:normalized] == true)
    |> unwrap!()
    |> float_series()
  end

  # -- hulls, bounding shapes, triangulations, sampling and grids ------------------------

  @doc """
  The concave hull of each geometry's vertices (GEOS `ConcaveHull`, as GeoPandas
  `concave_hull`).

  The hull erodes the border of the Delaunay triangulation of the vertices. `ratio` (a number
  from `0.0` to `1.0`, default `0.0`) is the share of the triangulation's edge-length range
  above which border triangles are removed: `0.0` erodes as much as possible and `1.0` is the
  convex hull. With `allow_holes: true` long interior triangles are removed too, which can
  leave holes. One vertex gives a point and two (or collinear) vertices a line.

  Hulls on exactly tied Delaunay choices (cocircular vertices, equal edge lengths) can differ
  from GEOS; see `docs/parity.md`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["MULTIPOINT ((0 0), (4 0), (4 4), (0 4), (2 3))"])
      iex> s |> GeoFrame.GeoSeries.concave_hull(ratio: 1.0) |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list()
      [16.0]
  """
  @spec concave_hull(t(), keyword()) :: t()
  def concave_hull(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, ratio: 0.0, allow_holes: false)

    ref
    |> Native.concave_hull(unit_ratio!(opts[:ratio]), opts[:allow_holes] == true)
    |> wrap()
    |> unwrap!()
  end

  defp unit_ratio!(ratio) when is_number(ratio) and ratio >= 0 and ratio <= 1, do: ratio * 1.0

  defp unit_ratio!(other),
    do: raise(ArgumentError, "ratio must be a number between 0 and 1, got: #{inspect(other)}")

  @doc """
  The minimum-area rotated rectangle around each geometry's vertices. Fewer than three or
  collinear vertices give a point or the line between the extreme vertices.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["MULTIPOINT ((0 0), (4 0), (4 2))"])
      iex> s |> GeoFrame.GeoSeries.minimum_rotated_rectangle() |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list() |> Enum.map(&Float.round(&1, 6))
      [8.0]
  """
  @spec minimum_rotated_rectangle(t()) :: t()
  def minimum_rotated_rectangle(%__MODULE__{ref: ref}) do
    ref |> Native.minimum_rotated_rectangle() |> wrap() |> unwrap!()
  end

  @doc """
  The minimum bounding circle of each geometry as a 32-sided polygon laid out like Shapely's
  (a buffered center, starting east and turning clockwise). A zero radius gives the point. The
  polygon is inscribed in the true circle, so a vertex of the input between two circle
  vertices can lie a hair outside it (use `minimum_bounding_radius/1` for the exact radius).
  """
  @spec minimum_bounding_circle(t()) :: t()
  def minimum_bounding_circle(%__MODULE__{ref: ref}) do
    ref |> Native.minimum_bounding_circle() |> wrap() |> unwrap!()
  end

  @doc """
  The radius of the minimum bounding circle of each geometry (`0.0` for a point; `nil` for null
  and empty rows).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0)"])
      iex> s |> GeoFrame.GeoSeries.minimum_bounding_radius() |> Explorer.Series.to_list()
      [5.0]
  """
  @spec minimum_bounding_radius(t()) :: Series.t()
  def minimum_bounding_radius(%__MODULE__{ref: ref}) do
    ref |> Native.minimum_bounding_radius() |> float_series()
  end

  @doc """
  The largest circle that fits inside each polygon, as the line from its center to the nearest
  boundary point (its length is the radius). The center is found by the cell search of
  JTS/GEOS and lies within `:tolerance` of the optimum; the default is the longer side of the
  bounding box divided by 1000. Rows that are not polygons are `nil`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 8 0, 8 4, 0 4, 0 0))"])
      iex> s |> GeoFrame.GeoSeries.maximum_inscribed_circle(tolerance: 0.001) |> GeoFrame.GeoSeries.length() |> Explorer.Series.to_list() |> Enum.map(&Float.round(&1))
      [2.0]
  """
  @spec maximum_inscribed_circle(t(), keyword()) :: t()
  def maximum_inscribed_circle(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, tolerance: nil)

    tolerance =
      case opts[:tolerance] do
        nil ->
          nil

        t when is_number(t) and t > 0 ->
          t * 1.0

        other ->
          raise ArgumentError, "tolerance must be a positive number, got: #{inspect(other)}"
      end

    ref |> Native.maximum_inscribed_circle(tolerance) |> wrap() |> unwrap!()
  end

  @doc """
  The Delaunay triangulation of the vertices of the whole series, as GeoPandas does: every
  geometry contributes its vertices to one point cloud, and the result has one row per triangle
  (a new series, not aligned with the input). Fewer than three or collinear vertices give an
  empty series.

  Options: `:tolerance` (default `0.0`) merges vertices closer than that; `:only_edges`
  (default `false`) returns one `LineString` per unique edge instead.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (0 0)", "POINT (4 0)", "POINT (0 4)", "POINT (4 4)"])
      iex> s |> GeoFrame.GeoSeries.delaunay_triangles() |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list()
      [8.0, 8.0]
  """
  @spec delaunay_triangles(t(), keyword()) :: t()
  def delaunay_triangles(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, tolerance: 0.0, only_edges: false)

    ref
    |> Native.delaunay_triangles(tolerance!(opts[:tolerance]), opts[:only_edges] == true)
    |> wrap()
    |> unwrap!()
  end

  defp tolerance!(t) when is_number(t) and t >= 0, do: t * 1.0

  defp tolerance!(other),
    do: raise(ArgumentError, "tolerance must be a non-negative number, got: #{inspect(other)}")

  @doc """
  Triangles that fill each polygon (holes respected, no extra vertices), as a
  `GeometryCollection`. Rows that are not polygons give an empty collection. The triangulation
  is constrained Delaunay: where several are valid (cocircular vertices) the choice can differ
  from GEOS.
  """
  @spec constrained_delaunay_triangles(t()) :: t()
  def constrained_delaunay_triangles(%__MODULE__{ref: ref}) do
    ref |> Native.constrained_delaunay_triangles() |> wrap() |> unwrap!()
  end

  @doc """
  The Voronoi diagram of the vertices of the whole series, as GeoPandas does: every geometry
  contributes its vertices to one point cloud, and the result has one polygon per distinct
  vertex, in vertex order (a new series, not aligned with the input). Fewer than two vertices
  give an empty series.

  The cells are clipped to the envelope of the vertices grown by its longer side on every
  side, like GEOS. Options:

    * `:tolerance` - merge vertices closer than this (default `0.0`);
    * `:extend_to` - a geometry (series or WKT) whose envelope the diagram must also cover;
    * `:only_edges` - return one `LineString` per interior cell side instead of the cells;
    * `:ordered` - accepted for Shapely compatibility; cells always come in vertex order.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (0 0)", "POINT (2 0)", "POINT (5 0)"])
      iex> s |> GeoFrame.GeoSeries.voronoi_polygons() |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list()
      [60.0, 25.0, 65.0]
  """
  @spec voronoi_polygons(t(), keyword()) :: t()
  def voronoi_polygons(%__MODULE__{ref: ref} = series, opts \\ []) do
    opts =
      Keyword.validate!(opts, tolerance: 0.0, extend_to: nil, only_edges: false, ordered: false)

    extend_to = if opts[:extend_to], do: other_ref(series, opts[:extend_to])

    ref
    |> Native.voronoi_polygons(
      tolerance!(opts[:tolerance]),
      extend_to,
      opts[:only_edges] == true,
      opts[:ordered] == true
    )
    |> wrap()
    |> unwrap!()
  end

  @doc """
  Random points inside each polygon or along each line, one `MultiPoint` per row.

  `size` is the number of points: one non-negative integer for every row, or a list with one
  integer per row. Options:

    * `:method` - only `:uniform` (the GeoPandas `pointpats` methods are not available);
    * `:seed` - a non-negative integer. The same seed always gives the same points, but not
      NumPy's stream, so samples do not match GeoPandas'. Without a seed the clock decides.

  Points, collections and zero-area polygons give an empty `MultiPoint`; null rows stay null.
  A polygon that is too thin to hit by rejection sampling gives `nil`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"])
      iex> s |> GeoFrame.GeoSeries.sample_points(5, seed: 1) |> GeoFrame.GeoSeries.count_geometries() |> Explorer.Series.to_list()
      [5]
  """
  @spec sample_points(t(), non_neg_integer() | [non_neg_integer()], keyword()) :: t()
  def sample_points(%__MODULE__{ref: ref}, size, opts \\ []) do
    opts = Keyword.validate!(opts, method: :uniform, seed: nil)

    unless opts[:method] == :uniform do
      raise ArgumentError, "method must be :uniform, got: #{inspect(opts[:method])}"
    end

    sizes = sample_sizes!(size)

    seed =
      case opts[:seed] do
        nil ->
          nil

        n when is_integer(n) and n >= 0 and n < 18_446_744_073_709_551_616 ->
          n

        other ->
          raise ArgumentError, "seed must be a non-negative integer, got: #{inspect(other)}"
      end

    ref |> Native.sample_points(sizes, seed) |> wrap() |> unwrap!()
  end

  # Most points one call may draw in total (all rows). Also enforced natively.
  @max_samples 10_000_000

  defp sample_sizes!(size) when is_integer(size) and size >= 0 do
    check_sample_total!(size)
    [size]
  end

  defp sample_sizes!(sizes) when is_list(sizes) and sizes != [] do
    if Enum.all?(sizes, &(is_integer(&1) and &1 >= 0)) do
      check_sample_total!(Enum.sum(sizes))
      sizes
    else
      raise ArgumentError, "size must be non-negative integers, got: #{inspect(sizes)}"
    end
  end

  defp sample_sizes!(other),
    do:
      raise(
        ArgumentError,
        "size must be a non-negative integer or a list of them, got: #{inspect(other)}"
      )

  defp check_sample_total!(total) when total > @max_samples,
    do:
      raise(
        ArgumentError,
        "sample_points: total number of samples (#{total}) exceeds the maximum of #{@max_samples}"
      )

  defp check_sample_total!(_total), do: :ok

  @doc """
  A square or hexagonal grid over the total bounds of the series (GeoPandas `make_grid`),
  with the series' CRS.

  `cell_size` is the side of a square cell, or the distance between opposite sides of a
  hexagon. Options:

    * `:grid_type` - `:square` (default) or `:hexagon`;
    * `:feature_type` - `:polygons` (default), `:centers` or `:corners`;
    * `:offset` - `{x, y}` of the grid origin (default: the lower left corner of the bounds);
    * `:intersect` - keep only the elements that intersect a geometry (default `true`);
    * `:flat_topped` - orient hexagons with a flat top (default: a corner points up).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"])
      iex> s |> GeoFrame.GeoSeries.make_grid(2) |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list()
      [4.0, 4.0, 4.0, 4.0]
  """
  @spec make_grid(t(), number(), keyword()) :: t()
  def make_grid(%__MODULE__{ref: ref}, cell_size, opts \\ []) do
    opts =
      Keyword.validate!(opts,
        grid_type: :square,
        feature_type: :polygons,
        offset: nil,
        intersect: true,
        flat_topped: false
      )

    unless is_number(cell_size) and cell_size > 0 do
      raise ArgumentError, "cell_size must be a positive number, got: #{inspect(cell_size)}"
    end

    grid_type = choice!(opts[:grid_type], [:square, :hexagon], "grid_type")
    feature_type = choice!(opts[:feature_type], [:centers, :corners, :polygons], "feature_type")

    offset =
      case opts[:offset] do
        nil -> nil
        {x, y} when is_number(x) and is_number(y) -> {x * 1.0, y * 1.0}
        other -> raise ArgumentError, "offset must be an {x, y} tuple, got: #{inspect(other)}"
      end

    ref
    |> Native.make_grid(
      cell_size * 1.0,
      Atom.to_string(grid_type),
      Atom.to_string(feature_type),
      offset,
      opts[:intersect] != false,
      opts[:flat_topped] == true
    )
    |> wrap()
    |> unwrap!()
  end

  defp choice!(value, allowed, name) do
    if value in allowed do
      value
    else
      raise ArgumentError, "#{name} must be one of #{inspect(allowed)}, got: #{inspect(value)}"
    end
  end

  @doc """
  Adds vertices so that no segment is longer than `max_segment_length` (a positive number,
  in CRS units; Euclidean). Each segment is split into equal parts. Points are unchanged.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 4 0)"])
      iex> s |> GeoFrame.GeoSeries.segmentize(2) |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(0 0,2 0,4 0)"]
  """
  @spec segmentize(t(), number()) :: t()
  def segmentize(%__MODULE__{ref: ref}, max_segment_length) do
    ref
    |> Native.segmentize(number!(max_segment_length, "max_segment_length"))
    |> wrap()
    |> unwrap!()
  end

  @doc """
  Removes consecutive repeated vertices. With a `tolerance` (default `0.0`: exact repeats
  only) a vertex within that distance of the last kept one is dropped too; the first and last
  vertex of every line and ring are always kept. A ring or line that collapses becomes empty
  (polygon) or keeps its two endpoints (line).
  """
  @spec remove_repeated_points(t(), number()) :: t()
  def remove_repeated_points(%__MODULE__{ref: ref}, tolerance \\ 0.0) do
    ref |> Native.remove_repeated_points(number!(tolerance, "tolerance")) |> wrap() |> unwrap!()
  end

  defp number!(value, _name) when is_number(value), do: value * 1.0

  defp number!(other, name),
    do: raise(ArgumentError, "#{name} must be a number, got: #{inspect(other)}")

  @doc """
  Reverses the vertex order of every line and ring. The parts of a multi-geometry keep their
  order; points are unchanged.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 1 1, 2 0)"])
      iex> s |> GeoFrame.GeoSeries.reverse() |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(2 0,1 1,0 0)"]
  """
  @spec reverse(t()) :: t()
  def reverse(%__MODULE__{ref: ref}), do: ref |> Native.reverse() |> wrap() |> unwrap!()

  @doc """
  Orients polygon rings: counter-clockwise shells and clockwise holes by default, the
  opposite with `exterior_cw: true`. Other types are unchanged.
  """
  @spec orient_polygons(t(), keyword()) :: t()
  def orient_polygons(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, exterior_cw: false)
    ref |> Native.orient_polygons(opts[:exterior_cw] == true) |> wrap() |> unwrap!()
  end

  @doc """
  Whether each geometry is a closed line wound counter-clockwise (Shapely `is_ccw`). Open
  lines and every non-line type are `false`; only null rows are `nil`.
  """
  @spec is_ccw(t()) :: Series.t()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_ccw(%__MODULE__{ref: ref}) do
    ref |> Native.ccw_flags() |> Series.from_list(dtype: :boolean)
  end

  @doc """
  Discrete Hausdorff distance between each pair: the largest distance from a vertex of one
  geometry to the *whole* other one, taken both ways (GEOS, so a vertex that lies on a segment
  of the other geometry counts as distance zero).

  Options: `:densify` - a fraction in `(0, 1]`; each segment is first split into
  `round(1 / densify)` parts so the vertices sample the geometry more finely. Default `nil`
  (vertices only). Pairing, null and CRS rules are those of `intersects/2`.
  """
  @spec hausdorff_distance(t(), other(), keyword()) :: Series.t()
  def hausdorff_distance(%__MODULE__{ref: left} = series, other, opts \\ []) do
    left
    |> Native.hausdorff_distance(other_ref(series, other), densify!(opts))
    |> unwrap!()
    |> float_series()
  end

  @doc """
  Discrete Fréchet distance between each pair of lines (the shortest leash for two walkers
  that never turn back), computed over vertices. Only `LineString` rows have one: every other
  type is `nil`, as are null and empty rows.

  Options: `:densify` as for `hausdorff_distance/3`. Pairing and CRS rules are those of
  `intersects/2`.
  """
  @spec frechet_distance(t(), other(), keyword()) :: Series.t()
  def frechet_distance(%__MODULE__{ref: left} = series, other, opts \\ []) do
    left
    |> Native.frechet_distance(other_ref(series, other), densify!(opts))
    |> unwrap!()
    |> float_series()
  end

  defp densify!(opts) do
    case Keyword.validate!(opts, densify: nil)[:densify] do
      nil -> nil
      value when is_number(value) -> value * 1.0
      other -> raise ArgumentError, ":densify must be a number or nil, got: #{inspect(other)}"
    end
  end

  @doc """
  The `LineString` between the nearest points of each pair, running from the point on the
  left geometry to the point on `other`. Its length equals `distance/2`. Geometries that
  intersect give a zero-length line at a common point. Which of several equally near pairs is
  returned may differ from GEOS. Pairing, null and CRS rules are those of `intersects/2`.

      iex> a = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0)"])
      iex> a |> GeoFrame.GeoSeries.shortest_line("POINT (4 3)") |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(4 0,4 3)"]
  """
  @spec shortest_line(t(), other()) :: t()
  def shortest_line(%__MODULE__{ref: left} = series, other) do
    left |> Native.shortest_line(other_ref(series, other)) |> wrap() |> unwrap!()
  end

  @doc """
  Snaps the vertices and segments of each geometry to the vertices of `other`, within
  `tolerance` (GeoPandas `snap`, GEOS `GeometrySnapper`).

  `other` is a series (paired by position), a one-row series or a WKT string (both broadcast).
  Each snap point in turn moves the nearest vertex of a line or ring that lies closer than
  `tolerance` onto itself, and a snap point closer than `tolerance` to a segment is inserted
  into the nearest one. The result keeps the type of the input; it is not guaranteed to be
  valid. `tolerance` is one number (GeoPandas also accepts an array-like). Rows that are null
  on either side, and rows whose work (vertices times snap points) exceeds 50,000,000, are
  `nil`. The order in which several snap points compete for one vertex follows GEOS in about
  99 % of random cases (see `docs/parity.md`).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0)"])
      iex> s |> GeoFrame.GeoSeries.snap("POINT (5 0.5)", 1) |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(0 0,5 0.5,10 0)"]
  """
  @spec snap(t(), other(), number()) :: t()
  def snap(%__MODULE__{ref: left} = series, other, tolerance) do
    unless is_number(tolerance) do
      raise ArgumentError, "tolerance must be a number, got: #{inspect(tolerance)}"
    end

    left |> Native.snap(other_ref(series, other), tolerance * 1.0) |> wrap() |> unwrap!()
  end

  @doc """
  The paths two linear geometries share (GeoPandas `shared_paths`): per row a
  `GEOMETRYCOLLECTION` of two `MULTILINESTRING`s, first the stretches that run the same way in
  both inputs, then those that run in opposite ways (either can be empty).

  Stretches are split where the overlap starts or ends, at every vertex they pass; a stretch
  met twice is reported once. Both sides must be lines or multi-lines: any other geometry
  (Shapely raises) and null rows give `nil`. `other` is a series, a one-row series or WKT, as
  for `snap/3`.

      iex> a = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 4 0)"])
      iex> a |> GeoFrame.GeoSeries.shared_paths("LINESTRING (4 0, 1 0)") |> GeoFrame.GeoSeries.to_wkt()
      ["GEOMETRYCOLLECTION(MULTILINESTRING EMPTY,MULTILINESTRING((1 0,4 0)))"]
  """
  @spec shared_paths(t(), other()) :: t()
  def shared_paths(%__MODULE__{ref: left} = series, other) do
    left |> Native.shared_paths(other_ref(series, other)) |> wrap() |> unwrap!()
  end

  @doc """
  Polygons formed by the linework of the whole series (GeoPandas `polygonize`).

  All geometries are used together and only their lines and rings count (points are ignored,
  null rows skipped). Lines that do not close a polygon are left out. The result has one row
  per polygon, in no particular order, shells clockwise and holes counter-clockwise like
  GEOS; a polygon lying inside another's hole is a polygon of its own.

  Options:

    * `:node` - `true` (default) merges the polygons of the input, drops lines inside them and
      nodes the linework (splits it wherever lines cross or touch) first; `false` takes every
      line as one edge from its first to its last vertex, so crossings that are not vertices
      of both lines are ignored.
    * `:full` - `true` returns `{polygons, cuts, dangles, invalid_rings}`, four series: the
      polygons, the edges that join two parts of the linework (cut edges), the edges that end
      in a dead end (dangles), and the rings that cross themselves.

  Raises when the noding needs over 50,000,000 segment pairs. Deviation: the collinear
  vertices at which merged input polygons met are dropped (see `docs/parity.md`).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)", "LINESTRING (0 0, 4 4)"])
      iex> s |> GeoFrame.GeoSeries.polygonize() |> GeoFrame.GeoSeries.size()
      2
  """
  @spec polygonize(t(), keyword()) :: t() | {t(), t(), t(), t()}
  def polygonize(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, node: true, full: false)

    {polygons, cuts, dangles, invalid} =
      ref |> Native.polygonize(opts[:node] == true) |> unwrap!()

    polygons = %__MODULE__{ref: polygons}

    if opts[:full] == true do
      {polygons, %__MODULE__{ref: cuts}, %__MODULE__{ref: dangles}, %__MODULE__{ref: invalid}}
    else
      polygons
    end
  end

  @doc """
  The area enclosed by the linework of the whole series, even-odd (GeoPandas `build_area`):
  polygons as `polygonize/2` finds them, but a ring inside a ring becomes a hole, a ring inside
  that an island again, and polygons sharing an edge merge. All geometries are used together;
  the result has one row per polygon (none when the linework encloses nothing).

  Option `:node` as for `polygonize/2`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0, 10 10, 0 10, 0 0)", "LINESTRING (2 2, 4 2, 4 4, 2 4, 2 2)"])
      iex> s |> GeoFrame.GeoSeries.build_area() |> GeoFrame.GeoSeries.area() |> Explorer.Series.to_list()
      [96.0]
  """
  @spec build_area(t(), keyword()) :: t()
  def build_area(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, node: true)
    ref |> Native.build_area(opts[:node] == true) |> wrap() |> unwrap!()
  end

  @doc """
  Whether the polygons of the whole series form a valid coverage (GeoPandas
  `is_valid_coverage`, GEOS `CoverageValidator`; a single boolean, not one per row).

  A polygonal coverage is valid when its polygons do not overlap and the edges they share match
  vertex for vertex. Holes (regions without a polygon) are allowed. Geometries that are not
  polygons or multipolygons, and null rows, are ignored; an empty series is valid.

  Option `:gap_width` (default `0.0`) also rejects gaps narrower than it: a segment that runs
  alongside a neighbour's for more than that length, closer than that width, is invalid. A
  negative width counts as `0.0`. Raises `ArgumentError` when the width is not a finite number,
  or when the check needs over 50,000,000 segment pairs.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 1 1, 1 0, 0 0))", "POLYGON ((0 0, 1 1, 0 1, 0 0))"])
      iex> GeoFrame.GeoSeries.is_valid_coverage(s)
      true
      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 1 1, 1 0, 0 0))", "POLYGON ((0 0, 0.5 0.5, 1 1, 0 1, 0 0))"])
      iex> GeoFrame.GeoSeries.is_valid_coverage(s)
      false
  """
  @spec is_valid_coverage(t(), keyword()) :: boolean()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_valid_coverage(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, gap_width: 0.0)
    ref |> Native.valid_coverage(number!(opts[:gap_width], "gap_width")) |> unwrap!()
  end

  @doc """
  The edges that make the coverage invalid, per row (GeoPandas `invalid_coverage_edges`).

  Each row gets the stretches of its polygon's rings that overlap a neighbour, fail to match a
  neighbour's edge vertex for vertex, or (with `:gap_width`) border a narrow gap: a
  `LINESTRING`, a `MULTILINESTRING` when there are several, or `LINESTRING EMPTY` when the row
  is valid (also for rows that are not polygons). Null rows stay null. Options and limits as
  for `is_valid_coverage/2`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 1 1, 1 0, 0 0))", "POLYGON ((0 0, 0.5 0.5, 1 1, 0 1, 0 0))"])
      iex> s |> GeoFrame.GeoSeries.invalid_coverage_edges() |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(0 0,1 1)", "LINESTRING(0 0,0.5 0.5,1 1)"]
  """
  @spec invalid_coverage_edges(t(), keyword()) :: t()
  def invalid_coverage_edges(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, gap_width: 0.0)

    ref
    |> Native.invalid_coverage_edges(number!(opts[:gap_width], "gap_width"))
    |> wrap()
    |> unwrap!()
  end

  @doc """
  Simplifies the polygons of the whole series as one coverage, keeping it a valid coverage
  (GeoPandas `simplify_coverage`, GEOS `CoverageSimplifier`).

  The edges the polygons share are simplified once, with Visvalingam-Whyatt, so neighbours keep
  matching vertex for vertex; no vertex is removed if that would make an edge cross or touch
  another. `tolerance` is roughly the square root of the area of the triangles removed (in the
  units of the coordinates; a negative value counts as its absolute value). Each polygon is
  rebuilt from its edges, so a ring may start at another vertex (the first node of the coverage)
  and, where a neighbour met a shared edge first, run the other way round.

  Option `:simplify_boundary` (default `true`) - `false` only simplifies the edges two polygons
  share and keeps the outer boundary of the coverage as it is.

  Rows that are null, not polygons or multipolygons, or that hold a NaN or infinite
  coordinate are `nil` (GeoPandas raises); empty polygons stay empty. The input is assumed to be
  a valid coverage (see `is_valid_coverage/2`); an overlapping one is still simplified, but
  the result may not be valid. Raises `ArgumentError` for a tolerance that is not a finite
  number, and when the coverage has over 10,000,000 vertices or its checks visit over
  50,000,000 vertices.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 1 1.1, 2 0, 1.5 1, 2 2, 0 2, 0 0))", "POLYGON ((2 0, 4 0, 4 2, 2 2, 1.5 1, 2 0))"])
      iex> s |> GeoFrame.GeoSeries.simplify_coverage(1) |> GeoFrame.GeoSeries.to_wkt()
      ["POLYGON((2 0,2 2,0 2,2 0))", "POLYGON((2 0,4 0,4 2,2 2,2 0))"]
  """
  @spec simplify_coverage(t(), number(), keyword()) :: t()
  def simplify_coverage(%__MODULE__{ref: ref}, tolerance, opts \\ []) do
    opts = Keyword.validate!(opts, simplify_boundary: true)

    ref
    |> Native.simplify_coverage(
      number!(tolerance, "tolerance"),
      opts[:simplify_boundary] != false
    )
    |> wrap()
    |> unwrap!()
  end

  @max_quad_segs 10_000

  @doc """
  The line parallel to each line at `distance` (`shapely.offset_curve`): positive distances
  offset to the left of the line's direction, negative to the right. The result is a
  `LINESTRING`, a `MULTILINESTRING` when the curve breaks (loops and spirals are cut away
  where the offset comes closer to the line than `distance`) or `LINESTRING EMPTY`.

  `distance` is a number, or a list or `Explorer.Series` with one entry per row (`nil` nulls
  the row). Options (Shapely names):

    * `:quad_segs` - segments per quarter circle of round joins, default `8`; like GEOS, never
      fewer than 8 are drawn. At most `#{@max_quad_segs}`.
    * `:join_style` - `:round` (default), `:mitre` or `:bevel`, for the outside corners.
    * `:mitre_limit` - for `:mitre`, the longest mitre as a multiple of the distance, default
      `5.0`, over 0.

  Lines and multi-lines are offset part by part; a closed line is a line (no join at its
  start). The rings of a polygon are offset as rings, so a positive distance grows it, and are
  cut against each other; collections use their lines and polygons, points give an empty line.
  Deviations from GEOS (see `docs/parity.md`): its input simplification is not applied, the
  mitred outside offset of a closed line keeps the two pieces GEOS drops, and where the offset
  of a polygon ring collapses to a line it is dropped only when it is clearly interior.

  Raises when the vertices times `quad_segs` would exceed 10,000,000 (the raw curve is
  proportional to both); a geometry whose cutting needs over 50,000,000 segment pairs is `nil`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0)"])
      iex> s |> GeoFrame.GeoSeries.offset_curve(1) |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(0 1,10 1)"]
  """
  @spec offset_curve(t(), param(), keyword()) :: t()
  def offset_curve(%__MODULE__{ref: ref} = series, distance, opts \\ []) do
    opts = Keyword.validate!(opts, quad_segs: 8, join_style: :round, mitre_limit: 5.0)
    join = enum!(opts, :join_style, @join_styles)
    quad_segs = opts[:quad_segs]
    mitre_limit = opts[:mitre_limit]

    unless is_integer(quad_segs) and quad_segs > 0 and quad_segs <= @max_quad_segs do
      raise ArgumentError,
            ":quad_segs must be an integer from 1 to #{@max_quad_segs}, got: #{inspect(quad_segs)}"
    end

    unless is_number(mitre_limit) and mitre_limit > 0 do
      raise ArgumentError, ":mitre_limit must be a number over 0, got: #{inspect(mitre_limit)}"
    end

    distances = param!(distance, size(series), "distance")

    ref
    |> Native.offset_curve(distances, quad_segs, Atom.to_string(join), mitre_limit * 1.0)
    |> wrap()
    |> unwrap!()
  end

  @doc """
  The minimum clearance of each geometry (`shapely.minimum_clearance`): the smallest distance
  by which a vertex could move to make the geometry invalid, found as the least distance
  between two vertices at different positions or between a vertex and a segment that does not
  end in it.

  A geometry without such a pair (a point, an empty geometry, one repeated position) reports
  `:infinity`, as Shapely does; null rows give `nil`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0, 5 0.5)", "POINT (1 1)"])
      iex> s |> GeoFrame.GeoSeries.minimum_clearance() |> Explorer.Series.to_list()
      [0.5, :infinity]
  """
  @spec minimum_clearance(t()) :: Series.t()
  def minimum_clearance(%__MODULE__{ref: ref}) do
    # The core answers f64::MAX (GEOS' DBL_MAX) when no clearance exists.
    ref
    |> Native.minimum_clearance()
    |> Enum.map(fn
      value when is_float(value) and value > 1.0e300 -> :infinity
      value -> value
    end)
    |> float_series()
  end

  @doc """
  The line between the two points that realise `minimum_clearance/1`
  (`shapely.minimum_clearance_line`); `LINESTRING EMPTY` when no clearance exists, `nil` for
  null rows. Among equally close pairs the one reported, and the direction of the line, can
  differ from GEOS.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 10 0, 5 0.5)"])
      iex> s |> GeoFrame.GeoSeries.minimum_clearance_line() |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(5 0.5,5 0)"]
  """
  @spec minimum_clearance_line(t()) :: t()
  def minimum_clearance_line(%__MODULE__{ref: ref}) do
    ref |> Native.minimum_clearance_line() |> wrap() |> unwrap!()
  end

  @doc """
  Joins line parts that meet end to end. A point where exactly two parts end is merged
  through; a junction of three or more is not. Returns a `LineString` when everything forms
  one chain, otherwise a `MultiLineString`. A `LineString` comes back unchanged and a row with
  no lines (a point or polygon) becomes `GEOMETRYCOLLECTION EMPTY`, as in Shapely.

  Options: `:directed` - when `true`, parts only join head to tail, never flipped (default
  `false`, where parts may be reversed to fit).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["MULTILINESTRING ((0 0, 1 1), (1 1, 2 2))"])
      iex> s |> GeoFrame.GeoSeries.line_merge() |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(0 0,1 1,2 2)"]
  """
  @spec line_merge(t(), keyword()) :: t()
  def line_merge(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, directed: false)
    ref |> Native.line_merge(opts[:directed] == true) |> wrap() |> unwrap!()
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
  Z coordinate of each point as an f64 series (`GeoSeries.z` in GeoPandas, also available as
  `get_z/1`). Non-point geometries, empty points, points without Z and nulls give `nil`
  (GeoPandas raises for non-points and gives NaN for 2D points).

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT Z (1 2 3)", "POINT (1 2)", nil])
      iex> s |> GeoFrame.GeoSeries.z() |> Explorer.Series.to_list()
      [3.0, nil, nil]
  """
  @spec z(t()) :: Series.t()
  def z(%__MODULE__{ref: ref}), do: ref |> Native.get_z() |> float_series()

  @doc "Alias of `z/1`."
  @spec get_z(t()) :: Series.t()
  def get_z(series), do: z(series)

  @doc """
  M (measure) of each point as an f64 series (`GeoSeries.m` in GeoPandas, also available as
  `get_m/1`); same rules as `z/1`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT M (1 2 4)", "POINT ZM (1 2 3 5)"])
      iex> s |> GeoFrame.GeoSeries.m() |> Explorer.Series.to_list()
      [4.0, 5.0]
  """
  @spec m(t()) :: Series.t()
  def m(%__MODULE__{ref: ref}), do: ref |> Native.get_m() |> float_series()

  @doc "Alias of `m/1`."
  @spec get_m(t()) :: Series.t()
  def get_m(series), do: m(series)

  @doc """
  Whether each geometry has a Z coordinate; `false` for null rows (like Shapely `has_z`).
  An empty geometry keeps the dimension it was declared with (`POINT Z EMPTY` has Z) and a
  collection has Z when any member does.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT Z (1 2 3)", "POINT (1 2)", nil])
      iex> s |> GeoFrame.GeoSeries.has_z() |> Explorer.Series.to_list()
      [true, false, false]
  """
  @spec has_z(t()) :: Series.t()
  def has_z(%__MODULE__{ref: ref}), do: ref |> Native.has_z() |> Series.from_list(dtype: :boolean)

  @doc """
  Whether each geometry has an M coordinate; same rules as `has_z/1`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT M (1 2 4)", "POINT Z (1 2 3)"])
      iex> s |> GeoFrame.GeoSeries.has_m() |> Explorer.Series.to_list()
      [true, false]
  """
  @spec has_m(t()) :: Series.t()
  def has_m(%__MODULE__{ref: ref}), do: ref |> Native.has_m() |> Series.from_list(dtype: :boolean)

  @doc """
  Drops the Z and M coordinates of every geometry. The CRS is kept.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT ZM (1 2 3 4)"])
      iex> s |> GeoFrame.GeoSeries.force_2d() |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(1 2)"]
  """
  @spec force_2d(t()) :: t()
  def force_2d(%__MODULE__{ref: ref}), do: ref |> Native.force_2d() |> wrap() |> unwrap!()

  @doc """
  Gives every geometry without Z a Z coordinate (Shapely `force_3d`). `z` is a number
  (default `0.0`), or a list or `Explorer.Series` with one number per row (`nil` is NaN).

  Like Shapely 2.2, a geometry that already has Z keeps it and loses its M, and a geometry
  with M but no Z is left unchanged.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 2)", "POINT Z (1 2 3)"])
      iex> s |> GeoFrame.GeoSeries.force_3d(9) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT Z(1 2 9)", "POINT Z(1 2 3)"]
  """
  @spec force_3d(t(), param()) :: t()
  def force_3d(%__MODULE__{ref: ref} = series, z \\ 0.0) do
    values = param!(z, size(series), "z")
    ref |> Native.force_3d(values) |> wrap() |> unwrap!()
  end

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

  # -- extraction and parts -------------------------------------------------------------

  @doc """
  Every vertex of every geometry as a dataframe with `x` and `y` (f64), in row order.

  Null and empty geometries contribute no rows. A polygon ring includes its closing vertex,
  so the row count equals the sum of `count_coordinates/1`. Option `:include_index` (default
  `false`) adds a leading `index` column (s64) with the zero-based row position, GeoPandas'
  index. GeoPandas' `ignore_index` and `index_parts` are not supported (there is no
  multi-index).

  Options `:include_z` and `:include_m` (default `false`) append `z` and `m` columns (f64),
  after `x` and `y`, with NaN for a vertex whose geometry lacks that ordinate (Shapely
  `get_coordinates`). An XYM geometry therefore has a NaN `z`.
  """
  @spec get_coordinates(t(), keyword()) :: DataFrame.t()
  def get_coordinates(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, include_index: false, include_z: false, include_m: false)
    {rows, xs, ys, zs, ms} = Native.get_coordinates_zm(ref)

    columns =
      [{"x", xs}, {"y", ys}] ++
        if(opts[:include_z], do: [{"z", nan_for_nil(zs)}], else: []) ++
        if(opts[:include_m], do: [{"m", nan_for_nil(ms)}], else: [])

    dtypes = Enum.map(columns, fn {name, _values} -> {name, {:f, 64}} end)

    if opts[:include_index] do
      DataFrame.new([{"index", rows} | columns], dtypes: [{"index", {:s, 64}} | dtypes])
    else
      DataFrame.new(columns, dtypes: dtypes)
    end
  end

  defp nan_for_nil(values), do: Enum.map(values, &(&1 || :nan))

  @doc """
  Number of parts in each geometry as an s64 series: the members of a multi-part geometry or
  collection, `1` for a single geometry, `0` for empty ones. Null rows are `nil` (Shapely
  counts them as `0`).
  """
  @spec count_geometries(t()) :: Series.t()
  def count_geometries(%__MODULE__{ref: ref}) do
    ref |> Native.count_geometries() |> Series.from_list(dtype: {:s, 64})
  end

  @doc """
  Number of holes in each polygon as an s64 series. Every other geometry type, including
  multipolygons, gives `0` (as Shapely); null rows are `nil`.
  """
  @spec count_interior_rings(t()) :: Series.t()
  def count_interior_rings(%__MODULE__{ref: ref}) do
    ref |> Native.count_interior_rings() |> Series.from_list(dtype: {:s, 64})
  end

  @doc """
  The `n`th part of each geometry (zero-based); a negative `n` counts from the end.

  A single geometry is its own part `0`. Out-of-range indices and null rows give `nil`.
  Only an integer `n` is supported (GeoPandas also accepts an array of indices).
  """
  @spec get_geometry(t(), integer()) :: t()
  def get_geometry(%__MODULE__{ref: ref}, n) when is_integer(n) do
    ref |> Native.get_geometry(n) |> wrap() |> unwrap!()
  end

  @doc """
  The holes of each polygon.

  GeoPandas returns a Python list of rings per row; a series cannot hold lists, so each row
  is a `MULTILINESTRING` of the holes (`MULTILINESTRING EMPTY` for a polygon without holes).
  Every non-polygon row, multipolygons included, and null rows give `nil`.
  """
  @spec interiors(t()) :: t()
  def interiors(%__MODULE__{ref: ref}), do: ref |> Native.interiors() |> wrap() |> unwrap!()

  @doc """
  The distinct vertices of each geometry as a `MULTIPOINT`, in first-seen order.
  """
  @spec extract_unique_points(t()) :: t()
  def extract_unique_points(%__MODULE__{ref: ref}) do
    ref |> Native.extract_unique_points() |> wrap() |> unwrap!()
  end

  @doc """
  The mean of each geometry's vertices as a point (GeoPolars `mean_coordinate`).

  Polygon rings do not count their closing vertex twice; a closed line string does. This is
  a vertex average, not a centroid. Empty and null rows give `nil`.
  """
  @spec mean_coordinate(t()) :: t()
  def mean_coordinate(%__MODULE__{ref: ref}) do
    ref |> Native.mean_coordinate() |> wrap() |> unwrap!()
  end

  @doc """
  Combines all non-null geometries into a **one-row** series (GeoPandas `tools.collect`).

  Geometries of one family (points, lines or polygons) become the matching `Multi*`
  geometry, with multi-part inputs flattened; mixed families become a `GeometryCollection`
  of the inputs. A single geometry is returned as it is unless `multi: true` forces the
  `Multi*` form. No geometries give `GEOMETRYCOLLECTION EMPTY`. The CRS is kept.
  """
  @spec collect(t(), keyword()) :: t()
  def collect(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, multi: false)
    unless is_boolean(opts[:multi]), do: raise(ArgumentError, ":multi must be a boolean")
    ref |> Native.collect(opts[:multi]) |> wrap() |> unwrap!()
  end

  # -- missing values ---------------------------------------------------------------------

  @doc """
  Boolean series: whether each row is a missing (null) geometry. Empty geometries are *not*
  missing (GeoPandas >= 0.6).
  """
  @spec isna(t()) :: Series.t()
  def isna(%__MODULE__{ref: ref}),
    do: ref |> Native.null_rows() |> Series.from_list(dtype: :boolean)

  @doc "Boolean series: the negation of `isna/1`."
  @spec notna(t()) :: Series.t()
  def notna(%__MODULE__{ref: ref}) do
    ref |> Native.null_rows() |> Enum.map(&(not &1)) |> Series.from_list(dtype: :boolean)
  end

  @doc """
  Replaces null rows with `value`: a WKT string, or a one-row `GeoSeries`.

  With no value, nulls become `GEOMETRYCOLLECTION EMPTY`, as in GeoPandas. Empty geometries
  are kept. A WKT value takes the series' CRS; a series value with a different CRS (both
  set) raises `ArgumentError`, as does one that does not have exactly one row.
  """
  @spec fillna(t(), String.t() | t() | nil) :: t()
  def fillna(%__MODULE__{ref: ref} = series, value \\ nil) do
    fill = fill_ref(series, value)
    ref |> Native.fill_null(fill) |> wrap() |> unwrap!()
  end

  defp fill_ref(series, nil), do: fill_ref(series, "GEOMETRYCOLLECTION EMPTY")
  defp fill_ref(_series, %__MODULE__{ref: ref}), do: ref

  defp fill_ref(series, wkt) when is_binary(wkt) do
    %__MODULE__{ref: ref} = from_wkt!([wkt], crs: crs(series))
    ref
  end

  # -- lines and validity -----------------------------------------------------------------

  @doc """
  Whether each geometry is closed, following Shapely: a non-empty line string whose ends
  meet, or a multi-line string all of whose members do. Every other geometry type and empty
  lines give `false`; null rows give `nil`.
  """
  @spec is_closed(t()) :: Series.t()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_closed(%__MODULE__{ref: ref}) do
    ref |> Native.closed_flags() |> then(&Series.from_list(&1, dtype: :boolean))
  end

  @doc """
  Whether each geometry is a ring: a closed line string with at least four vertices and no
  self-intersection except at the closing vertex. Every other geometry type gives `false`;
  null rows give `nil`. The check compares all segment pairs, so it is quadratic in the
  number of vertices.
  """
  @spec is_ring(t()) :: Series.t()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_ring(%__MODULE__{ref: ref}) do
    ref |> Native.ring_flags() |> then(&Series.from_list(&1, dtype: :boolean))
  end

  @doc """
  Whether each geometry is simple, with Shapely's `is_simple` semantics.

  Points are simple; a multipoint is not if a point repeats. A line is simple unless it
  crosses or touches itself anywhere but at the shared vertex of consecutive segments (and,
  for a closed line, at its start); a multi-line also needs its members to meet only at end
  points of both (the end points of a closed line count as interior). A polygon or
  multipolygon is simple when each of its rings is, valid or not. Every collection is `false`,
  as in Shapely. Null rows give `nil`.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (0 0, 4 4, 4 0, 0 4)", "LINESTRING (0 0, 4 4)"])
      iex> s |> GeoFrame.GeoSeries.is_simple() |> Explorer.Series.to_list()
      [false, true]
  """
  @spec is_simple(t()) :: Series.t()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_simple(%__MODULE__{ref: ref}) do
    ref |> Native.simple_flags() |> then(&Series.from_list(&1, dtype: :boolean))
  end

  @doc """
  GeoPolars `self_intersects`: `true` for a line or multi-line that is not simple (see
  `is_simple/1`), `false` for every other geometry type; null rows give `nil`.
  """
  @spec self_intersects(t()) :: Series.t()
  def self_intersects(%__MODULE__{ref: ref}) do
    ref |> Native.self_intersects_flags() |> then(&Series.from_list(&1, dtype: :boolean))
  end

  @doc """
  A string series saying why each geometry is invalid, or `"Valid Geometry"` (Shapely's text)
  for valid ones; null rows give `nil`.

  Only the first problem is reported, in the wording of the `geo` crate (for example
  "interior ring at index 0 is not contained within the polygon's exterior"), not GEOS'
  `"Self-intersection[x y]"`.
  """
  @spec is_valid_reason(t()) :: Series.t()
  # GeoPandas-compatible name, see is_valid/1.
  # credo:disable-for-next-line Credo.Check.Readability.PredicateFunctionNames
  def is_valid_reason(%__MODULE__{ref: ref}) do
    ref |> Native.valid_reasons() |> Series.from_list(dtype: :string)
  end

  @doc "Keeps valid geometries and turns invalid ones into nulls (GeoPolars `null_out_invalid`)."
  @spec null_out_invalid(t()) :: t()
  def null_out_invalid(%__MODULE__{ref: ref}) do
    ref |> Native.null_out_invalid() |> wrap() |> unwrap!()
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
    GeoPandas' `geom_equals`; see `geom_equals_exact/3` and `geom_equals_identical/2` for the
    structural comparisons.
    """,
    contains_properly: """
    Whether `other` lies in the interior of the geometry with no contact with its boundary
    (DE-9IM `T**FF*FF*`). Stricter than `contains/2`: a polygon contains a line that starts
    on its edge but does not contain it properly.
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
  Distance between each pair, as an f64 series. Zero when they intersect.

  Option `:method`:

    * `:planar` (default) - Euclidean, in CRS units (degrees for `EPSG:4326`).
    * `:geodesic` - metres on the WGS 84 ellipsoid. `Point` to `Point` is exact (Karney).
      For any other pair the nearest points are found *planar, in lon/lat degrees* and the
      geodesic distance between them is returned: an approximation that can drift at high
      latitudes or across the antimeridian.
    * `:haversine` - as `:geodesic` on a sphere of radius 6 371 008.8 m.

  #{@geodesic_note}

  Pairing, null and CRS rules are those of `intersects/2`.
  """
  @spec distance(t(), other(), keyword()) :: Series.t()
  def distance(%__MODULE__{ref: left} = series, other, opts \\ []) do
    right = other_ref(series, other)

    case measure_method!(opts, [:planar, :geodesic, :haversine]) do
      :planar -> left |> Native.distance(right) |> unwrap!() |> float_series()
      :geodesic -> left |> Native.distance_geodesic(right, false) |> unwrap!() |> float_series()
      :haversine -> left |> Native.distance_geodesic(right, true) |> unwrap!() |> float_series()
    end
  end

  @doc """
  Whether each pair is within `limit` of each other: `distance(a, b) <= limit` (inclusive).

  Option `:method` is that of `distance/3`; with `:geodesic` or `:haversine`, `limit` is in
  metres. Pairing, null and CRS rules are those of `intersects/2`.
  """
  @spec dwithin(t(), other(), number(), keyword()) :: Series.t()
  def dwithin(%__MODULE__{ref: left} = series, other, limit, opts \\ []) when is_number(limit) do
    case measure_method!(opts, [:planar, :geodesic, :haversine]) do
      :planar ->
        left |> Native.dwithin(other_ref(series, other), limit * 1.0) |> boolean_series()

      _geodesic ->
        series
        |> distance(other, opts)
        |> Series.less_equal(limit * 1.0)
    end
  end

  @doc """
  All pairs of rows `{i, j}` for which `left[i].predicate(right[j])` holds, found with an
  R-tree built over the bounding boxes of `right` and refined with the exact predicate.

  Returns `{left_indices, right_indices}`: two parallel lists of zero-based row positions,
  ordered by left then right position. (Plain lists rather than series: they are consumed as
  `take/2` indices, and the pair count is data-dependent.) The result equals evaluating the
  predicate over every pair, only faster.

  `predicate` is one of `:intersects` (default), `:within`, `:contains`, `:touches`,
  `:crosses`, `:overlaps`, `:covers`, `:covered_by`, `:geom_equals`, `:contains_properly` or
  `:dwithin` (see `sindex_valid_query_predicates/0`). The
  predicate reads `left.predicate(right)`: `:within` means the left geometry is within the
  right one. `:disjoint` is rejected because a spatial index cannot answer it.

  Options: `:distance` - required, non-negative, in CRS units for `:dwithin`.

  Null and empty rows never match. Differing CRS (both set) raise `ArgumentError`, as do
  unknown predicates and a missing distance. Querying is single-threaded.
  """
  @spec sindex_query(t(), t(), atom(), keyword()) :: {[non_neg_integer()], [non_neg_integer()]}
  def sindex_query(left, right, predicate \\ :intersects, opts \\ [])

  def sindex_query(%__MODULE__{} = left, %__MODULE__{} = right, predicate, opts)
      when is_atom(predicate) and is_list(opts) do
    {left_rows, right_rows} = sindex_query_binaries(left, right, predicate, opts)
    {u32_list(left_rows), u32_list(right_rows)}
  end

  # The pairs as native-endian u32 binaries, the form the joins consume (no list of millions
  # of integers on the BEAM heap).
  @doc false
  def sindex_query_binaries(%__MODULE__{ref: left}, %__MODULE__{ref: right}, predicate, opts)
      when is_atom(predicate) and is_list(opts) do
    GeoFrame.Options.validate!(opts, [:distance])
    distance = opts |> Keyword.get(:distance) |> then(&(&1 && &1 * 1.0))

    left
    |> Native.sindex_query(right, Atom.to_string(predicate), distance)
    |> unwrap!()
  end

  defp u32_list(binary), do: for(<<row::unsigned-native-32 <- binary>>, do: row)

  @doc """
  For each left geometry, the nearest right geometries by planar euclidean distance.

  Returns `{left_idx, right_idx, distances}` as parallel lists ordered by left row then
  right row. The distance is geometry to geometry (not centroids), so a point inside a
  polygon is at distance `0.0`. Every right geometry at exactly the minimum distance is
  returned (ties), like GeoPandas. Computed with a best-first R-tree walk, then exact
  distances.

  Options:

    * `:max_distance` - non-negative; left rows whose nearest geometry is farther have no
      match (the limit itself is inclusive).
    * `:exclusive` - when `true`, right geometries equal to the left one (topologically) are
      ignored, so a frame joined with itself finds each row's nearest *other* row.
      Default `false`.

  Null and empty rows never match. Differing CRS (both set) raise `ArgumentError`.
  """
  @spec sindex_nearest(t(), t(), keyword()) ::
          {[non_neg_integer()], [non_neg_integer()], [float()]}
  def sindex_nearest(left, right, opts \\ [])

  def sindex_nearest(%__MODULE__{} = left, %__MODULE__{} = right, opts) when is_list(opts) do
    {left_rows, right_rows, distances} = sindex_nearest_binaries(left, right, opts)

    {u32_list(left_rows), u32_list(right_rows),
     distances |> Series.from_binary({:f, 64}) |> Series.to_list()}
  end

  # The `{left, right, distance}` triples as native-endian binaries (u32, u32, f64).
  @doc false
  def sindex_nearest_binaries(%__MODULE__{ref: left}, %__MODULE__{ref: right}, opts)
      when is_list(opts) do
    GeoFrame.Options.validate!(opts, [:max_distance, :exclusive])
    max_distance = opts |> Keyword.get(:max_distance) |> then(&(&1 && &1 * 1.0))
    exclusive = Keyword.get(opts, :exclusive, false)

    unless is_boolean(exclusive) do
      raise ArgumentError, "exclusive must be a boolean, got #{inspect(exclusive)}"
    end

    left
    |> Native.sindex_nearest(right, max_distance, exclusive)
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

  @doc """
  Whether each pair's DE-9IM matrix matches `pattern`, like Shapely `relate_pattern`.

  `pattern` is nine characters (case-insensitive) from `T` (any intersection), `F` (none),
  `*` (anything), or `0`, `1`, `2` (exactly that dimension). For example `"FF*F1****"`
  matches polygons that touch along an edge. Anything else raises `ArgumentError`.
  Pairing, null and CRS rules are those of `intersects/2`.

  ## Example

      iex> a = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"])
      iex> b = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((4 0, 8 0, 8 4, 4 4, 4 0))"])
      iex> GeoFrame.GeoSeries.relate_pattern(a, b, "FF*F1****") |> Explorer.Series.to_list()
      [true]
  """
  @spec relate_pattern(t(), other(), String.t()) :: Series.t()
  def relate_pattern(%__MODULE__{ref: left} = series, other, pattern) when is_binary(pattern) do
    left |> Native.relate_pattern(other_ref(series, other), pattern) |> boolean_series()
  end

  @doc """
  Structural equality: same geometry type and structure, with every vertex in the same order
  and within `tolerance` (planar distance, inclusive) of its counterpart. Shapely
  `equals_exact`.

  Unlike `geom_equals/2` a reversed ring or a different start vertex is a difference, and NaN
  never equals NaN. `tolerance` must be a finite, non-negative number. Pairing, null and CRS
  rules are those of `intersects/2`.

  ## Example

      iex> a = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)"])
      iex> b = GeoFrame.GeoSeries.from_wkt!(["POINT (1.0004 1)"])
      iex> GeoFrame.GeoSeries.geom_equals_exact(a, b, 0.001) |> Explorer.Series.to_list()
      [true]
  """
  @spec geom_equals_exact(t(), other(), number()) :: Series.t()
  def geom_equals_exact(%__MODULE__{ref: left} = series, other, tolerance)
      when is_number(tolerance) do
    left |> Native.equals_exact(other_ref(series, other), tolerance * 1.0) |> boolean_series()
  end

  @doc """
  Exact equality of structure and coordinates, without tolerance; NaN coordinates are equal
  to each other. Shapely `equals_identical`. Pairing, null and CRS rules are those of
  `intersects/2`.
  """
  @spec geom_equals_identical(t(), other()) :: Series.t()
  def geom_equals_identical(%__MODULE__{ref: left} = series, other) do
    left |> Native.equals_identical(other_ref(series, other)) |> boolean_series()
  end

  @valid_query_predicates ~w(intersects within contains touches crosses overlaps covers
                             covered_by geom_equals contains_properly dwithin)a

  @doc """
  The predicate names `sindex_query/4` (and `GeoFrame.GeoDataFrame.sjoin/3`) accept, like
  GeoPandas `sindex.valid_query_predicates` (`:disjoint` is excluded, and GeoPandas' `nil`
  default is spelled `:intersects`).
  """
  @spec sindex_valid_query_predicates() :: [atom()]
  def sindex_valid_query_predicates, do: @valid_query_predicates

  @doc """
  Zero-based positions of the rows whose bounding box intersects `{minx, miny, maxx, maxy}`
  (edges inclusive), in row order. Like GeoPandas `sindex.intersection(bounds)`: it is an
  envelope test, not an exact intersection (see `cx/5` for that). Null and empty rows never
  match. Raises `ArgumentError` for an inverted box. A linear scan over the bounds.
  """
  @spec sindex_intersection(t(), {number(), number(), number(), number()}) ::
          [non_neg_integer()]
  def sindex_intersection(%__MODULE__{ref: ref}, {minx, miny, maxx, maxy} = bbox)
      when is_number(minx) and is_number(miny) and is_number(maxx) and is_number(maxy) do
    ref
    |> Native.bbox_intersection(
      bbox
      |> Tuple.to_list()
      |> Enum.map(&(&1 * 1.0))
      |> List.to_tuple()
    )
    |> unwrap!()
  end

  @doc """
  Rows whose geometry intersects the box `xmin..xmax` by `ymin..ymax`, like GeoPandas
  `s.cx[xmin:xmax, ymin:ymax]`.

  A `nil` bound is unbounded: it takes the matching side of `total_bounds/1`. The test is the
  exact `intersects/2` against the box polygon (touching the edge counts). Returns a
  `GeoSeries` of the selected rows in their original order; null rows are dropped.

  ## Example

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)", "POINT (9 9)"])
      iex> GeoFrame.GeoSeries.cx(s, 0, 0, 2, 2) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(1 1)"]
  """
  @spec cx(t(), number() | nil, number() | nil, number() | nil, number() | nil) :: t()
  def cx(%__MODULE__{} = series, xmin, ymin, xmax, ymax) do
    take(series, cx_positions(series, xmin, ymin, xmax, ymax))
  end

  @doc """
  The zero-based row positions `cx/5` selects, for aligning other data with the selection.
  """
  @spec cx_positions(t(), number() | nil, number() | nil, number() | nil, number() | nil) ::
          [non_neg_integer()]
  def cx_positions(%__MODULE__{} = series, xmin, ymin, xmax, ymax) do
    {bx0, by0, bx1, by1} = total_bounds(series) || {nil, nil, nil, nil}
    box = {xmin || bx0, ymin || by0, xmax || bx1, ymax || by1}

    cond do
      # no non-empty geometry to take the unbounded sides from, so nothing can intersect
      Tuple.to_list(box) |> Enum.any?(&is_nil/1) ->
        []

      not valid_box?(box) ->
        raise ArgumentError,
              "invalid box: expected xmin <= xmax and ymin <= ymax, got " <>
                inspect({xmin, ymin, xmax, ymax})

      true ->
        positions_intersecting(series, box)
    end
  end

  defp valid_box?({x0, y0, x1, y1}), do: x0 <= x1 and y0 <= y1

  defp positions_intersecting(series, {x0, y0, x1, y1}) do
    series
    |> intersects(box_wkt(x0, y0, x1, y1))
    |> Series.to_list()
    |> Enum.with_index()
    |> Enum.flat_map(fn
      {true, index} -> [index]
      _ -> []
    end)
  end

  defp box_wkt(x0, y0, x1, y1) do
    [a, b, c, d] = Enum.map([x0, y0, x1, y1], &wkt_number/1)
    "POLYGON ((#{a} #{b}, #{c} #{b}, #{c} #{d}, #{a} #{d}, #{a} #{b}))"
  end

  defp wkt_number(value),
    do: :erlang.float_to_binary(value * 1.0, [{:decimals, 17}, :compact])

  @doc """
  Appends the rows of several series in order, like `pandas.concat` on GeoSeries.

  Every series must have the same CRS (both unset counts as equal); a mismatch or an empty
  list raises `ArgumentError`.
  """
  @spec concat([t()]) :: t()
  def concat([_ | _] = series) do
    unless Enum.all?(series, &match?(%__MODULE__{}, &1)) do
      raise ArgumentError, "concat expects only GeoSeries, got: #{inspect(series)}"
    end

    series
    |> Enum.map(& &1.ref)
    |> Native.concat()
    |> wrap()
    |> unwrap!()
  end

  def concat(other),
    do:
      raise(ArgumentError, "concat expects a non-empty list of GeoSeries, got: #{inspect(other)}")

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

    Every combination of points, lines and polygons is supported, with GEOS results: lines are
    noded (crossings, overlaps), the lower-dimensional parts of a result are kept (two squares
    sharing an edge intersect in that `LINESTRING`, sharing a corner in a `POINT`), and a result
    of several dimensions is a `GEOMETRYCOLLECTION` with one member per part. A
    `GeometryCollection` input is supported by `intersection` only (member by member); the other
    operations give `nil` for it. Empty results are empty geometries
    (`POLYGON EMPTY`, `LINESTRING EMPTY`, `MULTIPOINT EMPTY`), not `nil`.
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

  Polygons are merged with a single `unary_union`, lines are noded and overlaps merged, and
  points are deduplicated. A mix of families gives a `GEOMETRYCOLLECTION` in which lines inside
  a polygon and points on a polygon or line are absorbed. A `GeometryCollection` row gives a
  `nil` row; no non-null input gives `GEOMETRYCOLLECTION EMPTY`.
  """
  @spec union_all(t()) :: t()
  def union_all(%__MODULE__{ref: ref}) do
    {:ok, result} = ref |> Native.union_all() |> wrap()
    result
  end

  @doc """
  Grouped `union_all/1`: `group_ids` assigns each row to a group in `0..n_groups-1`, and the
  result has one row per group (same CRS).

  Null rows are skipped; a group without non-null rows gives `GEOMETRYCOLLECTION EMPTY`, and
  a group containing a `GeometryCollection` row gives a null row, as `union_all/1` does. Raises
  `ArgumentError` when `group_ids` has the wrong length or an id is out of range.
  """
  @spec union_groups(t(), [non_neg_integer()], non_neg_integer()) :: t()
  def union_groups(%__MODULE__{ref: ref}, group_ids, n_groups)
      when is_list(group_ids) and is_integer(n_groups) and n_groups >= 0 do
    if n_groups > Kernel.length(group_ids) do
      raise ArgumentError,
            "n_groups (#{n_groups}) cannot exceed the number of group ids (#{Kernel.length(group_ids)})"
    end

    if Enum.any?(group_ids, &(is_integer(&1) and &1 >= n_groups)) do
      raise ArgumentError, "group id out of range for #{n_groups} groups"
    end

    ref |> Native.union_groups(group_ids, n_groups) |> wrap() |> unwrap!()
  end

  @doc """
  The intersection of all non-null geometries as a **one-row** GeoSeries (GeoPandas
  `intersection_all`, which returns a scalar).

  Rows are folded left to right with `intersection/2`, so the lower-dimensional parts of
  touching geometries survive (two squares sharing an edge give that edge). A step whose result
  is a `GEOMETRYCOLLECTION` (mixed dimensions) cannot be intersected further and gives a `nil`
  row. No non-null input gives `GEOMETRYCOLLECTION EMPTY`.
  """
  @spec intersection_all(t()) :: t()
  def intersection_all(%__MODULE__{ref: ref}) do
    ref |> Native.intersection_all() |> wrap() |> unwrap!()
  end

  @doc """
  Splits multi-part geometries and collections into their parts, like `GeoSeries.explode`.

  Returns `{parts, positions}`: a `GeoSeries` with one row per part and the parallel list of
  zero-based **source row positions** (what the pandas index holds in GeoPandas). Multi-part
  geometries give their members and collections their direct members (one level, as
  `shapely.get_parts`; nested collections do not exist because storage flattens them). An
  atomic geometry, even an empty polygon or line, is its own single part. Null rows and empty
  multi-geometries or collections have no parts and disappear. The CRS is kept.

  Option `index_parts: true` returns `{parts, positions, part_numbers}` where `part_numbers`
  holds the zero-based number of each part within its source row (the second level of the
  pandas MultiIndex).

  ## Example

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (0 0)", "MULTIPOINT ((1 1), (2 2))"])
      iex> {parts, positions} = GeoFrame.GeoSeries.explode(s)
      iex> {GeoFrame.GeoSeries.to_wkt(parts), positions}
      {["POINT(0 0)", "POINT(1 1)", "POINT(2 2)"], [0, 1, 1]}
  """
  @spec explode(t(), keyword()) ::
          {t(), [non_neg_integer()]} | {t(), [non_neg_integer()], [non_neg_integer()]}
  def explode(%__MODULE__{ref: ref}, opts \\ []) do
    opts = Keyword.validate!(opts, index_parts: false)
    {parts_ref, positions} = ref |> Native.explode() |> unwrap!()
    parts = %__MODULE__{ref: parts_ref}

    if opts[:index_parts],
      do: {parts, positions, part_numbers(positions)},
      else: {parts, positions}
  end

  defp part_numbers(positions) do
    {numbers, _} =
      Enum.map_reduce(positions, {nil, 0}, fn
        position, {position, n} -> {n + 1, {position, n + 1}}
        position, _ -> {0, {position, 0}}
      end)

    numbers
  end

  @doc """
  Shapely's `clip_by_rect`: every geometry clipped to the rectangle, row by row.

  This is the fast rectangle clip of GEOS. Only what reaches into the interior of the
  rectangle survives: a point or line touching the edge, or a polygon sharing only an edge with
  the rectangle, gives `GEOMETRYCOLLECTION EMPTY`. Empty results are **kept** (use
  `GeoFrame.clip/3` to drop them) and the result of a polygon is not guaranteed valid. Null and
  `GeometryCollection` rows give `nil`. Raises `ArgumentError` for an inverted rectangle.
  """
  @spec clip_by_rect(t(), number(), number(), number(), number()) :: t()
  def clip_by_rect(%__MODULE__{ref: ref}, xmin, ymin, xmax, ymax)
      when is_number(xmin) and is_number(ymin) and is_number(xmax) and is_number(ymax) do
    ref
    |> Native.clip_by_rect(xmin * 1.0, ymin * 1.0, xmax * 1.0, ymax * 1.0)
    |> wrap()
    |> unwrap!()
  end

  @doc """
  The canonical form of every geometry, as GEOS (`shapely.normalize`).

  Polygon shells are clockwise and holes counter-clockwise, each ring starts at its smallest
  coordinate, holes are sorted in descending order; an open line runs from its smaller end
  and a closed line is normalised like a ring; the parts of multi-geometries and collections
  are normalised and sorted in descending order. Null rows stay null; the CRS is kept.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["LINESTRING (3 3, 1 1)"])
      iex> s |> GeoFrame.GeoSeries.normalize() |> GeoFrame.GeoSeries.to_wkt()
      ["LINESTRING(1 1,3 3)"]
  """
  @spec normalize(t()) :: t()
  def normalize(%__MODULE__{ref: ref}), do: ref |> Native.normalize() |> wrap() |> unwrap!()

  @precision_modes [:valid_output, :pointwise, :keep_collapsed]

  @doc """
  Snaps every coordinate to a grid of `grid_size`, like `shapely.set_precision`.

  Options: `:mode` - `:valid_output` (default), `:pointwise` or `:keep_collapsed`.

    * `:pointwise` only rounds the coordinates (`floor(x / grid + 0.5) * grid`, halves round
      up also for negative numbers) and may leave invalid or degenerate geometry.
    * `:valid_output` also removes repeated vertices and collapsed parts (a line left with one
      point, a polygon or hole without area) and repairs a polygon that snapping made invalid.
    * `:keep_collapsed` is `:valid_output`, except that a line collapsed to a point stays a
      zero-length line.

  `grid_size` `0` returns the geometry unchanged with floating precision; the sign is ignored.
  The precision is stored on the series (`get_precision/1`), not in the GeoArrow data, so only
  the result of `set_precision/3` carries it: any other operation returns floating precision.

  Deviations from GEOS' snap-rounding overlay: lines are not noded (crossings between snapped
  segments get no vertex), repaired polygons can differ in vertex order and start, and an
  invalid input polygon is repaired instead of raising.

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POINT (1.234 5.678)"])
      iex> s |> GeoFrame.GeoSeries.set_precision(0.1) |> GeoFrame.GeoSeries.to_wkt()
      ["POINT(1.2 5.7)"]
  """
  @spec set_precision(t(), number(), keyword()) :: t()
  def set_precision(%__MODULE__{ref: ref}, grid_size, opts \\ []) do
    opts = Keyword.validate!(opts, mode: :valid_output)

    unless is_number(grid_size) do
      raise ArgumentError, "grid_size must be a number, got: #{inspect(grid_size)}"
    end

    unless opts[:mode] in @precision_modes do
      raise ArgumentError,
            "mode must be one of #{inspect(@precision_modes)}, got: #{inspect(opts[:mode])}"
    end

    grid = abs(grid_size) * 1.0
    result = ref |> Native.set_precision(grid, Atom.to_string(opts[:mode])) |> wrap() |> unwrap!()
    %{result | precision: grid}
  end

  @doc """
  The grid size of each geometry (`shapely.get_precision`): the value given to
  `set_precision/3`, or `0.0` for floating precision; `nil` for null rows.

  GeoArrow carries no precision, so it lives on the series: only the result of
  `set_precision/3` has a non-zero precision.
  """
  @spec get_precision(t()) :: Series.t()
  def get_precision(%__MODULE__{ref: ref, precision: precision}) do
    ref
    |> Native.null_rows()
    |> Enum.map(fn null? -> if null?, do: nil, else: precision end)
    |> float_series()
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
    # Take the rows first: serialising the whole column to WKT just to show five is O(n).
    case min(size(series), @preview_rows) do
      0 -> []
      n -> series |> take(Enum.to_list(0..(n - 1))) |> to_wkt()
    end
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
