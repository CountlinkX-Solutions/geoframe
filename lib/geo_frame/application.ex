defmodule GeoFrame.Application do
  @moduledoc false
  # Registers the Livebook Smart Cells when Kino is available and applies the configured datum
  # grid directories. GeoFrame itself needs no supervised processes.

  use Application

  @smart_cells [GeoFrame.Kino.ReadFileCell, GeoFrame.Kino.SpatialJoinCell]

  @impl true
  def start(_type, _args) do
    register_smart_cells()
    apply_grid_dirs()
    Supervisor.start_link([], strategy: :one_for_one, name: GeoFrame.Supervisor)
  end

  # `config :geoframe, grid_dirs: ["/data/proj"]`, see GeoFrame.Grids.
  defp apply_grid_dirs do
    case Application.get_env(:geoframe, :grid_dirs, []) do
      [] -> :ok
      dirs -> GeoFrame.Grids.set_dirs(List.wrap(dirs))
    end
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
