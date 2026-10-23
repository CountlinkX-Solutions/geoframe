defmodule GeoFrame.Application do
  @moduledoc false
  # Registers the Livebook Smart Cells when Kino is available. GeoFrame itself needs no
  # supervised processes.

  use Application

  @smart_cells [GeoFrame.Kino.ReadFileCell, GeoFrame.Kino.SpatialJoinCell]

  @impl true
  def start(_type, _args) do
    register_smart_cells()
    Supervisor.start_link([], strategy: :one_for_one, name: GeoFrame.Supervisor)
  end

  # The module held in a variable keeps the optional `Kino.SmartCell` out of compile-time checks.
  defp register_smart_cells do
    registry = Kino.SmartCell

    if Code.ensure_loaded?(registry) do
      for module <- @smart_cells, Code.ensure_loaded?(module), do: registry.register(module)
    end

    :ok
  end
end
