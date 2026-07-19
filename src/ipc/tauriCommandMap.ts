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
  AxisRef,
  BooleanParams,
  ExtrudeMode,
  ExtrudeParams,
  FeatureBooleanMode,
  FilletParams,
  OperationOp,
  RevolveParams,
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

/** Rust `AxisRef` (serde internally-tagged on `kind`, camelCase fields). */
type WireAxisRef =
  | { kind: "sketchLine"; sketchId: string; lineId: string }
  | { kind: "edge"; bodyId: string; edgeId: string };

interface WireRevolveParams {
  profile?: { sketchId: string; regionId: string };
  /** Rust `angleDeg` Scalar — DEGREES (no radians conversion). */
  angleDeg: WireScalar;
  axis?: WireAxisRef;
  booleanMode: FeatureBooleanMode;
  targetBodyId?: string;
}

interface WireFilletParams {
  radius: WireScalar;
  edgeIds: string[];
  /**
   * Typed per-edge semantic refs (Rust `FilletParams::edges` — one `ElementRef`
   * per `edgeIds` entry). CRITICAL: `edgeIds` (bare) and `edges` (typed) MUST stay
   * in lockstep — any command that rewrites one rewrites BOTH (record.rs FilletParams
   * / the M4b dual-edge rule). Optional so a legacy/bare-id fillet still marshals.
   */
  edges?: WireElementRef[];
  chainTangentEdges: boolean;
}

/** Rust `ElementRef` (refs.rs — identity + evidence + anchor; camelCase). */
export interface WireElementRef {
  primary?: { bodyId: string; elementId: string; kind: "face" | "edge" | "vertex" };
  anchor?: { worldPoint: [number, number, number]; surfaceUv?: [number, number] };
}

interface WireBooleanParams {
  operation: BooleanParams["operation"];
  targetBodyId: string;
  toolBodyId: string;
}

/** A known op on the wire — adjacently tagged `{opType, params}` (SCHEMA §7.3). */
type WireOperation =
  | { opType: "Extrude"; params: WireExtrudeParams }
  | { opType: "Revolve"; params: WireRevolveParams }
  | { opType: "Fillet"; params: WireFilletParams }
  | { opType: "Boolean"; params: WireBooleanParams };

/** A minimal real `OperationRecord` (every other field defaults on the Rust side). */
interface WireOperationRecord {
  recordId: string;
  opType: WireOperation["opType"];
  params: WireOperation["params"];
}

/**
 * A typed input-slot path (Rust `InputPath`, internally tagged on `"path"`,
 * camelCase). Only the fillet-edge arm is authored by M4b.
 */
export type WireInputPath = { path: "filletEdges"; index: number };

/**
 * The payload of an `EditOperationInput` (Rust `InputRef`, externally tagged,
 * camelCase). M4b authors only the `element` arm (fillet/chamfer edge rebind).
 */
export type WireInputRef = { element: WireElementRef };

/** The `EditCommand` variants this WP emits (serde tag `"cmd"`, camelCase). */
export type WireEditCommand =
  | { cmd: "addOperation"; record: WireOperationRecord; atCursor: boolean }
  | { cmd: "updateOperationParams"; record: string; op: WireOperation }
  | { cmd: "editOperationInput"; record: string; path: WireInputPath; reference: WireInputRef }
  | { cmd: "removeOperation"; record: string }
  | { cmd: "setRollback"; cursor: number }
  | { cmd: "setOperationSuppression"; record: string; suppressed: boolean; cascade: boolean };

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

function axisRef(a: AxisRef): WireAxisRef {
  return a.kind === "sketchLine"
    ? { kind: "sketchLine", sketchId: a.sketchId, lineId: a.lineId }
    : { kind: "edge", bodyId: a.bodyId, edgeId: a.edgeId };
}

