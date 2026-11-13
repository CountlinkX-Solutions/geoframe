// deck.gl map for GeoFrame.Kino.Deck. Geometry arrives as one binary payload of typed-array
// buffers (see the module docs); deck.gl reads them as binary attributes, so no per-feature
// JavaScript object is created. Libraries load from pinned jsDelivr URLs.
//
// deck.gl comes from its single-file UMD bundle, not from the per-package `+esm` builds: those
// draw correctly but picking returns nothing (no hover, no tooltips), so use the bundle.
const DECK_JS = "https://cdn.jsdelivr.net/npm/deck.gl@9.4.0/dist.min.js";
const MAPLIBRE_JS = "https://cdn.jsdelivr.net/npm/maplibre-gl@5.24.0/dist/maplibre-gl.js";
const MAPLIBRE_CSS = "https://cdn.jsdelivr.net/npm/maplibre-gl@5.24.0/dist/maplibre-gl.css";
const BLANK_STYLE = {
  version: 8,
  sources: {},
  layers: [{ id: "background", type: "background", paint: { "background-color": "#f3f4f6" } }],
};
const TYPES = { f64: Float64Array, i32: Int32Array, u32: Uint32Array, f32: Float32Array, u8: Uint8Array };

export async function init(ctx, [info, buffer]) {
  await ctx.importCSS(MAPLIBRE_CSS);
  await Promise.all([ctx.importJS(MAPLIBRE_JS), ctx.importJS(DECK_JS)]);
  const deckgl = globalThis.deck;

  ctx.root.style.cssText = `position:relative;height:${info.config.height}px;`;
  const container = document.createElement("div");
  container.style.cssText = "position:absolute;inset:0;";
  ctx.root.appendChild(container);

  // One group per layer. A payload from a single frame has no `groups`; it is one unnamed group
  // whose buffers carry no prefix.
  const groups = info.groups ?? [
    { name: null, prefix: "", layers: info.layers, tooltip: info.tooltip, popup: info.popup, legend: info.legend, config: info.config },
  ];
  const visible = groups.map(() => true);

  const [minx, miny, maxx, maxy] = info.bounds;
  const map = new maplibregl.Map({
    container,
    style: info.config.basemap || BLANK_STYLE,
    bounds: [[minx, miny], [maxx, maxy]],
    fitBoundsOptions: { padding: 40, maxZoom: 16 },
    attributionControl: { compact: true },
  });
  map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
  // Added after the attribution control, so MapLibre stacks the legend above it in the
  // bottom-right corner and the attribution stays visible.
  if (info.config.scale) {
    map.addControl(new maplibregl.ScaleControl({ unit: "metric" }), "bottom-left");
    map.addControl(new maplibregl.ScaleControl({ unit: "imperial" }), "bottom-left");
  }
  const legends = groups.map((group) => group.legend && legendBox(group.legend, groups.length > 1 ? group.name : null));
  if (legends.some(Boolean)) map.addControl(legendControl(legends), "bottom-right");

  const entries = []; // { layer, group } for every deck layer, in drawing order
  const byLayerId = {}; // deck layer id -> { group, rows }
  groups.forEach((group, g) => {
    const view = (name, type) => {
      const entry = info.buffers[group.prefix + name];
      return entry && new TYPES[type](buffer, entry.offset, entry.bytes / TYPES[type].BYTES_PER_ELEMENT);
    };
    const idPrefix = `geoframe-${group.prefix.replace(".", "-")}`;
    for (const layer of buildLayers(group, idPrefix, view, deckgl)) entries.push({ layer, g });
    for (const kind of ["points", "lines", "polygons"]) {
      byLayerId[`${idPrefix}${kind}`] = { group: g, rows: view(`${kind}.rows`, "i32") };
    }
  });

  const currentLayers = () => entries.map(({ layer, g }) => layer.clone({ visible: visible[g] }));
  const deckOverlay = new deckgl.MapLibreOverlay({
    interleaved: false,
    layers: currentLayers(),
    getTooltip: ({ index, layer }) => {
      const hit = byLayerId[layer?.id];
      return hit && tooltip(groups[hit.group].tooltip, hit.rows, index);
    },
    onClick: ({ index, layer, coordinate }) => {
      const hit = byLayerId[layer?.id];
      const data = hit && groups[hit.group].popup;
      if (data && coordinate) showPopup(map, data, hit.rows, index, coordinate, hit.group);
    },
  });
  map.addControl(deckOverlay);

  // The layer control: one checkbox per layer; hiding a layer also hides its legend and popup.
  if (groups.length > 1) {
    map.addControl(
      layerControl(groups, visible, (g, on) => {
        visible[g] = on;
        deckOverlay.setProps({ layers: currentLayers() });
        if (legends[g]) legends[g].style.display = on ? "" : "none";
        if (!on && popupGroup === g) closePopup();
      }),
      "top-left",
    );
  }
  map.once("load", () => map.resize());
}

