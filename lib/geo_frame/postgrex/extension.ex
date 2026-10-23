if Code.ensure_loaded?(Postgrex) do
  defmodule GeoFrame.Postgrex.Extension do
    @moduledoc """
    A `Postgrex.Extension` for the PostGIS `geometry` type.

    Postgrex has no built-in handler for `geometry`; this one decodes a value to its raw
    **EWKB binary** (WKB plus the SRID) without building Elixir structs, which is exactly what
    `GeoFrame.read_postgis/3` consumes. It uses the text transfer format (hex EWKB) because the
    binary `geometry_send` drops the SRID. On encode it accepts an EWKB or WKB binary.

    Register it through `GeoFrame.Postgrex.Types`, or define your own types module:

        Postgrex.Types.define(MyApp.PostgresTypes, [GeoFrame.Postgrex.Extension])
    """

    @behaviour Postgrex.Extension

    @impl true
    def init(_opts), do: nil

    @impl true
    def matching(_state), do: [type: "geometry"]

    @impl true
    def format(_state), do: :text

    @impl true
    def encode(_state) do
      quote do
        bin when is_binary(bin) ->
          hex = Base.encode16(bin)
          [<<byte_size(hex)::signed-size(32)>> | hex]
      end
    end

    @impl true
    def decode(_state) do
      quote do
        <<len::signed-size(32), hex::binary-size(len)>> -> Base.decode16!(hex, case: :mixed)
      end
    end
  end
end
