defmodule GeoFrame.Reference do
  @moduledoc false
  # Engine of the differential reference suite (test/geo_frame/reference_test.exs).
  #
  # Fixtures in test/reference/fixtures/*.json are produced by test/reference/generate.py from
  # GeoPandas/Shapely. Each fixture holds named row sets and groups (one group = one operation
  # with fixed arguments); `run/0` evaluates every row with GeoFrame and returns one result per
  # case: `:agree` or `:differ`.

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.Dissolve
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.SpatialJoin

  @fixtures ~w(measures unary_predicates binary_predicates constructive constructive_e set_ops
               affine linear_referencing crs geodesic io parity_i parity_j zm zm_ops)
  @dir Path.expand("../reference/fixtures", __DIR__)
  # GEOS' DBL_MAX, which stands for "no clearance" in the fixtures.
  @dbl_max 1.797_693_134_862_315_7e308

  @type result :: %{
          file: String.t(),
          op: String.t(),
          label: String.t(),
          id: String.t(),
          status: :agree | :differ,
          nullish: boolean(),
          detail: term()
        }

  def corpus do
    @dir |> Path.join("corpus.json") |> File.read!() |> Jason.decode!() |> Map.fetch!("corpus")
  end

  def meta do
    @dir |> Path.join("corpus.json") |> File.read!() |> Jason.decode!() |> Map.fetch!("meta")
  end

  def read(name), do: @dir |> Path.join("#{name}.json") |> File.read!() |> Jason.decode!()

  @doc "Evaluates every series fixture and returns the list of per-case results."
  @spec run() :: [result()]
  def run do
    corpus = corpus()

    Enum.flat_map(@fixtures, fn name ->
      doc = read(name)
      Enum.flat_map(doc["groups"], &run_group(name, &1, doc["sets"], corpus))
    end)
  end

  # -- groups ----------------------------------------------------------------------------

  defp run_group(file, group, sets, corpus) do
    rows = Map.fetch!(sets, group["set"])
    crs = group["crs"]
    a_ids = Enum.map(rows, &Enum.at(&1, 0))
    b_ids = Enum.map(rows, &Enum.at(&1, 1))
    has_b = Enum.any?(b_ids, & &1)
    expected = group["e"]

    actual = actuals(group, a_ids, b_ids, has_b, corpus, crs)

    for {{a, b, e}, act} <- Enum.zip([Enum.zip([a_ids, b_ids, expected]), actual]) do
      id = if b, do: "#{group["label"]}:#{a}|#{b}", else: "#{group["label"]}:#{a}"
      nullish = nullish?(corpus, a) or (b != nil and nullish?(corpus, b))
      {status, detail} = compare(group, act, e, {corpus[a], b && corpus[b]}, crs)

      %{
        file: file,
        op: group["op"],
        label: group["label"],
        id: id,
        status: status,
        nullish: nullish,
        detail: detail
      }
    end
    |> Enum.reject(&(&1.status == :skip))
  end

  defp nullish?(corpus, id), do: is_nil(corpus[id]) or String.contains?(corpus[id], "EMPTY")

  defp series(ids, corpus, crs), do: GeoSeries.from_wkt!(Enum.map(ids, &corpus[&1]), crs: crs)

  # Vectorised first; if GeoFrame raises, isolate row by row.
  defp actuals(group, a_ids, b_ids, has_b, corpus, crs) do
    if group["per_row"] do
      Enum.map(Enum.zip(a_ids, b_ids), fn {a, b} ->
        row_actual(group, a, b, has_b, corpus, crs)
      end)
    else
      try do
        a = series(a_ids, corpus, crs)
        b = if has_b, do: series(b_ids, corpus, crs)
        group |> apply_op(a, b) |> split_rows(group["kind"])
      rescue
        _ ->
          Enum.map(Enum.zip(a_ids, b_ids), fn {a, b} ->
            row_actual(group, a, b, has_b, corpus, crs)
          end)
      end
    end
  end

  defp row_actual(group, a, b, has_b, corpus, crs) do
    sa = series([a], corpus, crs)
    sb = if has_b, do: series([b], corpus, crs)

    group
    |> apply_op(sa, sb)
    |> split_rows(group["kind"])
    |> hd()
  rescue
    e -> {:raised, Exception.message(e)}
  end

  # Turns the raw result of an op into one value per row.
  defp split_rows(%Series{} = s, _), do: Series.to_list(s)
  defp split_rows(%GeoSeries{} = g, _), do: GeoSeries.to_wkt(g)
  defp split_rows(%Explorer.DataFrame{} = df, "floats"), do: bounds_rows(df)
  defp split_rows(list, _) when is_list(list), do: list
  defp split_rows(nil, "floats"), do: [nil]
  defp split_rows(tuple, "floats") when is_tuple(tuple), do: [Tuple.to_list(tuple)]
  defp split_rows(other, _), do: [other]

  defp bounds_rows(df) do
    cols = for c <- ~w(minx miny maxx maxy), do: Series.to_list(df[c])

    cols
    |> Enum.zip()
    |> Enum.map(fn row ->
      list = Tuple.to_list(row)
      if Enum.any?(list, &is_nil/1), do: nil, else: list
    end)
  end

  # -- operations -------------------------------------------------------------------------

  @unary_ops ~w(area length geom_type count_coordinates count_geometries count_interior_rings x y
                is_valid is_empty is_ring is_closed is_ccw is_valid_reason isna bounds total_bounds
                area_geodesic length_geodesic centroid convex_hull envelope boundary exterior
                extract_unique_points reverse representative_point make_valid line_merge interiors
                is_simple normalize has_z has_m z m)
  @binary_ops ~w(distance relate shortest_line intersects contains within touches crosses overlaps
                 disjoint covers covered_by contains_properly geom_equals geom_equals_identical
                 intersection union difference symmetric_difference)

  defp apply_op(group, a, b) do
    op = group["op"]

    cond do
      op in @unary_ops -> apply(GeoSeries, String.to_atom(op), [a])
      op in @binary_ops -> apply(GeoSeries, String.to_atom(op), [a, b])
      true -> apply_with_args(op, group["args"], a, b)
    end
  end

  defp apply_with_args("distance_geodesic", _, a, b),
    do: GeoSeries.distance(a, b, method: :geodesic)

  defp apply_with_args("hausdorff_distance", args, a, b),
    do: GeoSeries.hausdorff_distance(a, b, densify_opts(args))

  defp apply_with_args("frechet_distance", args, a, b),
    do: GeoSeries.frechet_distance(a, b, densify_opts(args))

  defp apply_with_args("geom_equals_exact", args, a, b),
    do: GeoSeries.geom_equals_exact(a, b, args["tolerance"])

  defp apply_with_args("relate_pattern", args, a, b),
    do: GeoSeries.relate_pattern(a, b, args["pattern"])

  defp apply_with_args("dwithin", args, a, b), do: GeoSeries.dwithin(a, b, args["distance"])
  defp apply_with_args(op, args, a, b), do: apply_param(op, args, a, b)

  defp apply_param("buffer", args, a, _b) do
    opts =
      [
        cap_style: args["cap_style"] && String.to_atom(args["cap_style"]),
        join_style: args["join_style"] && String.to_atom(args["join_style"]),
        quad_segs: args["quad_segs"]
      ]
      |> Enum.reject(fn {_, v} -> is_nil(v) end)

    GeoSeries.buffer(a, args["distance"], opts)
  end

  defp apply_param("simplify", args, a, _b) do
    opts = if args["preserve_topology"] == false, do: [preserve_topology: false], else: []
    GeoSeries.simplify(a, args["tolerance"], opts)
  end

  defp apply_param("segmentize", args, a, _),
    do: GeoSeries.segmentize(a, args["max_segment_length"])

  defp apply_param("remove_repeated_points", args, a, _),
    do: GeoSeries.remove_repeated_points(a, args["tolerance"])

  defp apply_param("orient_polygons", args, a, _),
    do: GeoSeries.orient_polygons(a, exterior_cw: args["exterior_cw"])

  defp apply_param("clip_by_rect", args, a, _) do
    [xmin, ymin, xmax, ymax] = args["rect"]
    GeoSeries.clip_by_rect(a, xmin, ymin, xmax, ymax)
  end

  defp apply_param("get_geometry", args, a, _), do: GeoSeries.get_geometry(a, args["n"])
  defp apply_param("shortest_line", _, a, b), do: GeoSeries.shortest_line(a, b)

  defp apply_param("translate", args, a, _),
    do: GeoSeries.translate(a, args["x"], args["y"], zoff: args["z"] || 0)

  defp apply_param("scale", args, a, _),
    do:
      GeoSeries.scale(a, args["x"], args["y"],
        origin: origin(args["origin"]),
        zfact: args["zfact"] || 1
      )

  defp apply_param("rotate", args, a, _),
    do: GeoSeries.rotate(a, args["angle"], origin: origin(args["origin"]))

  defp apply_param("skew", args, a, _),
    do: GeoSeries.skew(a, args["xs"], args["ys"], origin: origin(args["origin"]))

  defp apply_param("affine_transform", args, a, _),
    do: GeoSeries.affine_transform(a, args["matrix"])

  defp apply_param("interpolate", args, a, _),
    do: GeoSeries.interpolate(a, args["distance"], normalized: args["normalized"] == true)

  defp apply_param("project", args, a, b),
    do: GeoSeries.project(a, b, normalized: args["normalized"] == true)

  defp apply_param("concave_hull", args, a, _),
    do: GeoSeries.concave_hull(a, ratio: args["ratio"], allow_holes: args["allow_holes"])

  defp apply_param("minimum_rotated_rectangle", _, a, _),
    do: GeoSeries.minimum_rotated_rectangle(a)

  defp apply_param("minimum_bounding_circle", _, a, _), do: GeoSeries.minimum_bounding_circle(a)
  defp apply_param("minimum_bounding_radius", _, a, _), do: GeoSeries.minimum_bounding_radius(a)

  defp apply_param("maximum_inscribed_circle", args, a, _),
    do: GeoSeries.maximum_inscribed_circle(a, tolerance: args["tolerance"])

  defp apply_param("constrained_delaunay_triangles", _, a, _),
    do: GeoSeries.constrained_delaunay_triangles(a)

  defp apply_param("set_precision", args, a, _) do
    precise = GeoSeries.set_precision(a, args["grid_size"], mode: String.to_atom(args["mode"]))
    if args["normalize"], do: GeoSeries.normalize(precise), else: precise
  end

  defp apply_param("get_precision", %{"grid_size" => grid}, a, _),
    do: a |> GeoSeries.set_precision(grid) |> GeoSeries.get_precision()

  defp apply_param("get_precision", _, a, _), do: GeoSeries.get_precision(a)

  defp apply_param("to_json", args, a, _) do
    opts =
      for key <- ~w(show_bbox drop_id to_wgs84), Map.has_key?(args, key) do
        {String.to_atom(key), args[key]}
      end

    GeoSeries.to_json(a, opts)
  end

  defp apply_param("to_geo_interface", _, a, _),
    do: a |> GeoSeries.to_geo_interface() |> Jason.encode!()

  defp apply_param("offset_curve", args, a, _) do
    opts =
      [
        quad_segs: args["quad_segs"],
        join_style: String.to_atom(args["join_style"]),
        mitre_limit: args["mitre_limit"]
      ]
      |> Enum.reject(fn {_, v} -> is_nil(v) end)

    GeoSeries.offset_curve(a, args["distance"], opts)
  end

  defp apply_param("snap", args, a, b), do: GeoSeries.snap(a, b, args["tolerance"])
  defp apply_param("shared_paths", _, a, b), do: GeoSeries.shared_paths(a, b)

  # Shapely says infinity where the fixture holds GEOS' DBL_MAX.
  defp apply_param("minimum_clearance", _, a, _) do
    a
    |> GeoSeries.minimum_clearance()
    |> Series.to_list()
    |> Enum.map(fn
      :infinity -> @dbl_max
      value -> value
    end)
  end

  defp apply_param("minimum_clearance_line", _, a, _), do: GeoSeries.minimum_clearance_line(a)

  defp apply_param("to_crs", args, a, _), do: GeoSeries.to_crs(a, args["target"])
  defp apply_param("estimate_utm_crs", _, a, _), do: GeoSeries.estimate_utm_crs(a)
  defp apply_param("to_wkb_hex", _, a, _), do: GeoSeries.to_wkb(a, hex: true)
  defp apply_param("to_wkt_roundtrip", _, a, _), do: a

  defp apply_param("force_2d", _, a, _),
    do: a |> GeoSeries.force_2d() |> GeoSeries.to_wkb(hex: true)

  defp apply_param("force_3d", args, a, _),
    do: a |> GeoSeries.force_3d(args["z"]) |> GeoSeries.to_wkb(hex: true)

  defp apply_param("to_wkt_zm", _, a, _) do
    a |> GeoSeries.to_wkt() |> Enum.map(&(&1 && String.replace(&1, ~r/\s*([(),])\s*/, "\\1")))
  end

  defp apply_param("geojson", _, a, _),
    do: a |> GeoSeries.to_geojson() |> Enum.map(&encode_json/1)

  defp encode_json(nil), do: nil
  defp encode_json(map), do: map

  defp densify_opts(%{"densify" => d}), do: [densify: d]
  defp densify_opts(_), do: []

  defp origin("center"), do: :center
  defp origin("centroid"), do: :centroid
  defp origin([x, y]), do: {x, y}

  # -- comparison -------------------------------------------------------------------------

  @rtol 1.0e-9
  @atol 1.0e-9

  # Per-op tolerances: {rtol, atol}.
  defp tolerance(%{"op" => op, "label" => label}) do
    cond do
      op in ["area_geodesic", "length_geodesic", "distance_geodesic"] -> {1.0e-6, 1.0e-6}
      op == "to_crs" -> {1.0e-9, 1.0e-6}
      String.starts_with?(label, "hausdorff") -> {1.0e-9, 1.0e-9}
      true -> {@rtol, @atol}
    end
  end

  # Which comparison a geometry group uses.
  @exact_ops ~w(reverse interpolate translate scale rotate skew affine_transform to_crs
                segmentize remove_repeated_points orient_polygons exterior interiors
                to_wkt_roundtrip simplify get_geometry centroid shortest_line normalize)
  defp geom_mode(%{"op" => "buffer"}), do: :buffer
  defp geom_mode(%{"op" => "offset_curve"}), do: :curve
  defp geom_mode(%{"op" => "snap"}), do: :exact
  defp geom_mode(%{"op" => "shared_paths"}), do: :shared
  defp geom_mode(%{"op" => "minimum_clearance_line"}), do: :clearance_line
  defp geom_mode(%{"op" => "representative_point"}), do: :representative

  defp geom_mode(%{"op" => op})
       when op in ~w(concave_hull minimum_rotated_rectangle minimum_bounding_circle),
       do: :approx

  defp geom_mode(%{"op" => "maximum_inscribed_circle"}), do: :inscribed

  defp geom_mode(%{"op" => "set_precision", "args" => %{"mode" => "pointwise"}}), do: :exact

  defp geom_mode(%{"op" => "constrained_delaunay_triangles"}), do: :triangulation

  defp geom_mode(%{"op" => op})
       when op in ~w(intersection union difference symmetric_difference clip_by_rect),
       do: :overlay

  defp geom_mode(%{"op" => op}) when op in @exact_ops, do: :exact
  defp geom_mode(_), do: :topological

  defp compare(_group, _act, %{"error" => true}, _, _), do: {:skip, nil}
  defp compare(_group, {:raised, msg}, _e, _, _), do: {:differ, {:raised, msg}}

  defp compare(%{"kind" => "geom"} = group, wkt, e, inputs, crs),
    do: compare_geom(group, wkt, e, inputs, crs)

  defp compare(%{"kind" => kind} = group, act, e, _, _) when kind in ["float", "int"] do
    {rtol, atol} = tolerance(group)
    if numbers_close?(act, e, rtol, atol), do: {:agree, nil}, else: {:differ, {act, e}}
  end

  defp compare(%{"kind" => "floats"} = group, act, e, _, _) do
    {rtol, atol} = tolerance(group)

    ok =
      case {act, e} do
        {nil, nil} -> true
        {nil, _} -> false
        {_, nil} -> false
        {a, b} -> Enum.zip(a, b) |> Enum.all?(fn {x, y} -> numbers_close?(x, y, rtol, atol) end)
      end

    if ok, do: {:agree, nil}, else: {:differ, {act, e}}
  end

  defp compare(%{"kind" => "json"}, act, e, _, _) do
    expected = e && Jason.decode!(e)

    if json_close?(act && Jason.decode!(act), expected),
      do: {:agree, nil},
      else: {:differ, {act, expected}}
  end

  defp compare(%{"op" => "geojson"}, act, e, _, _) do
    expected = e && Jason.decode!(e)
    if json_close?(act, expected), do: {:agree, nil}, else: {:differ, {act, expected}}
  end

  # Z/M geometries as WKT text: same text around the numbers, numbers close (NaN equals NaN).
  defp compare(%{"kind" => "zm"} = group, act, e, _, _) do
    {rtol, atol} = tolerance(group)

    if zm_close?(act && squeeze(act), e && squeeze(e), rtol, atol),
      do: {:agree, nil},
      else: {:differ, {act, e}}
  end

  defp compare(_group, act, e, _, _) do
    if act == e, do: {:agree, nil}, else: {:differ, {act, e}}
  end

  @number ~r/-?(?:\d+\.?\d*(?:e[-+]?\d+)?|nan|inf)/i

  defp squeeze(text), do: String.replace(text, ~r/\s*([(),])\s*/, "\\1")

  defp zm_close?(nil, nil, _, _), do: true
  defp zm_close?(nil, _, _, _), do: false
  defp zm_close?(_, nil, _, _), do: false

  defp zm_close?(act, e, rtol, atol) do
    shape = &String.replace(&1, @number, "#")

    numbers = fn text ->
      @number |> Regex.scan(text) |> List.flatten() |> Enum.map(&zm_number/1)
    end

    shape.(act) == shape.(e) and
      act
      |> numbers.()
      |> Enum.zip(numbers.(e))
      |> Enum.all?(fn {x, y} -> zm_equal?(x, y, rtol, atol) end)
  end

  defp zm_number(text) do
    case String.downcase(text) do
      "nan" ->
        :nan

      "-nan" ->
        :nan

      "inf" ->
        :inf

      "-inf" ->
        :ninf

      _ ->
        text
        |> then(&if(String.contains?(&1, [".", "e", "E"]), do: &1, else: &1 <> ".0"))
        |> String.to_float()
    end
  end

  defp zm_equal?(x, y, rtol, atol) when is_float(x) and is_float(y),
    do: numbers_close?(x, y, rtol, atol)

  defp zm_equal?(x, y, _, _), do: x == y

  defp numbers_close?(nil, nil, _, _), do: true
  defp numbers_close?(nil, _, _, _), do: false
  defp numbers_close?(_, nil, _, _), do: false

  defp numbers_close?(a, e, rtol, atol) when is_number(a) and is_number(e),
    do: abs(a - e) <= atol + rtol * abs(e)

  defp numbers_close?(_, _, _, _), do: false

  defp json_close?(a, e) when is_number(a) and is_number(e),
    do: numbers_close?(a, e, @rtol, @atol)

  defp json_close?(a, e) when is_list(a) and is_list(e) and length(a) == length(e),
    do: Enum.zip(a, e) |> Enum.all?(fn {x, y} -> json_close?(x, y) end)

  defp json_close?(a, e) when is_map(a) and is_map(e) do
    Map.keys(a) == Map.keys(e) and Enum.all?(a, fn {k, v} -> json_close?(v, e[k]) end)
  end

  defp json_close?(a, e), do: a == e

  defp compare_geom(_group, nil, nil, _, _), do: {:agree, nil}
  defp compare_geom(_group, nil, e, _, _), do: {:differ, {nil, e}}
  defp compare_geom(_group, wkt, nil, _, _), do: {:differ, {wkt, nil}}

  defp compare_geom(group, wkt, e, {a_wkt, _b}, crs) do
    expected = GeoSeries.from_wkt!([e], crs: crs)
    actual = GeoSeries.from_wkt!([wkt], crs: crs)
    both_empty = empty?(actual) and empty?(expected)

    cond do
      both_empty -> {:agree, nil}
      empty?(actual) != empty?(expected) -> {:differ, {wkt, e}}
      true -> compare_mode(geom_mode(group), group, actual, expected, a_wkt, crs, wkt, e)
    end
  end

  defp compare_mode(:topological, _group, actual, expected, _a, _crs, wkt, e),
    do: verdict(GeoSeries.geom_equals(actual, expected), wkt, e)

  # Overlay results: topologically equal, or equal within the 1e-6 snapping grid of `geo`'s
  # boolean operations (intersections are computed on an integer grid, so a vertex can move by
  # about 1e-8 of the extent).
  defp compare_mode(:overlay, _group, actual, expected, _a, _crs, wkt, e) do
    case GeoSeries.geom_equals(actual, expected) |> Series.to_list() do
      [true] -> {:agree, nil}
      _ -> if close?(actual, expected), do: {:agree, :snapped}, else: {:differ, {wkt, e}}
    end
  end

  defp compare_mode(:exact, group, actual, expected, _a, _crs, wkt, e) do
    # to_crs: 1e-6 projected units (proj4rs vs PROJ, measured: within 1e-6 m) or 1e-9 degrees.
    tol =
      cond do
        group["op"] != "to_crs" -> 1.0e-9
        group["args"]["target"] == "EPSG:4326" -> 1.0e-9
        true -> 1.0e-6
      end

    verdict(GeoSeries.geom_equals_exact(actual, expected, tol), wkt, e)
  end

  # Same shape up to floating-point noise and vertex order: topologically equal, or within 1e-9
  # by Hausdorff distance and area.
  defp compare_mode(:approx, _group, actual, expected, _a, _crs, wkt, e) do
    case GeoSeries.geom_equals(actual, expected) |> Series.to_list() do
      [true] -> {:agree, nil}
      _ -> if near?(actual, expected, 1.0e-9), do: {:agree, :noise}, else: {:differ, {wkt, e}}
    end
  end

  # The inscribed circle is a search result: the center is only guaranteed within the
  # tolerance of the optimum, so the radius is compared with that tolerance and the center must
  # lie in the input.
  defp compare_mode(:inscribed, group, actual, expected, a_wkt, crs, wkt, e) do
    [al] = actual |> GeoSeries.length() |> Series.to_list()
    [el] = expected |> GeoSeries.length() |> Series.to_list()
    input = GeoSeries.from_wkt!([a_wkt], crs: crs)
    [x0, y0, x1, y1] = input |> GeoSeries.total_bounds() |> Tuple.to_list()
    tol = group["args"]["tolerance"] || max(x1 - x0, y1 - y0) / 1000
    center = GeoSeries.interpolate(actual, 0.0)

    cond do
      match?({:agree, _}, compare_mode(:approx, group, actual, expected, a_wkt, crs, wkt, e)) ->
        {:agree, nil}

      abs(al - el) <= tol + 1.0e-9 and
          GeoSeries.covers(input, center) |> Series.to_list() == [true] ->
        {:agree, :within_tolerance}

      true ->
        {:differ, {wkt, e}}
    end
  end

  # Constrained triangulations of a polygon: the same set of triangles, or (cocircular vertices
  # allow two triangulations) the same number of triangles covering the same area.
  defp compare_mode(:triangulation, _group, _actual, _expected, _a, _crs, wkt, e) do
    got = triangle_keys(wkt)
    want = triangle_keys(e)

    cond do
      got == want ->
        {:agree, nil}

      length(got) == length(want) and same_union?(parts_of(wkt), parts_of(e)) ->
        {:agree, :cocircular}

      true ->
        {:differ, {wkt, e}}
    end
  end

  # Offset curves: the same curve up to vertex placement: equal length and within 1e-6 of each
  # other (Hausdorff).
  defp compare_mode(:curve, _group, actual, expected, _a, _crs, wkt, e) do
    [al] = actual |> GeoSeries.length() |> Series.to_list()
    [el] = expected |> GeoSeries.length() |> Series.to_list()
    [hd] = actual |> GeoSeries.hausdorff_distance(expected) |> Series.to_list()

    if is_number(hd) and hd <= 1.0e-6 and abs(al - el) <= 1.0e-6 * (1.0 + abs(el)),
      do: {:agree, nil},
      else: {:differ, {wkt, e}}
  end

  # Shared paths: the same-direction and the opposite-direction multi-line, each in canonical
  # form (the collection itself would be sorted by GEOS' normalize, losing which is which).
  defp compare_mode(:shared, _group, actual, expected, _a, _crs, wkt, e) do
    same =
      for n <- [0, 1] do
        got = actual |> GeoSeries.get_geometry(n) |> GeoSeries.normalize()
        want = expected |> GeoSeries.get_geometry(n) |> GeoSeries.normalize()
        both_empty = empty?(got) and empty?(want)
        both_empty or GeoSeries.geom_equals_exact(got, want, 1.0e-9) |> Series.to_list() == [true]
      end

    if Enum.all?(same), do: {:agree, nil}, else: {:differ, {wkt, e}}
  end

  # The clearance line joins the closest pair; equally close pairs are a tie, so the same
  # length is enough when the lines differ.
  defp compare_mode(:clearance_line, _group, actual, expected, _a, _crs, wkt, e) do
    case GeoSeries.geom_equals(actual, expected) |> Series.to_list() do
      [true] ->
        {:agree, nil}

      _ ->
        [al] = actual |> GeoSeries.length() |> Series.to_list()
        [el] = expected |> GeoSeries.length() |> Series.to_list()

        if abs(al - el) <= 1.0e-9 * (1.0 + el),
          do: {:agree, :tie},
          else: {:differ, {wkt, e}}
    end
  end

  defp compare_mode(:buffer, _group, actual, expected, _a, _crs, wkt, e) do
    [aa] = actual |> GeoSeries.area() |> Series.to_list()
    [ea] = expected |> GeoSeries.area() |> Series.to_list()
    [at] = actual |> GeoSeries.geom_type() |> Series.to_list()
    [et] = expected |> GeoSeries.geom_type() |> Series.to_list()

    if at == et and abs(aa - ea) <= 1.0e-9 + 2.0e-2 * abs(ea),
      do: {:agree, nil},
      else: {:differ, {wkt, e}}
  end

  defp compare_mode(:representative, _group, actual, _expected, a_wkt, crs, wkt, e) do
    orig = GeoSeries.from_wkt!([a_wkt], crs: crs)

    case GeoSeries.intersects(orig, actual) |> Series.to_list() do
      [true] -> {:agree, nil}
      _ -> {:differ, {wkt, e}}
    end
  end

  defp near?(actual, expected, tol) do
    [aa] = actual |> GeoSeries.area() |> Series.to_list()
    [ea] = expected |> GeoSeries.area() |> Series.to_list()
    [hd] = actual |> GeoSeries.hausdorff_distance(expected) |> Series.to_list()

    is_number(aa) and is_number(ea) and is_number(hd) and hd <= tol and
      abs(aa - ea) <= tol * (1.0 + abs(ea))
  end

  defp parts_of(wkt) do
    {parts, _} = [wkt] |> GeoSeries.from_wkt!() |> GeoSeries.explode()
    GeoSeries.to_wkt(parts)
  end

  # One sorted, rounded vertex list per triangle of a collection.
  defp triangle_keys(wkt), do: wkt |> parts_of() |> Enum.map(&triangle_key/1) |> Enum.sort()

  defp close?(actual, expected) do
    [aa] = actual |> GeoSeries.area() |> Series.to_list()
    [ea] = expected |> GeoSeries.area() |> Series.to_list()
    [hd] = actual |> GeoSeries.hausdorff_distance(expected) |> Series.to_list()

    is_number(aa) and is_number(ea) and is_number(hd) and hd <= 1.0e-6 and
      abs(aa - ea) <= 1.0e-6 * (1.0 + abs(ea))
  end

  defp verdict([true], _, _), do: {:agree, nil}
  defp verdict(%Series{} = s, wkt, e), do: verdict(Series.to_list(s), wkt, e)
  defp verdict(_, wkt, e), do: {:differ, {wkt, e}}

  defp empty?(series), do: Series.to_list(GeoSeries.is_empty(series)) == [true]

  # -- frames: sjoin / sjoin_nearest / dissolve ---------------------------------------------

  @doc "Evaluates the sjoin, sjoin_nearest and dissolve fixtures (one case per configuration)."
  @spec run_frames() :: [result()]
  def run_frames do
    doc = read("frames")
    left = frame(doc["left"], ~w(lid grp v))
    right = frame(doc["right"], ~w(rid))

    sjoin =
      for c <- doc["sjoin"] do
        opts =
          [how: String.to_atom(c["how"]), predicate: String.to_atom(c["predicate"])] ++
            if(c["distance"], do: [distance: c["distance"]], else: [])

        got = left |> SpatialJoin.sjoin(right, opts) |> pairs()

        frame_result(
          "sjoin",
          "sjoin(#{c["predicate"]},#{c["how"]})",
          got == c["e"],
          {got, c["e"]}
        )
      end

    nearest =
      for c <- doc["sjoin_nearest"] do
        extra = if m = c["args"]["max_distance"], do: [max_distance: m], else: []
        opts = [how: String.to_atom(c["how"]), distance_col: "dist"] ++ extra
        got = left |> SpatialJoin.sjoin_nearest(right, opts) |> triples()
        label = "sjoin_nearest(#{c["how"]},#{inspect(c["args"])})"
        frame_result("sjoin_nearest", label, triples_close?(got, c["e"]), {got, c["e"]})
      end

    dissolved =
      for c <- doc["dissolve"] do
        by = c["by"]
        agg = String.to_atom(c["aggfunc"])
        opts = [aggfunc: %{"v" => agg}] ++ if(by, do: [by: by], else: [])
        got = left |> Dissolve.dissolve(opts) |> dissolve_rows(by)
        label = "dissolve(by=#{inspect(by)},#{c["aggfunc"]})"
        frame_result("dissolve", label, dissolve_close?(got, c["e"]), {got, c["e"]})
      end

    sjoin ++ nearest ++ dissolved ++ union_all_result()
  end

  defp union_all_result do
    %{"ids" => ids, "e" => e} = read("set_ops")["union_all"]
    corpus = corpus()

    [wkt] =
      ids
      |> Enum.map(&corpus[&1])
      |> GeoSeries.from_wkt!()
      |> GeoSeries.union_all()
      |> GeoSeries.to_wkt()

    ok =
      GeoSeries.geom_equals(GeoSeries.from_wkt!([wkt]), GeoSeries.from_wkt!([e]))
      |> Series.to_list() == [true]

    [frame_result("union_all", "union_all", ok, {wkt, e})]
  end

  # -- frames: overlay / clip / explode / intersection_all -----------------------------------

  @doc """
  Evaluates the whole-series fixtures: `make_grid`, `delaunay_triangles` and `voronoi_polygons`.
  GeoPandas treats the series as one point cloud and returns a series of another length, so each
  case compares a list of geometries.
  """
  @spec run_collections() :: [result()]
  def run_collections do
    corpus = corpus()

    for c <- read("collections")["cases"] do
      series = GeoSeries.from_wkt!(Enum.map(c["ids"], &corpus[&1]))

      {status, detail} =
        try do
          got = c["op"] |> collection_op(series, c["args"]) |> GeoSeries.to_wkt()
          compare_collection(c["op"], c["args"], got, c["e"])
        rescue
          e -> {:differ, {:raised, Exception.message(e)}}
        end

      %{
        file: "collections",
        op: c["op"],
        label: c["label"],
        id: c["label"],
        status: status,
        nullish: false,
        detail: detail
      }
    end
  end

  defp collection_op("make_grid", series, args) do
    GeoSeries.make_grid(series, args["cell_size"],
      grid_type: String.to_atom(args["grid_type"]),
      feature_type: String.to_atom(args["feature_type"]),
      intersect: args["intersect"],
      flat_topped: args["flat_topped"],
      offset: args["offset"] && List.to_tuple(args["offset"])
    )
  end

  defp collection_op("delaunay_triangles", series, args) do
    GeoSeries.delaunay_triangles(series,
      tolerance: args["tolerance"] || 0.0,
      only_edges: args["only_edges"] == true
    )
  end

  defp collection_op("voronoi_polygons", series, args) do
    GeoSeries.voronoi_polygons(series,
      tolerance: args["tolerance"] || 0.0,
      extend_to: args["extend_to"],
      only_edges: args["only_edges"] == true
    )
  end

  # make_grid: same length, same order, vertex for vertex.
  defp compare_collection("make_grid", _args, got, want) when length(got) != length(want),
    do: {:differ, {:count, length(got), length(want)}}

  defp compare_collection("make_grid", _args, got, want) do
    got
    |> Enum.zip(want)
    |> Enum.with_index()
    |> Enum.find(fn {{g, w}, _} -> not grid_cell_close?(g, w) end)
    |> case do
      nil -> {:agree, nil}
      {{g, w}, i} -> {:differ, {i, g, w}}
    end
  end

  # Triangles: the same set (order is not specified); failing that, with cocircular points two
  # triangulations are valid, so the same number of triangles covering the same area.
  defp compare_collection("delaunay_triangles", %{"only_edges" => true}, got, want) do
    got_keys = edge_keys(got)
    want_keys = edge_keys(want)
    extra_got = got_keys -- want_keys
    extra_want = want_keys -- got_keys

    cond do
      extra_got == [] and extra_want == [] -> {:agree, nil}
      flips_only?(extra_got, extra_want) -> {:agree, :cocircular}
      true -> {:differ, {:edges, extra_got, extra_want}}
    end
  end

  defp compare_collection("delaunay_triangles", _args, got, want) do
    cond do
      Enum.sort(Enum.map(got, &triangle_key/1)) == Enum.sort(Enum.map(want, &triangle_key/1)) ->
        {:agree, nil}

      length(got) == length(want) and same_union?(got, want) ->
        {:agree, :cocircular}

      true ->
        {:differ, {:triangles, got, want}}
    end
  end

  # Voronoi: every expected cell or side has a counterpart within 1e-9, and the counts agree.
  defp compare_collection("voronoi_polygons", _args, got, want) do
    if length(got) == length(want) and Enum.all?(want, &has_close_match?(got, &1)),
      do: {:agree, nil},
      else: {:differ, {:cells, got, want}}
  end

  defp has_close_match?(got, wkt) do
    got
    |> GeoSeries.from_wkt!()
    |> GeoSeries.hausdorff_distance(wkt)
    |> Series.to_list()
    |> Enum.any?(&(is_number(&1) and &1 <= 1.0e-9))
  end

  defp grid_cell_close?(got, want) do
    GeoSeries.geom_equals_exact(GeoSeries.from_wkt!([got]), GeoSeries.from_wkt!([want]), 1.0e-9)
    |> Series.to_list() == [true]
  end

  # Two valid triangulations of the same points differ only by flipped diagonals: each edge one
  # has and the other lacks is properly crossed by an edge of the other.
  defp flips_only?(extra_got, extra_want) do
    length(extra_got) == length(extra_want) and
      Enum.all?(extra_got, fn [a, b] ->
        Enum.any?(extra_want, fn [c, d] -> properly_cross?(a, b, c, d) end)
      end)
  end

  defp properly_cross?(a, b, c, d) do
    orient = fn [px, py], [qx, qy], [rx, ry] -> (qx - px) * (ry - py) - (qy - py) * (rx - px) end
    d1 = orient.(a, b, c)
    d2 = orient.(a, b, d)
    d3 = orient.(c, d, a)
    d4 = orient.(c, d, b)
    d1 * d2 < 0 and d3 * d4 < 0
  end

  defp edge_keys(wkts),
    do: wkts |> Enum.map(&coordinate_pairs/1) |> Enum.map(&Enum.sort/1) |> Enum.sort()

  defp triangle_key(wkt), do: wkt |> coordinate_pairs() |> Enum.uniq() |> Enum.sort()

  defp coordinate_pairs(wkt) do
    ~r/-?\d+(?:\.\d+)?(?:e-?\d+)?/
    |> Regex.scan(wkt)
    |> List.flatten()
    |> Enum.map(&(&1 |> Float.parse() |> elem(0) |> Float.round(9) |> Kernel.+(0.0)))
    |> Enum.chunk_every(2)
  end

  defp same_union?(a, b) do
    union = fn wkts -> wkts |> GeoSeries.from_wkt!() |> GeoSeries.union_all() end
    GeoSeries.geom_equals(union.(a), union.(b)) |> Series.to_list() == [true]
  end

  @doc """
  Evaluates the whole-series polygonization fixtures (`polygonize` with `full: true` and
  `build_area`): the polygons, cut edges, dangles and invalid rings must match as sets of
  shapes (same area and within 1e-9 by Hausdorff distance).
  """
  @spec run_aggregates() :: [result()]
  def run_aggregates do
    doc = read("parity_j_aggregate")
    corpus = Map.merge(corpus(), doc["extra"])

    for c <- doc["cases"] do
      series = GeoSeries.from_wkt!(Enum.map(c["ids"], &corpus[&1]))

      {status, detail} =
        try do
          aggregate(c["op"], c["args"]["node"], series, c["e"])
        rescue
          e -> {:differ, {:raised, Exception.message(e)}}
        end

      %{
        file: "parity_j_aggregate",
        op: c["op"],
        label: c["label"],
        id: c["label"],
        status: status,
        nullish: false,
        detail: detail
      }
    end
  end

  defp aggregate("polygonize", node, series, e) do
    {polygons, cuts, dangles, invalid} = GeoSeries.polygonize(series, node: node, full: true)

    misses =
      for {name, got} <- [
            {"polygons", polygons},
            {"cuts", cuts},
            {"dangles", dangles},
            {"invalid", invalid}
          ],
          not same_shapes?(GeoSeries.to_wkt(got), e[name]),
          do: {name, GeoSeries.to_wkt(got), e[name]}

    if misses == [], do: {:agree, nil}, else: {:differ, misses}
  end

  defp aggregate("build_area", node, series, e) do
    got = GeoSeries.build_area(series, node: node)
    want = e["polygons"]

    if GeoSeries.size(got) == length(want) and same_area?(GeoSeries.to_wkt(got), want),
      do: {:agree, nil},
      else: {:differ, {GeoSeries.to_wkt(got), want}}
  end

  # Both lists describe the same shapes, in any order.
  defp same_shapes?(got, want) when length(got) != length(want), do: false

  defp same_shapes?(got, want) do
    {_, ok} =
      Enum.reduce(want, {got, true}, fn shape, {pool, ok} ->
        case Enum.find_index(pool, &close_shape?(&1, shape)) do
          nil -> {pool, false}
          i -> {List.delete_at(pool, i), ok}
        end
      end)

    ok
  end

  defp close_shape?(a, b) do
    sa = GeoSeries.from_wkt!([a])
    sb = GeoSeries.from_wkt!([b])
    [hd] = sa |> GeoSeries.hausdorff_distance(sb) |> Series.to_list()
    [aa] = sa |> GeoSeries.area() |> Series.to_list()
    [ba] = sb |> GeoSeries.area() |> Series.to_list()
    is_number(hd) and hd <= 1.0e-9 and abs(aa - ba) <= 1.0e-9 * (1.0 + abs(ba))
  end

  # The union of both lists covers the same area.
  defp same_area?([], []), do: true

  defp same_area?(got, want) do
    union = fn wkts -> wkts |> GeoSeries.from_wkt!() |> GeoSeries.union_all() end
    sym = GeoSeries.symmetric_difference(union.(got), union.(want))
    [area] = sym |> GeoSeries.area() |> Series.to_list()
    is_number(area) and area <= 1.0e-9
  end

  @doc """
  Evaluates the coverage fixtures (`is_valid_coverage`, `invalid_coverage_edges`,
  `simplify_coverage`): whole-series cases whose geometries must match GeoPandas vertex for
  vertex (compared as the crate prints them).
  """
  @spec run_coverage() :: [result()]
  def run_coverage do
    doc = read("parity_k")

    for c <- doc["cases"] do
      series = GeoSeries.from_wkt!(doc["sets"][c["set"]])

      {status, detail} =
        try do
          coverage_case(c["op"], c["args"], series, c["e"])
        rescue
          e -> {:differ, {:raised, Exception.message(e)}}
        end

      %{
        file: "parity_k",
        op: c["op"],
        label: c["label"],
        id: c["label"],
        status: status,
        nullish: false,
        detail: detail
      }
    end
  end

  defp coverage_case("is_valid_coverage", %{"gap_width" => gap}, series, expected) do
    compare(GeoSeries.is_valid_coverage(series, gap_width: gap), expected)
  end

  defp coverage_case("invalid_coverage_edges", %{"gap_width" => gap}, series, expected) do
    got = series |> GeoSeries.invalid_coverage_edges(gap_width: gap) |> GeoSeries.to_wkt()
    compare(got, canonical_wkt(expected))
  end

  defp coverage_case("simplify_coverage", args, series, expected) do
    got =
      series
      |> GeoSeries.simplify_coverage(args["tolerance"],
        simplify_boundary: args["simplify_boundary"]
      )
      |> GeoSeries.to_wkt()

    compare(got, canonical_wkt(expected))
  end

  defp compare(got, expected) when got == expected, do: {:agree, nil}
  defp compare(got, expected), do: {:differ, {got, expected}}

  defp canonical_wkt(wkts), do: wkts |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt()

  @doc "Evaluates the overlay, clip, explode and intersection_all fixtures."
  @spec run_overlay() :: [result()]
  def run_overlay do
    doc = read("overlay")
    overlay_cases(doc) ++ clip_cases(doc) ++ explode_cases(doc) ++ intersection_all_cases(doc)
  end

  defp overlay_cases(doc) do
    for c <- doc["overlay"] do
      %{"left" => left, "right" => right} = doc["frames"][c["frames"]]
      df1 = overlay_frame(left, "lid", "v")
      df2 = overlay_frame(right, "rid", "w")
      keep = c["keep_geom_type"]
      label = "overlay(#{c["frames"]},#{c["how"]},keep=#{inspect(keep)})"

      got =
        try do
          opts = [how: String.to_atom(c["how"]), keep_geom_type: keep]
          {result, _log} = ExUnit.CaptureLog.with_log(fn -> GeoFrame.overlay(df1, df2, opts) end)
          result
        rescue
          e -> {:raised, Exception.message(e)}
        end

      frame_case("overlay", "overlay", label, got, c["columns"], c["rows"])
    end
  end

  defp overlay_frame(rows, id, value) do
    data =
      DataFrame.new([
        {id, Enum.map(rows, &Enum.at(&1, 0))},
        {"k", Enum.map(rows, &Enum.at(&1, 1))},
        {value, Enum.map(rows, &Enum.at(&1, 2))}
      ])

    GeoDataFrame.new(data, GeoSeries.from_wkt!(Enum.map(rows, &Enum.at(&1, 3))))
  end

  defp clip_cases(doc) do
    corpus = corpus()

    for c <- doc["clip"] do
      ids = c["ids"]

      gdf =
        GeoDataFrame.new(DataFrame.new(id: ids), GeoSeries.from_wkt!(Enum.map(ids, &corpus[&1])))

      mask = if is_list(c["mask"]), do: List.to_tuple(c["mask"]), else: c["mask"]
      label = "clip(#{c["data"]},#{c["label"]},keep=#{c["keep_geom_type"]})"

      got =
        try do
          gdf
          |> GeoFrame.clip(mask, keep_geom_type: c["keep_geom_type"])
          |> sort_by_first_column()
        rescue
          e -> {:raised, Exception.message(e)}
        end

      frame_case("clip", "clip", label, got, c["columns"], c["rows"])
    end
  end

  defp sort_by_first_column(gdf) do
    order =
      gdf
      |> column("id")
      |> Enum.with_index()
      |> Enum.sort()
      |> Enum.map(&elem(&1, 1))

    GeoDataFrame.take(gdf, order)
  end

  defp explode_cases(doc) do
    corpus = corpus()

    for c <- doc["explode"] do
      ids = c["ids"]
      n = Enum.to_list(0..(length(ids) - 1))
      gdf = GeoDataFrame.new(DataFrame.new(n: n), GeoSeries.from_wkt!(Enum.map(ids, &corpus[&1])))
      label = "explode(index_parts=#{c["index_parts"]},ignore_index=#{c["ignore_index"]})"

      opts = [index_parts: c["index_parts"], ignore_index: c["ignore_index"]]
      got = GeoDataFrame.explode(gdf, opts)

      expected_columns = explode_columns(c) ++ ["n", "geometry"]
      rows = for r <- c["rows"], do: explode_values(c, r) ++ [r["n"], r["g"]]

      frame_case("explode", "explode", label, got, expected_columns, rows)
    end
  end

  defp explode_columns(c) do
    if(c["ignore_index"], do: [], else: ["index"]) ++ if(c["index_parts"], do: ["part"], else: [])
  end

  defp explode_values(c, r) do
    if(c["ignore_index"], do: [], else: [r["index"]]) ++
      if c["index_parts"], do: [r["part"]], else: []
  end

  defp intersection_all_cases(doc) do
    corpus = corpus()

    for c <- doc["intersection_all"] do
      series = GeoSeries.from_wkt!(Enum.map(c["ids"], &corpus[&1]))
      [wkt] = series |> GeoSeries.intersection_all() |> GeoSeries.to_wkt()
      label = "intersection_all(#{Enum.join(c["ids"], ",")})"
      {status, detail} = compare_geom(%{"op" => "intersection"}, wkt, c["e"], {nil, nil}, nil)

      %{
        file: "overlay",
        op: "intersection_all",
        label: label,
        id: label,
        status: status,
        nullish: false,
        detail: detail
      }
    end
  end

  # Compares a result frame (or a raised error) with expected column names and rows.
  defp frame_case(op, _kind, label, got, columns, rows) do
    {status, detail} =
      case got do
        {:raised, msg} -> {:differ, {:raised, msg}}
        %GeoDataFrame{} = gdf -> compare_frame(gdf, columns, rows)
      end

    %{
      file: "overlay",
      op: op,
      label: label,
      id: label,
      status: status,
      nullish: false,
      detail: detail
    }
  end

  defp compare_frame(gdf, columns, expected) do
    names = GeoDataFrame.names(gdf)
    actual = frame_values(gdf)

    cond do
      names != columns ->
        {:differ, {:columns, names, columns}}

      length(actual) != length(expected) ->
        {:differ, {:row_count, length(actual), length(expected)}}

      true ->
        case first_disagreement(actual, expected) do
          nil -> {:agree, nil}
          bad -> {:differ, bad}
        end
    end
  end

  defp first_disagreement(actual, expected) do
    actual
    |> Enum.zip(expected)
    |> Enum.with_index()
    |> Enum.find_value(fn {{got, want}, i} ->
      if row_agrees?(got, want), do: nil, else: {i, got, want}
    end)
  end

  defp frame_values(gdf) do
    attrs = gdf.data |> DataFrame.names() |> Enum.map(&column(gdf, &1))
    wkts = GeoSeries.to_wkt(GeoDataFrame.geometry(gdf))
    Enum.zip_with(attrs ++ [wkts], & &1)
  end

  defp row_agrees?(got, want) do
    {g_attrs, [g_wkt]} = Enum.split(got, -1)
    {w_attrs, [w_wkt]} = Enum.split(want, -1)

    Enum.zip(g_attrs, w_attrs)
    |> Enum.all?(fn
      {a, b} when is_number(a) and is_number(b) -> numbers_close?(a, b, @rtol, @atol)
      {a, b} -> a == b
    end) and
      match?({:agree, _}, compare_geom(%{"op" => "intersection"}, g_wkt, w_wkt, {nil, nil}, nil))
  end

  defp frame_result(op, label, ok, detail) do
    %{
      file: "frames",
      op: op,
      label: label,
      id: label,
      status: if(ok, do: :agree, else: :differ),
      nullish: false,
      detail: if(ok, do: nil, else: detail)
    }
  end

  defp frame(rows, columns) do
    data =
      for c <- columns, into: %{} do
        {c, Enum.map(rows, & &1[c])}
      end

    GeoDataFrame.new(DataFrame.new(data), GeoSeries.from_wkt!(Enum.map(rows, & &1["g"])))
  end

  defp column(gdf, name),
    do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  defp pairs(gdf) do
    gdf
    |> column("lid")
    |> Enum.zip(column(gdf, "rid"))
    |> Enum.map(&Tuple.to_list/1)
    |> sort_rows()
  end

  defp triples(gdf) do
    [column(gdf, "lid"), column(gdf, "rid"), column(gdf, "dist")]
    |> Enum.zip()
    |> Enum.map(&Tuple.to_list/1)
    |> sort_rows()
  end

  defp sort_rows(rows), do: Enum.sort_by(rows, fn [a, b | _] -> {a || "", b || ""} end)

  defp triples_close?(got, expected) do
    length(got) == length(expected) and
      got
      |> Enum.zip(expected)
      |> Enum.all?(fn {[l1, r1, d1], [l2, r2, d2]} ->
        l1 == l2 and r1 == r2 and numbers_close?(d1, d2, @rtol, @atol)
      end)
  end

  defp dissolve_rows(gdf, by) do
    keys = if by, do: column(gdf, by), else: List.duplicate(nil, GeoDataFrame.n_rows(gdf))
    wkts = GeoSeries.to_wkt(GeoDataFrame.geometry(gdf))

    [keys, column(gdf, "v"), wkts]
    |> Enum.zip()
    |> Enum.map(&Tuple.to_list/1)
    |> Enum.map(&List.to_tuple/1)
  end

  defp dissolve_close?(got, expected) do
    length(got) == length(expected) and
      got
      |> Enum.zip(expected)
      |> Enum.all?(fn {{k1, v1, w1}, %{"key" => k2, "v" => v2, "g" => w2}} ->
        k1 == k2 and numbers_close?(v1, v2, @rtol, @atol) and
          GeoSeries.geom_equals(GeoSeries.from_wkt!([w1]), GeoSeries.from_wkt!([w2]))
          |> Series.to_list() ==
            [true]
      end)
  end
end
