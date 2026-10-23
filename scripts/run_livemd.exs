# Runs the Elixir code cells of a Livebook notebook, in order, in one shared binding.
# Livebook has no headless runner, so this verifies that a notebook's code works.
#
#     elixir scripts/run_livemd.exs notebooks/spike.livemd
#
# Each cell's value is also passed through `Kino.Render.to_livebook/1` when Kino is loaded,
# which exercises the rendering a notebook would show. The first cell is expected to be
# `Mix.install/2`. Only ```elixir fences are run; other fences are ignored.

defmodule RunLivemd do
  def cells(markdown) do
    ~r/^```elixir[^\n]*\n(.*?)^```\s*$/ms
    |> Regex.scan(markdown, capture: :all_but_first)
    |> Enum.map(fn [code] -> code end)
  end

  def run(path) do
    path = Path.expand(path)
    cells = path |> File.read!() |> cells()
    IO.puts("#{length(cells)} code cells in #{path}")

    cells
    |> Enum.with_index(1)
    |> Enum.reduce([], fn {code, index}, binding ->
      IO.puts("--- cell #{index}")
      {value, binding} = Code.eval_string(code, binding, file: path)
      render(value)
      binding
    end)

    IO.puts("OK: all cells evaluated")
  end

  # Mimics the implicit output of a Livebook cell: renders the value when Kino is loaded.
  defp render(value) do
    if Code.ensure_loaded?(Kino.Render) do
      output = Kino.Render.to_livebook(value)
      IO.puts("  rendered: " <> preview(output))
    else
      IO.puts("  value: " <> preview(value))
    end
  end

  defp preview(term), do: term |> inspect(limit: 4, printable_limit: 60) |> String.slice(0, 110)
end

case System.argv() do
  [path] -> RunLivemd.run(path)
  _ -> IO.puts(:stderr, "usage: elixir scripts/run_livemd.exs NOTEBOOK.livemd") && System.halt(1)
end
