defmodule GeoFrame.ReferenceTest do
  @moduledoc """
  Differential tests against GeoPandas 1.2 / Shapely 2.2 (GEOS 3.14).

  The expected values live in `test/reference/fixtures/*.json`, produced by
  `test/reference/generate.py` (dev-only; see `test/reference/README.md`). Python is never
  needed here.

  Policy for known deviations (`test/reference/known_deviations.exs`): a listed case is
  asserted to STILL deviate, so fixing a deviation fails this suite until its entry is
  removed. A differing case that is not listed fails the suite.
  """
  use ExUnit.Case, async: true

  alias GeoFrame.Reference

  @moduletag :reference

  @files ~w(measures unary_predicates binary_predicates constructive set_ops affine
            linear_referencing crs geodesic io)
  @frame_ops ~w(sjoin sjoin_nearest dissolve union_all)
  @overlay_ops ~w(overlay clip explode intersection_all)

  {deviations, _} = Code.eval_file(Path.expand("../reference/known_deviations.exs", __DIR__))
  @deviations deviations

  @labels for file <- @files,
              group <- Reference.read(file)["groups"],
              uniq: true,
              do: {file, group["label"]}

  setup_all do
    results = Reference.run() ++ Reference.run_frames() ++ Reference.run_overlay()
    if System.get_env("REFERENCE_REPORT"), do: report(results)
    %{results: results}
  end

  test "fixtures come from the pinned reference versions" do
    meta = Reference.meta()
    assert meta["geopandas"] =~ ~r/^1\.2\./
    assert meta["shapely"] =~ ~r/^2\.2\./
    assert meta["geos"] =~ ~r/^3\.14\./
  end

  test "the reference suite covers a substantial number of cases", %{results: results} do
    assert length(results) > 5_000
  end

  for {file, label} <- @labels do
    test "#{file}: #{label}", %{results: results} do
      check(Enum.filter(results, &(&1.file == unquote(file) and &1.label == unquote(label))))
    end
  end

  for op <- @frame_ops do
    test "frames: #{op}", %{results: results} do
      check(Enum.filter(results, &(&1.file == "frames" and &1.op == unquote(op))))
    end
  end

  for op <- @overlay_ops do
    test "overlay fixtures: #{op}", %{results: results} do
      check(Enum.filter(results, &(&1.file == "overlay" and &1.op == unquote(op))))
    end
  end

  test "every documented deviation still deviates", %{results: results} do
    by_id = Map.new(results, &{&1.id, &1})

    stale =
      for %{cases: cases, op: op} <- @deviations,
          id <- cases,
          not match?(%{status: :differ, op: ^op}, by_id[id]),
          do: id

    assert stale == [],
           "these cases now agree with GeoPandas (or vanished): remove them from " <>
             "test/reference/known_deviations.exs and docs/parity.md:\n" <> Enum.join(stale, "\n")
  end

  test "the null/empty class still occurs", %{results: results} do
    assert Enum.any?(results, &(&1.status == :differ and &1.nullish))
  end

  # -- helpers ------------------------------------------------------------------------------

  defp check(results) do
    assert results != []

    unexplained =
      for %{status: :differ} = r <- results, not documented?(r), do: {r.id, r.detail}

    assert unexplained == [],
           "GeoFrame disagrees with GeoPandas (fix it, or add it to known_deviations.exs):\n" <>
             Enum.map_join(unexplained, "\n", fn {id, detail} ->
               "  #{id}: #{inspect(detail, limit: 8, printable_limit: 200)}"
             end)
  end

  defp documented?(result), do: Enum.any?(@deviations, &entry_matches?(&1, result))

  defp entry_matches?(%{nullish: true}, result), do: result.nullish
  defp entry_matches?(%{op: op, cases: cases}, result), do: result.op == op and result.id in cases
  defp entry_matches?(_, _), do: false

  defp report(results) do
    rows =
      results
      |> Enum.group_by(& &1.op)
      |> Enum.sort()
      |> Enum.map(fn {op, rs} ->
        agree = Enum.count(rs, &(&1.status == :agree))
        documented = Enum.count(rs, &(&1.status == :differ and documented?(&1)))
        unexplained = length(rs) - agree - documented
        "| #{op} | #{length(rs)} | #{agree} | #{documented} | #{unexplained} |"
      end)

    IO.puts(
      "\n| op | cases | agree | deviate (documented) | unexplained |\n|---|---|---|---|---|"
    )

    IO.puts(Enum.join(rows, "\n"))
  end
end
