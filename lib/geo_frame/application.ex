defmodule GeoFrame.Application do
  @moduledoc false
  # Registers the Livebook Smart Cells when Kino is available and applies the configured datum
  # grid directories. GeoFrame itself needs no supervised processes.

  use Application

  # Kino is optional. Elixir 1.19 also checks a remote call made through a variable that holds
  # the module, so this attribute is what keeps `register_smart_cells/0` from warning without it.
  @compile {:no_warn_undefined, Kino.SmartCell}

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

  defp register_smart_cells do
    if Code.ensure_loaded?(Kino.SmartCell) do
      for module <- @smart_cells, Code.ensure_loaded?(module), do: Kino.SmartCell.register(module)
    end

    :ok
  end
end
