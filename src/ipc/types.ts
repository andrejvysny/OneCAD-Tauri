/*
 * IPC data types — a MIRROR of the future Rust DTOs.
 *
 * This file is intentionally TINY. The real, authoritative shapes are minted on
 * the Rust side (serde camelCase) and land with their own work packages:
 *   - DocumentSnapshot   → R-WP10 (projection DTOs / app shell)
 *   - RecentProject      → Rust settings/recents store (later WP)
 *
 * Until then these placeholders let the start screen (F-WP2) compile and run
 * against the mock client. Keep every field here 1:1 with the eventual Rust
 * struct so the swap to the real tauri client (F-WP8) is a no-op for the UI.
 */

/** One entry in the "Recent projects" list on the start screen. */
export interface RecentProject {
  id: string;
  name: string;
  /** Absolute project path (also used as the card's title tooltip). */
  path: string;
  /** ISO-8601 timestamp of the last modification. */
  modifiedAt: string;
  /** Optional data-URI / asset URL for the preview thumbnail. */
  thumbnail?: string;
}

/**
 * Placeholder document handle returned by open/new/import.
 *
 * The real DocumentSnapshot (full projection: bodies, timeline, revision, …)
 * lands with R-WP10. Keep this minimal — the start screen only needs to know a
 * document exists so it can transition to the editor.
 */
export interface DocumentSnapshot {
  documentId: string;
  title: string;
}

/** Level-of-detail tier for a mesh fetch (deflection relative to bbox diagonal). */
export type Lod = "coarse" | "medium" | "fine";

/** Unsubscribe handle returned by event subscriptions. */
export type Unsubscribe = () => void;

/**
 * One changed body in a `document-changed` event (plan's PULL model). The
 * backend only announces WHICH bodies changed + an opaque cache key; the
 * frontend fetches the MESH1 bytes for the visible ones via `getBodyMesh`.
 * `meshKey` mirrors the Rust MeshCache key `(BodyId, Lod, generation)`.
 */
export interface BodyMeshRef {
  bodyId: string;
  meshKey: string;
}

/**
 * `document-changed` payload. Projection stores are written only by backend
 * events; this is the delta a later IPC WP delivers for real (mock emits it).
 */
export interface DocumentChange {
  revision: number;
  changedBodies: BodyMeshRef[];
  removedBodies: string[];
}

// ── Sketch wire shapes (SCHEMA §7.3 Sketch op params + §7.4 solver lane) ─────
//
// These mirror the JSON the C++ worker's solver lane speaks. The mock client
// (this WP) implements the same shapes so the whole sketch UI runs with no
// backend; the real tauri client swaps in later with zero UI changes.

/** Named or custom sketch plane (SCHEMA §7.3 — the non-standard XY basis). */
export type SketchPlaneKind = "XY" | "XZ" | "YZ" | "custom";

export interface SketchPlane {
  kind: SketchPlaneKind;
  origin: [number, number, number];
  xAxis: [number, number, number];
  yAxis: [number, number, number];
  normal: [number, number, number];
}

/** Entity kinds the vertical-slice tools author (subset of §7.3's six). */
export type SketchEntityType = "Point" | "Line" | "Arc" | "Circle";

/**
 * One sketch entity in **plane (u,v) coordinates**. Only the fields relevant to
 * `type` are populated (Line → p0/p1; Circle → center/radius; Arc →
 * center/radius/start/end; Point → p0).
 */
export interface SketchEntity {
  id: string;
  type: SketchEntityType;
  /** Construction geometry (dashed, not part of profiles). */
  construction?: boolean;
  p0?: [number, number];
  p1?: [number, number];
  center?: [number, number];
  radius?: number;
  start?: [number, number];
  end?: [number, number];
}

/** The 18 constraint kinds (SCHEMA §7.3, verbatim from SketchTypes.h). */
export type SketchConstraintType =
  | "Coincident"
  | "Horizontal"
  | "Vertical"
  | "Fixed"
  | "Midpoint"
  | "OnCurve"
  | "Parallel"
  | "Perpendicular"
  | "Tangent"
  | "Concentric"
  | "Equal"
  | "Distance"
  | "HorizontalDistance"
  | "VerticalDistance"
  | "Angle"
  | "Radius"
  | "Diameter"
  | "Symmetric";