function buildLayers(group, idPrefix, view, { ScatterplotLayer, PathLayer, SolidPolygonLayer }) {
  const { color, radius, width, opacity } = group.config;
  const out = [];
  // A fixed style is a layer prop (`getFillColor: [r, g, b, a]`); a per-feature style is a binary
  // attribute. A constant placed inside `data.attributes` is silently ignored by deck.gl.
  const attr = (kind, accessor, value, size, type, name) =>
    value == null ? { [accessor]: { value: view(`${kind}.${name}`, type), size } } : {};
  const prop = (accessor, value) => (value == null ? {} : { [accessor]: value });

  if (group.layers.polygons > 0) {
    const positions = view("polygons.positions", "f64");
    const ringStarts = view("polygons.ring_starts", "i32");
    // deck.gl wants the last vertex of every ring flagged invalid so rings do not connect.
    const vertexValid = new Uint16Array(positions.length / 2).fill(1);
    for (const start of ringStarts) if (start > 0) vertexValid[start - 1] = 0;
    out.push(
      new SolidPolygonLayer({
        id: `${idPrefix}polygons`,
        data: {
          length: group.layers.polygons,
          startIndices: view("polygons.starts", "i32"),
          attributes: {
            getPolygon: { value: positions, size: 2 },
            indices: view("polygons.triangles", "u32"),
            instanceVertexValid: { value: vertexValid, size: 1 },
            ...attr("polygons", "getFillColor", color, 4, "u8", "colors"),
          },
        },
        _normalize: false,
        ...prop("getFillColor", color),
        opacity,
        pickable: true,
        autoHighlight: true,
      }),
      new PathLayer({
        id: `${idPrefix}outlines`,
        data: {
          length: ringStarts.length - 1,
          startIndices: ringStarts,
          attributes: { getPath: { value: positions, size: 2 } },
        },
        _pathType: "open",
        getColor: [255, 255, 255, 200],
        getWidth: 1,
        widthUnits: "pixels",
        opacity,
      }),
    );
  }
  if (group.layers.lines > 0) {
    out.push(
      new PathLayer({
        id: `${idPrefix}lines`,
        data: {
          length: group.layers.lines,
          startIndices: view("lines.starts", "i32"),
          attributes: {
            getPath: { value: view("lines.positions", "f64"), size: 2 },
            ...attr("lines", "getColor", color, 4, "u8", "colors"),
            ...attr("lines", "getWidth", width, 1, "f32", "sizes"),
          },
        },
        ...prop("getColor", color),
        ...prop("getWidth", width),
        _pathType: "open",
        widthUnits: "pixels",
        capRounded: true,
        jointRounded: true,
        opacity,
        pickable: true,
        autoHighlight: true,
      }),
    );
  }
  if (group.layers.points > 0) {
    out.push(
      new ScatterplotLayer({
        id: `${idPrefix}points`,
        data: {
          length: group.layers.points,
          attributes: {
            getPosition: { value: view("points.positions", "f64"), size: 2 },
            ...attr("points", "getFillColor", color, 4, "u8", "colors"),
            ...attr("points", "getRadius", radius, 1, "f32", "sizes"),
          },
        },
        ...prop("getFillColor", color),
        ...prop("getRadius", radius),
        radiusUnits: "pixels",
        opacity,
        pickable: true,
        autoHighlight: true,
      }),
    );
  }
  return out;
}

function tooltip(data, rows, index) {
  if (!data || !rows || index < 0) return null;
  const row = rows[index];
  const text = data.columns.map((name, i) => `${name}: ${data.values[i][row] ?? "-"}`).join("\n");
  return { text, style: { fontSize: "12px", whiteSpace: "pre" } };
}

// -- popup --------------------------------------------------------------------------------
// A click on a feature opens a closable card at the clicked point. Attribute values are placed
// with `textContent`, never as HTML, so a value like "<img onerror=...>" is shown literally.

const POPUP_CSS = `
.gf-popup{font:12px/1.4 system-ui,sans-serif;color:#222;max-width:260px}
.gf-popup-row{display:flex;gap:8px;margin:1px 0}
.gf-popup-row b{flex:none;font-weight:600}
.gf-popup-row span{overflow-wrap:anywhere}
`;

let popup = null; // one popup at a time
let popupGroup = null; // the layer (group index) it belongs to

function closePopup() {
  if (popup) popup.remove();
  popup = null;
  popupGroup = null;
}

function popupContent(data, row) {
  const box = el("div", "gf-popup");
  box.append(el("style", null, POPUP_CSS));
  data.columns.forEach((name, i) => {
    const line = el("div", "gf-popup-row");
    line.append(el("b", null, `${name}:`), el("span", null, String(data.values[i][row] ?? "-")));
    box.append(line);
  });
  return box;
}

