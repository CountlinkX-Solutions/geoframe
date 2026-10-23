if Code.ensure_loaded?(Kino.SmartCell) do
  defmodule GeoFrame.Kino.CellSupport do
    @moduledoc false
    # Parsing and AST helpers shared by the GeoFrame Smart Cells.

    @doc "Trims a string attribute; anything else becomes an empty string."
    @spec text(term()) :: String.t()
    def text(value) when is_binary(value), do: String.trim(value)
    def text(_other), do: ""

    @doc "Parses a decimal number such as 3 or -1.5e2, or returns `nil`."
    @spec number(term()) :: float() | nil
    def number(value) do
      case Float.parse(text(value)) do
        {number, ""} -> number
        _other -> nil
      end
    end

    @doc "Parses a comma separated minx, miny, maxx, maxy into a float tuple, or `nil`."
    @spec bbox(term()) :: {float(), float(), float(), float()} | nil
    def bbox(value) do
      numbers = value |> text() |> String.split(",") |> Enum.map(&number/1)

      case numbers do
        [minx, miny, maxx, maxy]
        when is_float(minx) and is_float(miny) and is_float(maxx) and is_float(maxy) and
               minx <= maxx and miny <= maxy ->
          {minx, miny, maxx, maxy}

        _other ->
          nil
      end
    end

    @doc "Whether `name` is a valid, non-empty Elixir variable name."
    @spec variable?(term()) :: boolean()
    def variable?(name) when is_binary(name),
      do: name != "" and Kino.SmartCell.valid_variable_name?(name)

    def variable?(_other), do: false

    @doc "The AST of the variable called `name`."
    @spec var(String.t()) :: Macro.t()
    def var(name), do: Macro.var(String.to_atom(name), nil)

    @doc "Formats statements as one block of formatted Elixir source."
    @spec block([Macro.t()]) :: String.t()
    def block(statements), do: Kino.SmartCell.quoted_to_string({:__block__, [], statements})
  end
end