/** Which point of an entity a positional constraint references. */
export type ConstraintPosition = "Start" | "End" | "Center" | "Midpoint";

export interface SketchConstraint {
  id: string;
  type: SketchConstraintType;
  /** Referenced entity ids (1 for H/V/Radius, 2 for Coincident/Equal, …). */
  entities: string[];
  /** Per-entity point selector for positional constraints (Coincident, …). */
  positions?: ConstraintPosition[];
  /** Value for dimensional constraints (Distance/Radius/Angle/…). */
  value?: number;
}

/** Solver state (SCHEMA §7.4 SketchUpsert `state`). */
export type SketchSolveStatus =
  | "UnderConstrained"
  | "FullyConstrained"
  | "OverConstrained"
  | "Conflicting";

/** Full authoritative sketch, returned by `enterSketch`. */
export interface SketchSession {
  sketchId: string;
  plane: SketchPlane;
  entities: SketchEntity[];
  constraints: SketchConstraint[];
  dof: number;
  status: SketchSolveStatus;
}

/** `enterSketch` target: an existing sketch id, or a fresh sketch on a plane. */
export type EnterSketchTarget =
  | string
  | { newOnPlane: SketchPlaneKind; sketchId?: string };

/** `sketchUpsert` result (SCHEMA §7.4 SketchUpsert result + solved coords). */
export interface SketchUpsertResult {
  sketchId: string;
  sketchRevision: number;
  dof: number;
  status: SketchSolveStatus;
  /** CHANGED point coordinates after the solve, keyed `entityId.point`. */
  solvedPositions?: Record<string, [number, number]>;
}

/** One closed profile region (SCHEMA §7.4 SketchRegions). */
export interface SketchRegion {
  regionId: string;
  outerLoop: string[];
  holes: string[][];
  /**
   * Optional triangulated fill in **plane (u,v) coordinates**: flat `positions`
   * (u,v pairs) + `indices` (triangle triples). Consumers apply the plane basis.
   */
  previewTriangles?: { positions: number[]; indices: number[] };
}

/** `finishSketch` result — the profiles an extrude/revolve can consume. */
export interface FinishSketchResult {
  regions: SketchRegion[];
}

// ── Model operations (SCHEMA §7.3 op payloads) ───────────────────────────────
//
// These mirror the JSON the C++ worker consumes inside `ExecutePlan.ops`. The
// mock accepts the SAME shapes so the later real-backend swap (F-WP8) is a no-op
// for the tool layer. Values keep OneCAD-CPP `operationTypeName` spelling
// (PascalCase). The vertical slice authors Extrude | Fillet | Boolean.

export type OpType = "Extrude" | "Fillet" | "Boolean";

/** Extrude end condition (SCHEMA §7.3 ExtrudeParams). */
export type ExtrudeMode = "Blind" | "ThroughAll" | "Symmetric" | "ToNext" | "ToFace";
/** Boolean fused into a feature op (SCHEMA §7.3 `booleanMode`). */
export type FeatureBooleanMode = "NewBody" | "Add" | "Cut" | "Intersect";
/** Standalone body-body boolean (SCHEMA §7.3 BooleanParams `operation`). */
export type BooleanOperation = "Union" | "Cut" | "Intersect";

/**
 * A semantic reference (SCHEMA §7.3 `inputs[]` element) — the topological input
 * to an op carried as evidence + identity so the resolution ladder can rebind
 * after edits. The mock only reads `primary`/`anchor`; `intent.descriptor` is
 * captured by Rust in F-WP8. Kept minimal here on purpose.
 */
export interface SemanticRef {
  primary: {
    bodyId: string;
    elementId?: string;
    kind: "body" | "face" | "edge" | "vertex";
  };
  anchor?: {
    worldPoint?: [number, number, number];
    surfaceUv?: [number, number];
  };
}

