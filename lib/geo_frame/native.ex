defmodule GeoFrame.Native do
  @moduledoc false
  # NIF bindings to the `geoframe_native` Rust crate. Each function below is replaced
  # when the NIF library loads; the bodies only run if loading failed.
  use Rustler, otp_app: :geoframe, crate: "geoframe_native"

  def add(_a, _b), do: :erlang.nif_error(:nif_not_loaded)
end