function showPopup(map, data, rows, index, coordinate, group) {
  closePopup();
  if (!rows || index < 0) return;
  popupGroup = group;
  popup = new maplibregl.Popup({ closeButton: true, closeOnClick: false, maxWidth: "280px" })
    .setLngLat(coordinate)
    .setDOMContent(popupContent(data, rows[index]))
    .addTo(map);
  popup.on("close", () => {
    popup = null;
    popupGroup = null;
  });
}

// -- legend -------------------------------------------------------------------------------
// The Elixir side decides what the legend says (see `legend` in the payload); this only draws it.

const LEGEND_CSS = `
.gf-legend{background:rgba(255,255,255,.92);color:#222;font:12px/1.35 system-ui,sans-serif;
  border-radius:6px;box-shadow:0 1px 4px rgba(0,0,0,.3);padding:8px 10px;max-width:220px;
  pointer-events:auto}
.gf-legends{display:flex;flex-direction:column;gap:6px}
.gf-legend-layer{font-weight:600;margin-bottom:2px}
.gf-legend-title{font-weight:600;margin-bottom:6px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.gf-legend-bar{height:10px;width:180px;border-radius:2px}
.gf-legend-ticks{position:relative;height:16px;width:180px;margin-top:2px}
.gf-legend-ticks span{position:absolute;top:0;transform:translateX(-50%);font-size:11px;white-space:nowrap}
.gf-legend-ticks span:first-child:not(:only-child){transform:none}
.gf-legend-ticks span:last-child:not(:only-child){transform:translateX(-100%)}
.gf-legend-row{display:flex;align-items:center;gap:6px;margin:2px 0}
.gf-legend-row i{display:inline-block;width:12px;height:12px;border-radius:2px;flex:none;
  border:1px solid rgba(0,0,0,.15)}
.gf-legend-row span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.gf-legend-more{color:#666;margin-top:2px}
`;

const rgb = ([r, g, b]) => `rgb(${r},${g},${b})`;

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function swatchRow(color, label) {
  const row = el("div", "gf-legend-row");
  const chip = el("i");
  chip.style.background = rgb(color);
  row.append(chip, el("span", null, label));
  return row;
}

// One card for one layer's legend; `layerName` heads it when several layers are drawn.
function legendBox(legend, layerName) {
  const box = el("div", "gf-legend");
  if (layerName != null) box.append(el("div", "gf-legend-layer", layerName));
  const title = el("div", "gf-legend-title", legend.title);
  title.title = legend.title;
  box.append(title);

  if (legend.kind === "gradient") {
    const bar = el("div", "gf-legend-bar");
    const stops = legend.stops.map((c, i) => `${rgb(c)} ${(i / (legend.stops.length - 1)) * 100}%`);
    bar.style.background = `linear-gradient(to right, ${stops.join(",")})`;
    const ticks = el("div", "gf-legend-ticks");
    for (const tick of legend.ticks) {
      const label = el("span", null, tick.label);
      label.style.left = `${tick.position * 100}%`;
      ticks.append(label);
    }
    box.append(bar, ticks);
  } else {
    for (const item of legend.items) box.append(swatchRow(item.color, item.label));
    if (legend.more > 0) box.append(el("div", "gf-legend-more", `${legend.more} more`));
  }
  if (legend.missing) box.append(swatchRow(legend.missing_color, "missing"));
  return box;
}

// The map control that stacks the layers' legend cards (null entries are layers without one).
function legendControl(boxes) {
  const wrap = el("div", "maplibregl-ctrl gf-legends");
  wrap.append(el("style", null, LEGEND_CSS), ...boxes.filter(Boolean));
  return { onAdd: () => wrap, onRemove: () => wrap.remove() };
}

// -- layer control ------------------------------------------------------------------------

const LAYERS_CSS = `
.gf-layers{background:rgba(255,255,255,.92);color:#222;font:12px/1.35 system-ui,sans-serif;
  border-radius:6px;box-shadow:0 1px 4px rgba(0,0,0,.3);padding:6px 10px;pointer-events:auto}
.gf-layers label{display:flex;align-items:center;gap:6px;margin:2px 0;cursor:pointer}
`;

function layerControl(groups, visible, onToggle) {
  const box = el("div", "maplibregl-ctrl gf-layers");
  box.append(el("style", null, LAYERS_CSS));
  groups.forEach((group, g) => {
    const label = el("label");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = visible[g];
    input.addEventListener("change", () => onToggle(g, input.checked));
    label.append(input, el("span", null, group.name));
    box.append(label);
  });
  return { onAdd: () => box, onRemove: () => box.remove() };
}
