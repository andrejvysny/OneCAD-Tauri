/*
 * OperationOp → EditCommand wire mapping (SCHEMA §7.3 / src-tauri edit::command).
 *
 * The frontend authors high-level `OperationOp`s (Extrude / Fillet / Boolean).
 * The Rust `apply_edit_command` command consumes an `EditCommand` (serde tag
 * `"cmd"`, camelCase, camelCase fields) whose `AddOperation` variant carries a
 * full `OperationRecord`. The Rust deserializer DEFAULTS every record field
 * except `recordId` and the op's `{opType, params}` (verified against
 * `document/record.rs`), so a minimal real record is `{recordId, opType, params}`.
 *
 * ── Reference-id reconciliation (the key F-WP8 → M2 flag) ─────────────────────
 * Rust ids (`SketchId`/`BodyId`/`RegionId`/`ElementId`/`RecordId`) are
 * `#[serde(transparent)]` UUIDs. The refs an op needs come from different lanes:
 *   • BODY refs (Boolean target/tool) — arrive as REAL UUIDs on `document-changed`
 *     (`BodyMeshRef.bodyId`), so Boolean maps fully real. ✔
 *   • PARAMETRIC edit target (`featureId`) — a projection feature `id` IS the
 *     record's `RecordId` UUID, so an edit maps to `UpdateOperationParams`. ✔
 *   • SKETCH/REGION refs (Extrude profile) — come from the sketch SOLVER lane,
 *     which is LOCAL until R-WP12. Until then `sketchId`/`regionId` are local
 *     strings; the shape below is faithful but the backend rejects the non-UUID
 *     ids. R-WP12 makes them real → Extrude commit works with no shape change.
 *   • ELEMENT/EDGE refs (Fillet `edgeIds`) — are snapshot TopoKeys promoted to
 *     `ElementId` via `AcquireElementIds` (UNSUPPORTED in V1). Same story.
 *
 * The mapper therefore emits the STRUCTURALLY-REAL command every time; which ops
 * the live backend accepts today is purely a function of whether their input
 * lanes are wired (Boolean now; Extrude/Fillet with R-WP12 + AcquireElementIds).
 */
import type {
  BooleanParams,
  ExtrudeMode,
  ExtrudeParams,
  FeatureBooleanMode,
  FilletParams,
  OperationOp,
} from "./types";

/** A dimension value on the wire (Rust `Scalar {value, expr?}`). */
interface WireScalar {
  value: number;
}

interface WireExtrudeParams {
  profile?: { sketchId: string; regionId: string };
  distance: WireScalar;
  draftAngleDeg: WireScalar;
  extrudeMode: ExtrudeMode;
  booleanMode: FeatureBooleanMode;
  targetBodyId?: string;
  twoDirections: boolean;
  extrudeMode2: ExtrudeMode;
  distance2: WireScalar;
}

interface WireFilletParams {
  radius: WireScalar;
  edgeIds: string[];
  chainTangentEdges: boolean;
}

interface WireBooleanParams {
  operation: BooleanParams["operation"];
  targetBodyId: string;
  toolBodyId: string;
}

/** A known op on the wire — adjacently tagged `{opType, params}` (SCHEMA §7.3). */
type WireOperation =
  | { opType: "Extrude"; params: WireExtrudeParams }
  | { opType: "Fillet"; params: WireFilletParams }
  | { opType: "Boolean"; params: WireBooleanParams };

/** A minimal real `OperationRecord` (every other field defaults on the Rust side). */
interface WireOperationRecord {
  recordId: string;
  opType: WireOperation["opType"];
  params: WireOperation["params"];
}

/** The `EditCommand` variants this WP emits (serde tag `"cmd"`, camelCase). */
export type WireEditCommand =
  | { cmd: "addOperation"; record: WireOperationRecord; atCursor: boolean }
  | { cmd: "updateOperationParams"; record: string; op: WireOperation };

const scalar = (n: number): WireScalar => ({ value: n });

/** Mint a client-side record id (real UUID; V1 has no server-side pre-mint step). */
function mintRecordId(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  // Fallback (jsdom without randomUUID): RFC-4122 v4 from getRandomValues.
  const b = c.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0"));
  return `${h[0]}${h[1]}${h[2]}${h[3]}-${h[4]}${h[5]}-${h[6]}${h[7]}-${h[8]}${h[9]}-${h[10]}${h[11]}${h[12]}${h[13]}${h[14]}${h[15]}`;
}

function extrudeParams(p: ExtrudeParams): WireExtrudeParams {
  const wire: WireExtrudeParams = {
    distance: scalar(p.distance),
    draftAngleDeg: scalar(p.draftAngleDeg ?? 0),
    extrudeMode: p.extrudeMode ?? "Blind",
    booleanMode: p.booleanMode ?? "NewBody",
    twoDirections: p.twoDirections ?? false,
    extrudeMode2: p.extrudeMode2 ?? "Blind",
    distance2: scalar(p.distance2 ?? 0),
  };
  if (p.targetBodyId !== undefined) wire.targetBodyId = p.targetBodyId;
  return wire;
}

function filletParams(p: FilletParams): WireFilletParams {
  return {
    radius: scalar(p.radius),
    // Chamfer shares FilletChamferParams in C++, but the vertical-slice tool only
    // authors Fillet; a Chamfer would map to opType "Chamfer" (future).
    edgeIds: [...p.edgeIds],
    chainTangentEdges: p.chainTangentEdges ?? true,
  };
}

function booleanParams(p: BooleanParams): WireBooleanParams {
  return {
    operation: p.operation,
    targetBodyId: p.targetBodyId,
    toolBodyId: p.toolBodyId,
  };
}

/** Build the `{opType, params}` wire op for an OperationOp (no ids yet). */
function wireOperation(op: OperationOp): WireOperation {
  switch (op.opType) {
    case "Extrude": {
      const params = extrudeParams(op.params);
      // The profile is a SketchRegionRef; the ids are real once R-WP12 lands.
      if (op.sketchId && op.regionId) {
        params.profile = { sketchId: op.sketchId, regionId: op.regionId };
      }
      return { opType: "Extrude", params };
    }
    case "Fillet":
      return { opType: "Fillet", params: filletParams(op.params) };
    case "Boolean":
      return { opType: "Boolean", params: booleanParams(op.params) };
  }
}

/**
 * Map an OperationOp to the `EditCommand` payload for `apply_edit_command`.
 * A `featureId` (a projection feature's `RecordId`) re-targets an existing op via
 * `UpdateOperationParams`; otherwise a fresh op is appended via `AddOperation`.
 */
export function operationToEditCommand(op: OperationOp): WireEditCommand {
  const operation = wireOperation(op);
  if (op.featureId !== undefined) {
    return { cmd: "updateOperationParams", record: op.featureId, op: operation };
  }
  return {
    cmd: "addOperation",
    record: { recordId: mintRecordId(), ...operation },
    atCursor: false,
  };
}

/** Human label for a committed/undone op, for the status-bar hint. */
export function opLabelFor(op: OperationOp): string {
  return op.opType === "Boolean" ? op.params.operation : op.opType;
}
