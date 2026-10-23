// deck.gl map for GeoFrame.Kino.Deck. Geometry arrives as one binary payload of typed-array
// buffers (see the module docs); deck.gl reads them as binary attributes, so no per-feature
// JavaScript object is created. Libraries load from pinned jsDelivr URLs.
const DECK = "https://cdn.jsdelivr.net/npm/@deck.gl/core@9.4.0/+esm";
const LAYERS = "https://cdn.jsdelivr.net/npm/@deck.gl/layers@9.4.0/+esm";
const OVERLAY = "https://cdn.jsdelivr.net/npm/@deck.gl/maplibre@9.4.0/+esm";
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
  const [, layers, overlay] = await Promise.all([ctx.importJS(MAPLIBRE_JS), import(LAYERS), import(OVERLAY)]);

  // A typed array over a named buffer, or undefined when the layer has none.
  const view = (name, type) => {
    const entry = info.buffers[name];
    return entry && new TYPES[type](buffer, entry.offset, entry.bytes / TYPES[type].BYTES_PER_ELEMENT);
  };

  ctx.root.style.cssText = `position:relative;height:${info.config.height}px;`;
  const container = document.createElement("div");
  container.style.cssText = "position:absolute;inset:0;";
  ctx.root.appendChild(container);

  const [minx, miny, maxx, maxy] = info.bounds;
  const map = new maplibregl.Map({
    container,
    style: info.config.basemap || BLANK_STYLE,
    bounds: [[minx, miny], [maxx, maxy]],
    fitBoundsOptions: { padding: 40, maxZoom: 16 },
    attributionControl: { compact: true },
  });
  map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");

  const deckLayers = buildLayers(info, view, layers);
  const rowsOf = {}; // layer id -> source row of each feature
  for (const kind of ["points", "lines", "polygons"]) rowsOf[`geoframe-${kind}`] = view(`${kind}.rows`, "i32");

  const deckOverlay = new overlay.MapLibreOverlay({
    interleaved: false,
    layers: deckLayers,
    getTooltip: ({ index, layer }) => tooltip(info.tooltip, rowsOf[layer?.id], index),
  });
  map.addControl(deckOverlay);
  map.once("load", () => map.resize());
}

function buildLayers(info, view, { ScatterplotLayer, PathLayer, SolidPolygonLayer }) {
  const { color, radius, width, opacity } = info.config;
  const out = [];
  // Fixed styles are plain props; per-feature styles are binary attributes.
  const style = (kind, accessor, value, size, type, name) =>
    value == null ? { [accessor]: { value: view(`${kind}.${name}`, type), size } } : { [accessor]: value };

  if (info.layers.polygons > 0) {
    const positions = view("polygons.positions", "f64");
    const ringStarts = view("polygons.ring_starts", "i32");
    // deck.gl wants the last vertex of every ring flagged invalid so rings do not connect.
    const vertexValid = new Uint16Array(positions.length / 2).fill(1);
    for (const start of ringStarts) if (start > 0) vertexValid[start - 1] = 0;
    out.push(
      new SolidPolygonLayer({
        id: "geoframe-polygons",
        data: {
          length: info.layers.polygons,
          startIndices: view("polygons.starts", "i32"),
          attributes: {
            getPolygon: { value: positions, size: 2 },
            indices: view("polygons.triangles", "u32"),
            instanceVertexValid: { value: vertexValid, size: 1 },
            ...style("polygons", "getFillColor", color, 4, "u8", "colors"),
          },
        },
        _normalize: false,
        opacity,
        pickable: true,
        autoHighlight: true,
      }),
      new PathLayer({
        id: "geoframe-outlines",
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
  if (info.layers.lines > 0) {
    out.push(
      new PathLayer({
        id: "geoframe-lines",
        data: {
          length: info.layers.lines,
          startIndices: view("lines.starts", "i32"),
          attributes: {
            getPath: { value: view("lines.positions", "f64"), size: 2 },
            ...style("lines", "getColor", color, 4, "u8", "colors"),
            ...style("lines", "getWidth", width, 1, "f32", "sizes"),
          },
        },
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
  if (info.layers.points > 0) {
    out.push(
      new ScatterplotLayer({
        id: "geoframe-points",
        data: {
          length: info.layers.points,
          attributes: {
            getPosition: { value: view("points.positions", "f64"), size: 2 },
            ...style("points", "getFillColor", color, 4, "u8", "colors"),
            ...style("points", "getRadius", radius, 1, "f32", "sizes"),
          },
        },
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
