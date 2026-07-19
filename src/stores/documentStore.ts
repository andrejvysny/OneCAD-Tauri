/*
 * Document projection store (F-WP3 mock).
 *
 * Per plan, projection stores are "written only by backend events". For the
 * chrome-only WP there is no worker yet, so `seedMockDocument()` stands in for
 * the first `applySnapshot`, and `applySnapshot` / `applyChange` are the write
 * path a later IPC WP will call. Shape mirrors the Rust projection DTO the plan
 * describes (bodies / sketches registries + a linear feature timeline).
 */
import { createStore, useStore } from "zustand";

export type DocStatus = "empty" | "loading" | "ready";

export interface BodyMeta {
  id: string;
  name: string;
  visible: boolean;
}

/** 'under' = under-constrained, 'over' = over-constrained (mock statuses). */
export type SketchStatus = "ok" | "under" | "over" | "error";

export interface SketchMeta {
  id: string;
  name: string;
  visible: boolean;
  dof: number;
  status: SketchStatus;
}

export type FeatureKind =
  | "sketch"
  | "extrude"
  | "revolve"
  | "fillet"
  | "boolean"
  | "shell"
  | "linearPattern"
  | "circularPattern"
  | "mirror";

export type FeatureStatus = "ok" | "dirty" | "error" | "needsRepair";

export interface FeatureMeta {
  id: string;
  kind: FeatureKind;
  label: string;
  /** Mono value shown on the right of the history chip, e.g. "83.3 mm". */
  valueText: string;
  status: FeatureStatus;
}

/** The full document projection (everything the chrome renders from). */
export interface DocumentProjection {
  status: DocStatus;
  revision: number;
  title: string;
  dirty: boolean;
  bodies: Record<string, BodyMeta>;
  sketches: Record<string, SketchMeta>;
  features: FeatureMeta[];
}

export interface DocumentState extends DocumentProjection {
  /** Replace the whole projection (backend snapshot event). */
  applySnapshot(snapshot: DocumentProjection): void;
  /** Merge a partial projection delta (backend change event). */
  applyChange(change: Partial<DocumentProjection>): void;
  /** Local visibility flip for a body or sketch (drives the tree eye toggle). */
  setVisibility(id: string, visible: boolean): void;
  /** Register a sketch (e.g. a freshly created one) in the tree/registry. */
  addSketch(meta: SketchMeta): void;
  /** Push a solver result onto a sketch (drives chrome bar + inspector DOF). */
  setSketchSolve(id: string, dof: number, status: SketchStatus): void;
}

/** Map the wire solver state (SCHEMA §7.4) → the tree/inspector status. */
export function docSketchStatus(
  state: "UnderConstrained" | "FullyConstrained" | "OverConstrained" | "Conflicting",
): SketchStatus {
  switch (state) {
    case "FullyConstrained":
      return "ok";
    case "OverConstrained":
      return "over";
    case "Conflicting":
      return "error";
    default:
      return "under";
  }
}

/**
 * Bracket-like demo document. The body/sketch registries mirror prototype 1c's
 * tree exactly (Body 1; Sketch 2 / 4 / 5) so the flagship screen renders
 * pixel-faithfully, while `features` carries the full timeline the plan lists
 * (Sketch 1 → Extrude 83.3 → Fillet 2.0 → Sketch 2 → Extrude 12.0 — two bodies
 * and two sketches at the history level). Values match the prototype inspector
 * HISTORY verbatim (83.3 mm / 2.0 mm / 12.0 mm).
 */
export function seedMockDocument(): DocumentProjection {
  return {
    status: "ready",
    revision: 5,
    title: "Bracket v2",
    dirty: false,
    bodies: {
      body1: { id: "body1", name: "Body 1", visible: true },
    },
    sketches: {
      sketch2: { id: "sketch2", name: "Sketch 2", visible: true, dof: 3, status: "under" },
      sketch4: { id: "sketch4", name: "Sketch 4", visible: true, dof: 0, status: "ok" },
      sketch5: { id: "sketch5", name: "Sketch 5", visible: false, dof: 0, status: "ok" },
    },
    features: [
      { id: "f1", kind: "sketch", label: "Sketch 1", valueText: "", status: "ok" },
      { id: "f2", kind: "extrude", label: "Extrude", valueText: "83.3 mm", status: "ok" },
      { id: "f3", kind: "fillet", label: "Fillet", valueText: "2.0 mm", status: "ok" },
      { id: "f4", kind: "sketch", label: "Sketch 2", valueText: "", status: "ok" },
      { id: "f5", kind: "extrude", label: "Extrude", valueText: "12.0 mm", status: "ok" },
    ],
  };
}

/** The "no document open" projection (mirrors Rust `DocumentProjection::empty`). */
export function emptyDocument(): DocumentProjection {
  return {
    status: "empty",
    revision: 0,
    title: "",
    dirty: false,
    bodies: {},
    sketches: {},
    features: [],
  };
}

/**
 * The store's initial projection. Under a real Tauri webview the app starts EMPTY
 * and hydrates from the backend's first `projection-updated` (no mock document);
 * in a plain browser / vitest / Playwright there is no backend, so the
 * `seedMockDocument()` demo drives the whole UI (tests depend on the seed).
 */
function initialDocument(): DocumentProjection {
  const underTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  return underTauri ? emptyDocument() : seedMockDocument();
}

export const documentStore = createStore<DocumentState>()((set) => ({
  ...initialDocument(),

  applySnapshot(snapshot) {
    set(snapshot);
  },

  applyChange(change) {
    set(change);
  },

  setVisibility(id, visible) {
    set((s) => {
      if (s.bodies[id]) {
        return { bodies: { ...s.bodies, [id]: { ...s.bodies[id], visible } } };
      }
      if (s.sketches[id]) {
        return { sketches: { ...s.sketches, [id]: { ...s.sketches[id], visible } } };
      }
      return {};
    });
  },

  addSketch(meta) {
    set((s) => ({ sketches: { ...s.sketches, [meta.id]: meta } }));
  },

  setSketchSolve(id, dof, status) {
    set((s) => {
      const sketch = s.sketches[id];
      if (!sketch) return {};
      return { sketches: { ...s.sketches, [id]: { ...sketch, dof, status } } };
    });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useDocumentStore<T>(selector: (s: DocumentState) => T): T {
  return useStore(documentStore, selector);
}
