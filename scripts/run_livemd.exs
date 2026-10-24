# Runs the code cells of a Livebook notebook, in order, in one shared binding and environment.
# Livebook has no headless runner, so this verifies that a notebook's code works.
#
#     elixir scripts/run_livemd.exs notebooks/spike.livemd
#
# Each cell's value is also passed through `Kino.Render.to_livebook/1` when Kino is loaded,
# which exercises the rendering a notebook would show. The first cell is expected to be
# `Mix.install/2`. Like Livebook, the runner evaluates three kinds of fences:
#
#   * ```elixir        - Elixir code; aliases, imports and requires carry over to later cells;
#   * ```python        - Python code through the `~PY` sigil of Pythonx, so Elixir variables
#                        are Python globals and the globals a cell assigns become Elixir
#                        variables, as in a Livebook Python cell;
#   * ```pyproject.toml - the Python environment, passed to `Pythonx.uv_init/1`.
#
# Python cells need `{:pythonx, "~> 0.4"}` in the notebook's `Mix.install/2`.

defmodule RunLivemd do
  def cells(markdown) do
    ~r/^```(elixir|python|pyproject\.toml)[^\n]*\n(.*?)^```\s*$/ms
    |> Regex.scan(markdown, capture: :all_but_first)
    |> Enum.map(fn [language, code] -> {language, code} end)
  end

  def run(path) do
    path = Path.expand(path)
    cells = path |> File.read!() |> cells()
    IO.puts("#{length(cells)} code cells in #{path}")

    state = %{binding: [], env: Code.env_for_eval(file: path), python?: false}

    cells
    |> Enum.with_index(1)
    |> Enum.reduce(state, fn {{language, code}, index}, state ->
      IO.puts("--- cell #{index} (#{language})")
      {value, state} = eval(language, code, state, path)
      render(value)
      state
    end)

    IO.puts("OK: all cells evaluated")
  end

  defp eval("elixir", code, state, path) do
    quoted = Code.string_to_quoted!(code, file: path)
    run_quoted(quoted, state)
  end

  defp eval("pyproject.toml", code, state, _path) do
    ensure_pythonx!()
    {apply(Pythonx, :uv_init, [code]), state}
  end

  defp eval("python", code, state, _path) do
    ensure_pythonx!()

    state =
      if state.python? do
        state
      else
        {_, state} = run_quoted(quote(do: import(Pythonx, only: [sigil_PY: 2])), state)
        %{state | python?: true}
      end

    run_quoted({:sigil_PY, [], [{:<<>>, [], [code]}, []]}, state)
  end

  defp run_quoted(quoted, state) do
    {value, binding, env} = Code.eval_quoted_with_env(quoted, state.binding, state.env)
    {value, %{state | binding: binding, env: env}}
  end

  defp ensure_pythonx! do
    unless Code.ensure_loaded?(Pythonx) do
      raise "a Python cell needs {:pythonx, \"~> 0.4\"} in the notebook's Mix.install/2"
    end
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