function revolveParams(p: RevolveParams): WireRevolveParams {
  const wire: WireRevolveParams = {
    // Rust `angleDeg` is DEGREES — pass through unchanged (unit pinned).
    angleDeg: scalar(p.angleDeg),
    booleanMode: p.booleanMode ?? "NewBody",
  };
  if (p.axis !== undefined) wire.axis = axisRef(p.axis);
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
    case "Revolve": {
      const params = revolveParams(op.params);
      // The profile is a SketchRegionRef (ids real once R-WP12 lands, as Extrude).
      if (op.sketchId && op.regionId) {
        params.profile = { sketchId: op.sketchId, regionId: op.regionId };
      }
      return { opType: "Revolve", params };
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

// ── M4b: raw EditCommand builders (repair rebind + history affordances) ────────
//
// These map straight onto the Rust `EditCommand` vocabulary (edit/command.rs) so
// `client.applyEditCommand(cmd)` can send them verbatim. The record/cursor ids
// are the projection feature ids (a feature's `id` IS its `RecordId` UUID) and
// the timeline cursor (= applied op count; history/timeline.rs).

/** The current fillet params a rebind rewrites (the SUBSET M4b touches). */
export interface CurrentFilletParams {
  radius: number;
  edgeIds: string[];
  /** Typed refs, parallel to `edgeIds` (may be shorter for a legacy fillet). */
  edges?: WireElementRef[];
  chainTangentEdges?: boolean;
}

/** Build the typed edge `ElementRef` for a rebound edge (primary + anchor). */
export function edgeElementRef(
  bodyId: string,
  elementId: string,
  worldPos?: [number, number, number],
): WireElementRef {
  const ref: WireElementRef = { primary: { bodyId, elementId, kind: "edge" } };
  if (worldPos) ref.anchor = { worldPoint: worldPos };
  return ref;
}

/**
 * PURE dual-field fillet-edge rewrite (M4b pinned rule): replace ONLY slot
 * `index` in BOTH `edgeIds` (bare) and `edges` (typed), leaving every sibling
 * edge untouched. `edgeIds[index]` becomes the minted `elementId`; `edges[index]`
 * becomes the typed `ElementRef`. Both arrays are grown to `index + 1` if the
 * current fillet stored fewer entries (legacy/short). Returns the full new
 * `WireFilletParams` an `UpdateOperationParams` carries.
 */
export function rewriteFilletEdgeParams(
  current: CurrentFilletParams,
  index: number,
  ref: WireElementRef,
): WireFilletParams {
  const elementId = ref.primary?.elementId ?? "";
  const edgeIds = [...current.edgeIds];
  const edges = [...(current.edges ?? [])];
  // Grow both arrays so slot `index` exists (keep them the SAME length).
  const len = Math.max(edgeIds.length, edges.length, index + 1);
  while (edgeIds.length < len) edgeIds.push("");
  while (edges.length < len) edges.push({});
  edgeIds[index] = elementId; // bare id (lockstep)
  edges[index] = ref; // typed ref (lockstep)
  return {
    radius: scalar(current.radius),
    edgeIds,
    edges,
    chainTangentEdges: current.chainTangentEdges ?? true,
  };
}

/** `UpdateOperationParams` for a rewritten Fillet (the pinned dual-field path). */
export function updateFilletParamsCommand(
  recordId: string,
  params: WireFilletParams,
): WireEditCommand {
  return { cmd: "updateOperationParams", record: recordId, op: { opType: "Fillet", params } };
}

/**
 * `EditOperationInput` for a single fillet edge slot (the backend-designated
 * fillet-edge rebind — command.rs `InputPath::FilletEdges`, which populates BOTH
 * `edge_ids[index]` and `edges[index]` in lockstep server-side). Needs only the
 * slot index + the new ref, so it works WITHOUT the frontend knowing the fillet's
 * full current edge set (which the projection does not expose).
 */
export function filletEdgeRebindCommand(
  recordId: string,
  index: number,
  ref: WireElementRef,
): WireEditCommand {
  return {
    cmd: "editOperationInput",
    record: recordId,
    path: { path: "filletEdges", index },
    reference: { element: ref },
  };
}

/** `SetOperationSuppression` — suppress/un-suppress `recordId` (optional cascade). */
export function suppressOperationCommand(
  recordId: string,
  suppressed: boolean,
  cascade = false,
): WireEditCommand {
  return { cmd: "setOperationSuppression", record: recordId, suppressed, cascade };
}

/** `SetRollback` — move the rollback cursor (= applied op count; timeline.rs). */
export function rollbackToCursorCommand(cursor: number): WireEditCommand {
  return { cmd: "setRollback", cursor: Math.max(0, Math.floor(cursor)) };
}

/** `RemoveOperation` — delete `recordId` from the timeline. */
export function removeOperationCommand(recordId: string): WireEditCommand {
  return { cmd: "removeOperation", record: recordId };
}

/** A short human label for a raw EditCommand (status-bar hint). */
export function editCommandLabel(cmd: WireEditCommand): string {
  switch (cmd.cmd) {
    case "editOperationInput":
    case "updateOperationParams":
      return "Repair reference";
    case "removeOperation":
      return "Delete feature";
    case "setRollback":
      return "Rollback";
    case "setOperationSuppression":
      return cmd.suppressed ? "Suppress" : "Unsuppress";
    default:
      return "Edit";
  }
}

/**
 * Parse a repair `refId` (`"<opId>.input<k>"`; SCHEMA §9) into its op id + input
 * slot index. Returns `null` when the shape does not match (the caller then treats
 * `k` as 0 / skips slot-targeting) so a backend format change fails soft.
 */
export function parseRefId(refId: string): { opId: string; index: number } | null {
  const m = /^(.*)\.input(\d+)$/.exec(refId);
  if (!m) return null;
  return { opId: m[1], index: Number(m[2]) };
}
