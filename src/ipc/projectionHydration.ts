/*
 * projectionHydration — the `projection-updated` → documentStore bridge (F-WP8
 * flag 2). Per the plan the frontend owns the projection stores and they are
 * "written only by backend events"; this pure function is the write path.
 *
 * ── Revision reconciliation ───────────────────────────────────────────────────
 * A regen emits several projections around one edit (pre-regen, then post-regen),
 * and a tool may have applied an OPTIMISTIC result already. To avoid clobbering
 * newer state with a stale projection, a payload is applied ONLY when its revision
 * is >= the store's current revision (newer-or-equal authoritative wins; a stale
 * lower-revision projection is dropped). The empty projection (status "empty",
 * revision 0 — emitted on close) always resets the store.
 */
import { documentStore, type DocumentProjection, type SketchStatus } from "@/stores/documentStore";
import type { DocumentProjectionWire, FeatureRecord } from "./types";

/** Coerce a wire sketch status token to the store's `SketchStatus`. */
function sketchStatus(s: string): SketchStatus {
  return s === "ok" || s === "under" || s === "over" || s === "error" ? s : "under";
}

/** Map the wire projection to the store projection (field-identical shapes). */
export function projectionToStore(p: DocumentProjectionWire): DocumentProjection {
  const sketches: DocumentProjection["sketches"] = {};
  for (const [id, s] of Object.entries(p.sketches)) {
    sketches[id] = { id: s.id, name: s.name, visible: s.visible, dof: s.dof, status: sketchStatus(s.status) };
  }
  const features = p.features.map((f: FeatureRecord) => ({
    id: f.id,
    kind: f.kind,
    label: f.label,
    valueText: f.valueText,
    status: f.status,
  }));
  return {
    status: p.status,
    revision: p.revision,
    title: p.title,
    dirty: p.dirty,
    bodies: { ...p.bodies },
    sketches,
    features,
  };
}

/**
 * Apply an authoritative projection to `documentStore`, reconciling by revision.
 * The empty projection (close) always resets; otherwise a payload is written only
 * when it is newer-or-equal to the store's revision. Returns whether it applied.
 */
export function applyProjectionToStore(p: DocumentProjectionWire): boolean {
  const store = documentStore.getState();
  const isEmpty = p.status === "empty";
  if (!isEmpty && p.revision < store.revision) return false; // stale — drop
  store.applySnapshot(projectionToStore(p));
  return true;
}
