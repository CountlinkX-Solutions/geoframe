export function init(ctx, payload) {
  ctx.importCSS("main.css");

  const VARIABLE = /^[a-z_][a-zA-Z0-9_]*[?!]?$/;
  const HOWS = ["inner", "left", "right"];
  const state = { fields: payload.fields, variables: payload.variables };

  const options = (values) =>
    values.map((v) => `<option value="${escapeHtml(v)}">${escapeHtml(v)}</option>`).join("");

  ctx.root.innerHTML = `
    <div class="app">
      <div class="row">
        <div class="field">
          <label for="left">Left</label>
          <select class="input" id="left" data-field="left"></select>
        </div>
        <div class="field">
          <label for="right">Right</label>
          <select class="input" id="right" data-field="right"></select>
        </div>
        <div class="field">
          <label for="mode">Mode</label>
          <select class="input" id="mode" data-field="mode">
            <option value="predicate">predicate (sjoin)</option>
            <option value="nearest">nearest (sjoin_nearest)</option>
          </select>
        </div>
        <div class="field">
          <label for="how">How</label>
          <select class="input" id="how" data-field="how">${options(HOWS)}</select>
        </div>
      </div>
      <div class="row">
        <div class="field" data-mode="predicate">
          <label for="predicate">Predicate</label>
          <select class="input" id="predicate" data-field="predicate">${options(payload.predicates)}</select>
        </div>
        <div class="field" data-mode="predicate" id="distance-field">
          <label for="distance">Distance</label>
          <input class="input" id="distance" data-field="distance" autocomplete="off" placeholder="for dwithin" />
        </div>
        <div class="field" data-mode="nearest">
          <label for="max_distance">Max distance (optional)</label>
          <input class="input" id="max_distance" data-field="max_distance" autocomplete="off" />
        </div>
        <div class="field" data-mode="nearest">
          <label for="distance_col">Distance column (optional)</label>
          <input class="input" id="distance_col" data-field="distance_col" autocomplete="off" />
        </div>
      </div>
      <div class="row">
        <div class="field">
          <label for="result">Assign to</label>
          <input class="input" id="result" data-field="result" autocomplete="off" />
        </div>
        <label class="toggle">
          <input type="checkbox" id="show_map" data-field="show_map" />
          Show map
        </label>
      </div>
      <p class="hint" id="hint"></p>
    </div>
  `;

  const el = (field) => ctx.root.querySelector(`[data-field="${field}"]`);

  function renderFrames() {
    for (const field of ["left", "right"]) {
      const select = el(field);
      const names = state.variables.includes(state.fields[field]) || !state.fields[field]
        ? state.variables
        : [state.fields[field], ...state.variables];
      select.innerHTML = `<option value="">(select a GeoDataFrame)</option>` + options(names);
      select.value = state.fields[field];
    }
    ctx.root.querySelector("#hint").textContent = state.variables.length
      ? ""
      : "No GeoDataFrame variables found. Evaluate a cell that creates one.";
  }

  function render() {
    for (const field of Object.keys(state.fields)) {
      const input = el(field);
      if (!input || field === "left" || field === "right") continue;
      if (input.type === "checkbox") input.checked = state.fields[field];
      else if (document.activeElement !== input) input.value = state.fields[field];
    }
    const nearest = state.fields.mode === "nearest";
    for (const node of ctx.root.querySelectorAll("[data-mode]")) {
      node.classList.toggle("hidden", (node.dataset.mode === "nearest") !== nearest);
    }
    ctx.root
      .querySelector("#distance-field")
      .classList.toggle("hidden", nearest || state.fields.predicate !== "dwithin");
    el("result").classList.toggle("invalid", !VARIABLE.test(state.fields.result));
    renderFrames();
  }

  ctx.root.addEventListener("input", onChange);
  ctx.root.addEventListener("change", onChange);

  function onChange(event) {
    const field = event.target.dataset && event.target.dataset.field;
    if (!field) return;
    const value = event.target.type === "checkbox" ? event.target.checked : event.target.value;
    if (state.fields[field] === value) return;
    state.fields[field] = value;
    if (field === "mode" || field === "predicate" || field === "result") render();
    ctx.pushEvent("update_field", { field, value });
  }

  ctx.handleEvent("update_field", ({ fields }) => {
    Object.assign(state.fields, fields);
    render();
  });

  ctx.handleEvent("variables", ({ variables }) => {
    state.variables = variables;
    renderFrames();
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