/** Extrude op params (SCHEMA §7.3 ExtrudeParams — vertical-slice subset). */
export interface ExtrudeParams {
  distance: number;
  draftAngleDeg?: number;
  extrudeMode?: ExtrudeMode;
  booleanMode?: FeatureBooleanMode;
  targetBodyId?: string;
  twoDirections?: boolean;
  extrudeMode2?: ExtrudeMode;
  distance2?: number;
}

/** Fillet/Chamfer op params (SCHEMA §7.3 FilletChamferParams; `mode` distinguishes). */
export interface FilletParams {
  mode: "Fillet" | "Chamfer";
  radius: number;
  /** TopoKeys (snapshot-scoped) or ElementIds; resolved through the ladder. */
  edgeIds: string[];
  chainTangentEdges?: boolean;
}

/** Standalone body-body boolean op params (SCHEMA §7.3 BooleanParams). */
export interface BooleanParams {
  operation: BooleanOperation;
  targetBodyId: string;
  toolBodyId: string;
}

/**
 * One op in an `ExecutePlan` (SCHEMA §7.3), discriminated by `opType`. An
 * optional `featureId` re-targets an EXISTING feature (parametric edit —
 * double-click a history entry → re-drag). `sketchId`/`regionId` on Extrude tell
 * the mock which finished region to synthesize a body from (the worker resolves
 * the region from the semantic ref in F-WP8).
 */
export type OperationOp =
  | {
      opType: "Extrude";
      opId?: string;
      featureId?: string;
      sketchId: string;
      regionId: string;
      inputs?: SemanticRef[];
      params: ExtrudeParams;
    }
  | {
      opType: "Fillet";
      opId?: string;
      featureId?: string;
      inputs?: SemanticRef[];
      params: FilletParams;
    }
  | {
      opType: "Boolean";
      opId?: string;
      featureId?: string;
      inputs?: SemanticRef[];
      params: BooleanParams;
    };

/**
 * One feature-timeline entry (mirrors the Rust projection DTO; identical shape to
 * the store's FeatureMeta so the controller maps it 1:1). The mock now emits
 * these with real values (e.g. "25.0 mm").
 */
export interface FeatureRecord {
  id: string;
  kind: "sketch" | "extrude" | "revolve" | "fillet" | "boolean";
  label: string;
  valueText: string;
  status: "ok" | "dirty" | "error" | "needsRepair";
}

/** `applyOperation` / `endPreview(commit)` / `undo` / `redo` result. */
export interface ApplyOperationResult {
  revision: number;
  changedBodies: BodyMeshRef[];
  removedBodies: string[];
  /** Full feature timeline after the change (authoritative). */
  features: FeatureRecord[];
  /** Human label of the op just applied/undone, for a status hint ("Extrude"). */
  opLabel?: string;
}

// ── Two-level preview (NEW_SPEC §15) ─────────────────────────────────────────

/** Params a preview update carries (opType-specific; loosely typed for the wire). */
export type PreviewParams = Partial<ExtrudeParams> &
  Partial<FilletParams> &
  Partial<BooleanParams> & { [k: string]: unknown };

/** `beginPreview` draft — the base op the drag will refine. */
export interface PreviewDraft {
  opType: OpType;
  sketchId?: string;
  regionId?: string;
  params: PreviewParams;
}

/** `beginPreview` result — the session + the body the L2 mesh is published under. */
export interface PreviewSession {
  sessionId: string;
  previewBodyId: string;
}

/**
 * An exact L2 preview result (NEW_SPEC §15 "Replace preview with exact result").
 * Carries its `epoch` so the frontend can reconcile against the latest params it
 * sent and discard stale responses (Invariant: L1 removed only after the matching
 * epoch arrives). The mesh is a full MESH1 blob (same path as a committed body).
 */
export interface PreviewResult {
  sessionId: string;
  epoch: number;
  bodyId: string;
  mesh: ArrayBuffer;
  /** True for the final exact mesh delivered on commit. */
  committed?: boolean;
}
