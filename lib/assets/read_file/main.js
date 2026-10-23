export function init(ctx, payload) {
  ctx.importCSS("main.css");

  const VARIABLE = /^[a-z_][a-zA-Z0-9_]*[?!]?$/;
  const state = { fields: payload.fields, layers: payload.layers };

  const driverOptions = ["auto", ...payload.drivers]
    .map((d) => `<option value="${d}">${d}</option>`)
    .join("");

  ctx.root.innerHTML = `
    <div class="app">
      <div class="row">
        <div class="field">
          <label for="variable">Assign to</label>
          <input class="input" id="variable" data-field="variable" autocomplete="off" />
        </div>
        <div class="field wide">
          <label for="path">Path</label>
          <input class="input" id="path" data-field="path" autocomplete="off"
                 placeholder="data/places.gpkg" />
        </div>
        <div class="field">
          <label for="driver">Driver</label>
          <select class="input" id="driver" data-field="driver">${driverOptions}</select>
        </div>
        <div class="field" id="layer-field">
          <label for="layer">Layer</label>
          <select class="input" id="layer" data-field="layer"></select>
        </div>
      </div>
      <div class="row">
        <div class="field wide">
          <label for="bbox">Bounding box (optional)</label>
          <input class="input" id="bbox" data-field="bbox" autocomplete="off"
                 placeholder="minx, miny, maxx, maxy" />
        </div>
        <div class="field">
          <label for="to_crs">Reproject to (optional)</label>
          <input class="input" id="to_crs" data-field="to_crs" autocomplete="off"
                 placeholder="EPSG:3857" />
        </div>
        <label class="toggle">
          <input type="checkbox" id="show_map" data-field="show_map" />
          Show map
        </label>
      </div>
      <p class="hint">The bounding box filter works for GeoParquet and FlatGeobuf only.</p>
    </div>
  `;

  const el = (field) => ctx.root.querySelector(`[data-field="${field}"]`);
  const layerField = ctx.root.querySelector("#layer-field");

  function renderLayers() {
    const select = el("layer");
    select.innerHTML = state.layers
      .map((name) => `<option value="${escapeHtml(name)}">${escapeHtml(name)}</option>`)
      .join("");
    select.value = state.fields.layer;
    layerField.classList.toggle("hidden", state.layers.length === 0);
  }

  function render() {
    for (const field of Object.keys(state.fields)) {
      const input = el(field);
      if (!input || field === "layer") continue;
      if (input.type === "checkbox") input.checked = state.fields[field];
      else if (document.activeElement !== input) input.value = state.fields[field];
    }
    el("variable").classList.toggle("invalid", !VARIABLE.test(state.fields.variable));
    renderLayers();
  }

  ctx.root.addEventListener("input", onChange);
  ctx.root.addEventListener("change", onChange);

  function onChange(event) {
    const field = event.target.dataset && event.target.dataset.field;
    if (!field) return;
    const value = event.target.type === "checkbox" ? event.target.checked : event.target.value;
    if (state.fields[field] === value) return;
    state.fields[field] = value;
    if (field === "variable") el("variable").classList.toggle("invalid", !VARIABLE.test(value));
    ctx.pushEvent("update_field", { field, value });
  }

  ctx.handleEvent("update_field", ({ fields, layers }) => {
    Object.assign(state.fields, fields);
    if (layers) state.layers = layers;
    render();
  });

  ctx.handleSync(() => {
    const active = document.activeElement;
    if (active && ctx.root.contains(active)) active.dispatchEvent(new Event("change", { bubbles: true }));
  });

  render();
}

function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}
