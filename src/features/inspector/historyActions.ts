/*
 * History + repair edit actions (M4b) — the imperative glue from the inspector's
 * history rows + repair panel to the raw `EditCommand` surface.
 *
 * Each action dispatches ONE `client.applyEditCommand(...)` (or, for a rebind, a
 * promote_selection followed by an EditOperationInput) and hydrates the document
 * store from the correlated regen result (mirrors ModelToolController.applyResult).
 * Errors surface through the StatusBar hint (viewportStore.setStatusHint).
 */
import { createClient } from "@/ipc/client";
import {
  edgeElementRef,
  filletEdgeRebindCommand,
  removeOperationCommand,
  rollbackToCursorCommand,
  suppressOperationCommand,
} from "@/ipc/tauriCommandMap";
import type { ApplyOperationResult, NeedsRepairItem, ResolveCandidate } from "@/ipc/types";
import { parseRefId } from "@/ipc/tauriCommandMap";
import { documentStore, type FeatureMeta } from "@/stores/documentStore";
import { historyStore } from "@/stores/historyStore";
import { viewportStore } from "@/stores/viewportStore";

/** Hydrate the document store from a regen result (bodies + feature timeline). */
function applyEditResult(res: ApplyOperationResult): void {
  const doc = documentStore.getState();
  const bodies = { ...doc.bodies };
  let n = Object.keys(bodies).length;
  for (const ref of res.changedBodies ?? []) {
    if (!bodies[ref.bodyId]) bodies[ref.bodyId] = { id: ref.bodyId, name: `Body ${++n}`, visible: true };
  }
  for (const id of res.removedBodies ?? []) delete bodies[id];
  doc.applyChange({
    revision: res.revision,
    features: res.features.map(toFeatureMeta),
    bodies,
    dirty: true,
  });
}

function toFeatureMeta(f: {
  id: string;
  kind: FeatureMeta["kind"];
  label: string;
  valueText: string;
  status: FeatureMeta["status"];
}): FeatureMeta {
  return { id: f.id, kind: f.kind, label: f.label, valueText: f.valueText, status: f.status };
}

function hint(text: string): void {
  viewportStore.getState().setStatusHint(text);
}

function errMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ── History-row affordances ────────────────────────────────────────────────

/** Suppress / un-suppress a feature (optimistic dim + `SetOperationSuppression`). */
export async function suppressFeature(opId: string, suppressed: boolean): Promise<void> {
  historyStore.getState().setSuppressed(opId, suppressed); // optimistic
  try {
    const res = await createClient().applyEditCommand(suppressOperationCommand(opId, suppressed));
    applyEditResult(res);
    hint(suppressed ? "Feature suppressed" : "Feature unsuppressed");
  } catch (e) {
    historyStore.getState().setSuppressed(opId, !suppressed); // revert optimistic
    hint(`Suppress failed: ${errMessage(e)}`);
  }
}

/**
 * Roll the timeline to a row (`SetRollback`). Cursor = applied op count, so
 * "roll to here" = `index + 1` (timeline.rs); rolling to the LAST row therefore
 * restores full history (cursor == length).
 */
export async function rollToIndex(index: number): Promise<void> {
  try {
    const res = await createClient().applyEditCommand(rollbackToCursorCommand(index + 1));
    applyEditResult(res);
    hint("Rolled timeline");
  } catch (e) {
    hint(`Rollback failed: ${errMessage(e)}`);
  }
}

/** Delete a feature permanently (`RemoveOperation`). */
export async function deleteFeature(opId: string): Promise<void> {
  try {
    const res = await createClient().applyEditCommand(removeOperationCommand(opId));
    applyEditResult(res);
    historyStore.getState().setSuppressed(opId, false); // drop any stale overlay
    hint("Feature deleted");
  } catch (e) {
    hint(`Delete failed: ${errMessage(e)}`);
  }
}

// ── Click-to-rebind (repair) ────────────────────────────────────────────────

/**
 * Derive the body a repair item's feature operated on. SEAM: the projection has
 * no feature→body linkage, so with a single body we use it; with several the
 * operated body is ambiguous and we fall back to the first (dev warn). A follow-up
 * needs the needs-repair item to carry its op's target body.
 */
function deriveOperatedBody(): string | null {
  const ids = Object.keys(documentStore.getState().bodies);
  if (ids.length === 0) return null;
  if (ids.length > 1 && import.meta.env?.DEV) {
    // eslint-disable-next-line no-console
    console.warn("[repair] ambiguous operated body (>1 body); using the first");
  }
  return ids[0];
}

/**
 * Rebind a NeedsRepair fillet edge to a chosen candidate:
 *   (a) promote the candidate TopoKey → a minted ElementId (anchor = worldPos),
 *   (b) build the typed edge ElementRef (primary {bodyId, elementId, kind:"edge"}
 *       + anchor.worldPoint), and
 *   (c) send `EditOperationInput{FilletEdges{index}}` (the backend-designated
 *       fillet-edge rebind — it rewrites BOTH `edge_ids[index]` and `edges[index]`
 *       in lockstep server-side; command.rs). `index` comes from the refId
 *       (`"<opId>.input<k>"`).
 * Returns false when it could not proceed (no body / already in flight upstream).
 */
export async function rebindCandidate(
  item: NeedsRepairItem,
  candidate: ResolveCandidate,
): Promise<boolean> {
  const bodyId = deriveOperatedBody();
  if (!bodyId) {
    hint("Cannot repair: no body to bind against");
    return false;
  }
  const index = parseRefId(item.refId)?.index ?? 0;
  const client = createClient();
  try {
    const [promoted] = await client.promoteSelection(bodyId, [
      { topoKey: candidate.topoKey, anchor: { worldPoint: candidate.worldPos } },
    ]);
    if (!promoted) {
      hint("Repair failed: could not promote candidate");
      return false;
    }
    const ref = edgeElementRef(promoted.bodyId, promoted.elementId, candidate.worldPos);
    const res = await client.applyEditCommand(filletEdgeRebindCommand(item.opId, index, ref));
    applyEditResult(res);
    hint("Reference repaired");
    return true;
  } catch (e) {
    hint(`Repair failed: ${errMessage(e)}`);
    return false;
  }
}
