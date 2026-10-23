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

  @fixtures ~w(measures unary_predicates binary_predicates constructive set_ops affine
               linear_referencing crs geodesic io)
  @dir Path.expand("../reference/fixtures", __DIR__)

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
                extract_unique_points reverse representative_point make_valid line_merge interiors)
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

  defp apply_param("get_geometry", args, a, _), do: GeoSeries.get_geometry(a, args["n"])
  defp apply_param("shortest_line", _, a, b), do: GeoSeries.shortest_line(a, b)

  defp apply_param("translate", args, a, _), do: GeoSeries.translate(a, args["x"], args["y"])

  defp apply_param("scale", args, a, _),
    do: GeoSeries.scale(a, args["x"], args["y"], origin: origin(args["origin"]))

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

  defp apply_param("to_crs", args, a, _), do: GeoSeries.to_crs(a, args["target"])
  defp apply_param("estimate_utm_crs", _, a, _), do: GeoSeries.estimate_utm_crs(a)
  defp apply_param("to_wkb_hex", _, a, _), do: GeoSeries.to_wkb(a, hex: true)
  defp apply_param("to_wkt_roundtrip", _, a, _), do: a

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
                to_wkt_roundtrip simplify get_geometry centroid shortest_line)
  defp geom_mode(%{"op" => "buffer"}), do: :buffer
  defp geom_mode(%{"op" => "representative_point"}), do: :representative

  defp geom_mode(%{"op" => op}) when op in ~w(intersection union difference symmetric_difference),
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

  defp compare(%{"op" => "geojson"}, act, e, _, _) do
    expected = e && Jason.decode!(e)
    if json_close?(act, expected), do: {:agree, nil}, else: {:differ, {act, expected}}
  end

  defp compare(_group, act, e, _, _) do
    if act == e, do: {:agree, nil}, else: {:differ, {act, e}}
  end

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
