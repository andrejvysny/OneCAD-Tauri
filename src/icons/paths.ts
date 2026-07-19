/*
 * OneCAD icon path table.
 *
 * Every `d` string below is extracted VERBATIM from the design prototype:
 *   design/project/OneCAD UI Explorations.dc.html
 *
 * Provenance (prototype line numbers):
 *   - Model toolbar tools .......... static MD table, lines 503-511
 *   - Sketch toolbar tools ......... static SK table, lines 512-523
 *   - Named glyphs (pen/cube/...) .. static ICONS table, lines 497-502
 *   - Inline chrome/start SVGs ..... individual lines cited per entry below
 *
 * Notes on intentional duplicates (kept as distinct keys because the prototype
 * uses them under distinct names / the migration plan references them by name):
 *   - `pen` === `sketch`   (ICONS.pen is the New-sketch tool glyph)
 *   - `display` === `cube` (the display-mode button reuses the cube hexagon)
 *
 * `eye-off` is intentionally ABSENT: the prototype has no eye-off glyph. Hidden
 * state is expressed by lowering the opacity of the single `eye` icon
 * (see EyeToggle / prototype line 187, opacity 0.85 on ↔ 0.3 off). Inventing an
 * eye-off path would violate the "icons verbatim" contract.
 */

export const ICON_PATHS = {
  // ---- Model toolbar tools (MD table, lines 503-511) ----
  select: "M5.5 3.5l6.5 16 2.2-6.8 6.8-2.2z",
  sketch: "M4.5 19.5l1.2-4L15.5 5.7l2.8 2.8L8.5 18.3l-4 1.2zM13.5 7.7l2.8 2.8",
  extrude: "M12 10V3.5M9.2 5.8L12 3.5l2.8 2.3M5.5 13.5h13v6.5h-13z",
  revolve: "M19.3 13a7.3 7.3 0 1 1-2-6M19.5 4v3.5H16",
  fillet: "M4.5 19.5V11a6.5 6.5 0 0 1 6.5-6.5h8.5",
  boolean: "M14 12a4.3 4.3 0 1 1-8.6 0 4.3 4.3 0 0 1 8.6 0zM18.6 12a4.3 4.3 0 1 1-8.6 0 4.3 4.3 0 0 1 8.6 0z",

  // ---- Sketch toolbar tools (SK table, lines 512-523) ----
  line: "M5 19L19 5",
  rect: "M5 6.5h14v11H5z",
  circle: "M19 12a7 7 0 1 1-14 0 7 7 0 0 1 14 0z",
  arc: "M5 19A14 14 0 0 1 19 5",
  dimension: "M4 12h16M4 8.5v7M20 8.5v7",
  trim: "M8 7a2 2 0 1 1-4 0 2 2 0 0 1 4 0zM8 17a2 2 0 1 1-4 0 2 2 0 0 1 4 0zM7.5 8.4L19 19M7.5 15.6L19 5",
  mirror: "M12 4v16M8.5 8L5 12l3.5 4M15.5 8L19 12l-3.5 4",

  // ---- Named glyphs (ICONS table, lines 497-502) ----
  pen: "M4.5 19.5l1.2-4L15.5 5.7l2.8 2.8L8.5 18.3l-4 1.2zM13.5 7.7l2.8 2.8", // === sketch
  cube: "M12 3l7.5 4.3v9.4L12 21l-7.5-4.3V7.3L12 3zM12 11.6l7.5-4.3M12 11.6L4.5 7.3M12 11.6V21",

  // ---- Inline chrome / start-screen SVGs ----
  plus: "M12 5v14M5 12h14", // New project (line 52)
  import: "M12 4v9M8.5 10L12 13.5 15.5 10M5 16v3.5h14V16", // Import STEP (line 54)
  search: "M15.5 15.5L20 20M11 17a6 6 0 1 1 0-12 6 6 0 0 1 0 12z", // search (line 60)
  chevronDown: "M6 9l6 6 6-6", // sort dropdown (line 64)
  clock: "M12 8v4l2.5 2.5M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z", // Recent (line 101)
  star: "M12 4l2.4 4.9 5.4.8-3.9 3.8.9 5.4-4.8-2.5-4.8 2.5.9-5.4L4.2 9.7l5.4-.8L12 4z", // Starred (line 102)
  template: "M4.5 7.5h15v12h-15zM8 7.5V5h8v2.5", // Templates (line 103)
  eye: "M2.5 12S6 6.5 12 6.5 21.5 12 21.5 12 18 17.5 12 17.5 2.5 12 2.5 12zM14.4 12a2.4 2.4 0 1 1-4.8 0 2.4 2.4 0 0 1 4.8 0z", // eye (line 187)
  x: "M6 6l12 12M18 6L6 18", // Cancel (line 176)
  check: "M5 12.5l4.5 4.5L19 7", // Finish sketch (line 177)
  display: "M12 3l7.5 4.3v9.4L12 21l-7.5-4.3V7.3L12 3zM12 11.6l7.5-4.3M12 11.6L4.5 7.3M12 11.6V21", // display mode (line 241) === cube
  grid: "M4.5 4.5h15v15h-15zM9.5 4.5v15M14.5 4.5v15M4.5 9.5h15M4.5 14.5h15", // grid (line 242)
  snap: "M6 3.5h4.5V11a1.5 1.5 0 0 0 3 0V3.5H18V11a6 6 0 0 1-12 0V3.5zM6 7h4.5M13.5 7H18", // magnet (line 243)
  home: "M4.5 10.5L12 4l7.5 6.5M6.5 9.5V20h11V9.5", // home view (line 272)
  fit: "M4 9V4.5h4.5M20 9V4.5h-4.5M4 15v4.5h4.5M20 15v4.5h-4.5", // zoom to fit (line 273)
  layers: "M12 4l8 4.2-8 4.2-8-4.2L12 4zM4.5 12.5L12 16.4l7.5-3.9M4.5 16.5L12 20.4l7.5-3.9", // view presets (line 274)
  penEdit: "M4.5 19.5l1.2-4L15.5 5.7l2.8 2.8L8.5 18.3l-4 1.2z", // "Editing sketch" badge, pen body only (line 173)

  // ---- M6b model-op tools (authored for this WP, NOT from the prototype) ----
  // Same conventions as the prototype glyphs: 24×24 grid, single stroked path,
  // fill none, round caps/joins, weights read cleanly at 14–15px.
  shell: "M4.5 7.5h15v12h-15zM8 7.5v8.5h8v-8.5", // hollow box, open top (removed face)
  linearPattern: "M4 8h4v4H4zM10 8h4v4h-4zM16 8h4v4h-4z", // three instances in a row
  circularPattern: "M18 12a6 6 0 1 1-12 0 6 6 0 0 1 12 0zM11 5h2v2h-2zM5.8 14h2v2h-2zM16.2 14h2v2h-2z", // instances around a circle
  mirrorBody: "M12 4v16M4 9h4v6H4zM16 9h4v6h-4z", // a body + its reflection across a plane
} as const;

export type IconName = keyof typeof ICON_PATHS;
