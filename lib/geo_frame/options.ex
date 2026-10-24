defmodule GeoFrame.Options do
  @moduledoc false

  # One place for the "unknown option" check, so a typo such as `predicat:` raises instead of
  # silently running with the defaults. Option lists are keyword lists, as everywhere in the
  # public API.

  @doc """
  Raises `ArgumentError` naming the keys of `opts` that are not in `allowed`.
  Returns `opts` so the call can sit in a pipeline.
  """
  @spec validate!(keyword(), [atom()]) :: keyword()
  def validate!(opts, allowed) when is_list(opts) and is_list(allowed) do
    unknown = Keyword.keys(opts) -- allowed

    if unknown != [] do
      raise ArgumentError,
            "unknown option(s) #{inspect(unknown)}; supported options: #{inspect(allowed)}"
    end

    opts
  end
end
