defmodule GeoFrame.NativeTest do
  use ExUnit.Case, async: true

  test "the NIF library loads" do
    assert GeoFrame.Native.add(1, 2) == 3
  end
end
