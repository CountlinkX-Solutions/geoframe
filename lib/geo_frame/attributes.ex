defmodule GeoFrame.Attributes do
  @moduledoc false
  # The attribute half of every file format: an Arrow IPC stream moved between an
  # `Explorer.DataFrame` and the NIF (one serialisation and one copy per direction).

  alias Explorer.DataFrame

  @spec load(binary()) :: {:ok, DataFrame.t()} | {:error, String.t()}
  def load(ipc) do
    case DataFrame.load_ipc_stream(ipc) do
      {:ok, data} -> {:ok, data}
      {:error, reason} -> {:error, "cannot load attribute columns: #{error_message(reason)}"}
    end
  end

  @spec dump(DataFrame.t()) :: {:ok, binary()} | {:error, String.t()}
  def dump(data) do
    case DataFrame.dump_ipc_stream(data) do
      {:ok, ipc} -> {:ok, ipc}
      {:error, reason} -> {:error, "cannot serialise attribute columns: #{error_message(reason)}"}
    end
  end

  defp error_message(reason) when is_binary(reason), do: reason
  defp error_message(%{__exception__: true} = error), do: Exception.message(error)
  defp error_message(reason), do: inspect(reason)
end
