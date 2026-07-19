/*
 * tauriCommandMap — M4b raw EditCommand builders (repair rebind + history
 * affordances). The dual `edge_ids`/`edges` lockstep rule is the pinned invariant.
 */
import { describe, it, expect } from "vitest";
import {
  bareBodyId,
  edgeElementRef,
  rewriteFilletEdgeParams,
  updateFilletParamsCommand,
  updateScalarParamsCommand,
  filletEdgeRebindCommand,
  suppressOperationCommand,
  rollbackToCursorCommand,
  removeOperationCommand,
  parseRefId,
  editCommandLabel,
  operationToEditCommand,
  opLabelFor,
  type CurrentFilletParams,
  type WireElementRef,
} from "./tauriCommandMap";
import type { OperationOp } from "./types";

/** Extract the AddOperation record's params (the wire `{opType, params}`). */
function addedParams(op: OperationOp): Record<string, unknown> {
  const cmd = operationToEditCommand(op);
  if (cmd.cmd !== "addOperation") throw new Error(`expected addOperation, got ${cmd.cmd}`);
  return cmd.record.params as unknown as Record<string, unknown>;
}

describe("parseRefId", () => {
  it("parses '<opId>.input<k>' into opId + index", () => {
    expect(parseRefId("op_5.input0")).toEqual({ opId: "op_5", index: 0 });
    expect(parseRefId("op_5.input2")).toEqual({ opId: "op_5", index: 2 });
    // opId may itself contain dots (uuid-ish) — only the trailing .input<k> is split.
    expect(parseRefId("a.b.c.input3")).toEqual({ opId: "a.b.c", index: 3 });
  });

  it("returns null for a shape that doesn't match", () => {
    expect(parseRefId("op_5")).toBeNull();
    expect(parseRefId("op_5.face0")).toBeNull();
  });
});

describe("edgeElementRef", () => {
  it("builds a typed edge ref with primary + anchor worldPoint", () => {
    expect(edgeElementRef("body1", "el_9", [1, 2, 3])).toEqual({
      primary: { bodyId: "body1", elementId: "el_9", kind: "edge" },
      anchor: { worldPoint: [1, 2, 3] },
    });
  });

  it("omits the anchor when no worldPos is given", () => {
    expect(edgeElementRef("body1", "el_9")).toEqual({
      primary: { bodyId: "body1", elementId: "el_9", kind: "edge" },
    });
  });

  it("strips a `body_<uuid>` wire-form bodyId to the bare core uuid", () => {
    // promoteSelection returns the worker wire form; the core EditCommand serde wants
    // a bare uuid (BodyId is #[serde(transparent)]).
    expect(bareBodyId("body_abc-123")).toBe("abc-123");
    expect(bareBodyId("abc-123")).toBe("abc-123"); // already bare — no-op
    expect(bareBodyId("body1")).toBe("body1"); // mock id (no underscore) — untouched
    expect(edgeElementRef("body_abc-123", "el_9").primary?.bodyId).toBe("abc-123");
  });
});

describe("filletParams — R-WP2.1 dual edge rule (AddOperation from UI selection)", () => {
  it("emits typed `edges` in lockstep with `edgeIds` (bodyId bare, elementId = edgeId, anchor)", () => {
    const bodyUuid = "11111111-1111-4111-8111-111111111111";
    const op: OperationOp = {
      opType: "Fillet",
      // The per-edge SemanticRefs the fillet tool authors: a `body_<uuid>` wire-form
      // body id (must be stripped) + the pick anchor world-point.
      inputs: [
        { primary: { bodyId: `body_${bodyUuid}`, kind: "edge" }, anchor: { worldPoint: [1, 2, 3] } },
        { primary: { bodyId: bodyUuid, kind: "edge" }, anchor: { worldPoint: [4, 5, 6] } },
      ],
      params: { mode: "Fillet", radius: 2, edgeIds: ["e:5", "e:9"], chainTangentEdges: true },
    };
    const p = addedParams(op);
    expect(p.radius).toEqual({ value: 2 });
    expect(p.edgeIds).toEqual(["e:5", "e:9"]);
    expect(p.chainTangentEdges).toBe(true);
    const edges = p.edges as WireElementRef[];
    expect(edges).toHaveLength(2);
    // bodyId stripped to the bare uuid; primary.element == edgeIds[i] (F2 lockstep).
    expect(edges[0].primary).toEqual({ bodyId: bodyUuid, elementId: "e:5", kind: "edge" });
    expect(edges[1].primary).toEqual({ bodyId: bodyUuid, elementId: "e:9", kind: "edge" });
    // anchor world-point rides through so the worker's ladder resolves the edge.
    expect(edges[0].anchor).toEqual({ worldPoint: [1, 2, 3] });
    expect(edges[1].anchor).toEqual({ worldPoint: [4, 5, 6] });
  });

  it("marshals a bare-`edgeIds` fillet (no typed edges) when an edge carries no body", () => {
    const op: OperationOp = {
      opType: "Fillet",
      inputs: [{ primary: { bodyId: "", kind: "edge" }, anchor: {} }],
      params: { mode: "Fillet", radius: 1, edgeIds: ["e:3"] },
    };
    const p = addedParams(op);
    expect(p.edgeIds).toEqual(["e:3"]);
    expect("edges" in p).toBe(false);
  });

  it("marshals a bare-`edgeIds` fillet when the op carries no inputs (legacy path)", () => {
    const op: OperationOp = {
      opType: "Fillet",
      params: { mode: "Fillet", radius: 1, edgeIds: ["e:3"] },
    };
    const p = addedParams(op);
    expect("edges" in p).toBe(false);
  });
});

describe("updateScalarParamsCommand — re-edit deep-merge (Findings 3+4)", () => {
  it("changes ONLY the scalar, preserving a revolve axis verbatim", () => {
    const stored = {
      angleDeg: { value: 360 },
      axis: { kind: "sketchLine", sketchId: "sk", lineId: "line-7" },
      booleanMode: "NewBody",
      profile: { sketchId: "sk", regionId: "r1" },
    };
    const cmd = updateScalarParamsCommand("rev-rec", "Revolve", stored, { angleDeg: { value: 90 } });
    if (cmd.cmd !== "updateOperationParams") throw new Error("unreachable");
    expect(cmd.record).toBe("rev-rec");
    expect(cmd.op.opType).toBe("Revolve");
    const p = cmd.op.params as unknown as Record<string, unknown>;
    expect(p.angleDeg).toEqual({ value: 90 }); // only the scalar changed
    expect(p.axis).toEqual({ kind: "sketchLine", sketchId: "sk", lineId: "line-7" }); // untouched
    expect(p.profile).toEqual({ sketchId: "sk", regionId: "r1" });
    expect(p.booleanMode).toBe("NewBody");
  });

  it("preserves shell openFaces + targetBodyId while changing thickness", () => {
    const stored = { thickness: { value: 2 }, openFaces: ["el_a", "el_b"], targetBodyId: "b-uuid" };
    const cmd = updateScalarParamsCommand("shell-rec", "Shell", stored, { thickness: { value: 4 } });
    if (cmd.cmd !== "updateOperationParams") throw new Error("unreachable");
    const p = cmd.op.params as unknown as Record<string, unknown>;
    expect(p.thickness).toEqual({ value: 4 });
    expect(p.openFaces).toEqual(["el_a", "el_b"]); // NOT wiped
    expect(p.targetBodyId).toBe("b-uuid");
  });

  it("preserves fillet edgeIds + typed edges while changing radius", () => {
    const edges = [{ primary: { bodyId: "b", elementId: "e:5", kind: "edge" } }];
    const stored = { radius: { value: 2 }, edgeIds: ["e:5"], edges, chainTangentEdges: true };
    const cmd = updateScalarParamsCommand("fil-rec", "Fillet", stored, { radius: { value: 5 } });
    if (cmd.cmd !== "updateOperationParams") throw new Error("unreachable");
    const p = cmd.op.params as unknown as Record<string, unknown>;
    expect(p.radius).toEqual({ value: 5 });
    expect(p.edgeIds).toEqual(["e:5"]); // NOT dropped
    expect(p.edges).toEqual(edges);
  });
});

describe("rewriteFilletEdgeParams — the dual edge_ids/edges lockstep rule", () => {
  const current: CurrentFilletParams = {
    radius: 2,
    edgeIds: ["el_a", "el_b", "el_c"],
    edges: [
      { primary: { bodyId: "b1", elementId: "el_a", kind: "edge" } },
      { primary: { bodyId: "b1", elementId: "el_b", kind: "edge" } },
      { primary: { bodyId: "b1", elementId: "el_c", kind: "edge" } },
    ],
    chainTangentEdges: true,
  };

  it("replaces ONLY the target slot in BOTH arrays, keeping siblings", () => {
    const ref = edgeElementRef("b1", "el_NEW", [5, 6, 7]);
    const params = rewriteFilletEdgeParams(current, 1, ref);
    // Bare id array: only index 1 changed.
    expect(params.edgeIds).toEqual(["el_a", "el_NEW", "el_c"]);
    // Typed array: index 1 is the new ref (with anchor); siblings untouched.
    expect(params.edges?.[0].primary?.elementId).toBe("el_a");
    expect(params.edges?.[1]).toEqual(ref);
    expect(params.edges?.[2].primary?.elementId).toBe("el_c");
    // The two arrays stay the same length (lockstep).
    expect(params.edgeIds.length).toBe(params.edges?.length);
    // radius passes through as a Scalar; chain flag preserved.
    expect(params.radius).toEqual({ value: 2 });
    expect(params.chainTangentEdges).toBe(true);
  });

  it("keeps edgeIds[index] and edges[index].primary.elementId identical", () => {
    const ref = edgeElementRef("b1", "el_X");
    const params = rewriteFilletEdgeParams(current, 0, ref);
    expect(params.edgeIds[0]).toBe("el_X");
    expect(params.edges?.[0].primary?.elementId).toBe("el_X");
  });

  it("grows both arrays (in lockstep) for a legacy fillet with no typed edges", () => {
    const legacy: CurrentFilletParams = { radius: 1, edgeIds: ["el_a"] };
    const params = rewriteFilletEdgeParams(legacy, 1, edgeElementRef("b1", "el_b"));
    expect(params.edgeIds).toEqual(["el_a", "el_b"]);
    expect(params.edges).toHaveLength(2);
    expect(params.edges?.[1].primary?.elementId).toBe("el_b");
  });

  it("wraps into an UpdateOperationParams command (opType Fillet)", () => {
    const params = rewriteFilletEdgeParams(current, 2, edgeElementRef("b1", "el_Z"));
    const cmd = updateFilletParamsCommand("rec-1", params);
    expect(cmd.cmd).toBe("updateOperationParams");
    expect(cmd).toMatchObject({ record: "rec-1", op: { opType: "Fillet" } });
  });
});

describe("filletEdgeRebindCommand (EditOperationInput — the live rebind path)", () => {
  it("carries the FilletEdges{index} path + the element ref", () => {
    const ref = edgeElementRef("b1", "el_9", [1, 2, 3]);
    const cmd = filletEdgeRebindCommand("rec-1", 2, ref);
    expect(cmd).toEqual({
      cmd: "editOperationInput",
      record: "rec-1",
      path: { path: "filletEdges", index: 2 },
      reference: { element: ref },
    });
  });
});

describe("history-affordance command mapping", () => {
  it("suppressOperationCommand → SetOperationSuppression", () => {
    expect(suppressOperationCommand("rec-1", true)).toEqual({
      cmd: "setOperationSuppression",
      record: "rec-1",
      suppressed: true,
      cascade: false,
    });
    expect(suppressOperationCommand("rec-1", false, true)).toMatchObject({
      suppressed: false,
      cascade: true,
    });
  });

  it("rollbackToCursorCommand → SetRollback (cursor = applied op count)", () => {
    // "Roll to here" = index + 1; the caller passes that count.
    expect(rollbackToCursorCommand(3)).toEqual({ cmd: "setRollback", cursor: 3 });
    // Clamped to a non-negative integer.
    expect(rollbackToCursorCommand(-1)).toEqual({ cmd: "setRollback", cursor: 0 });
  });

  it("removeOperationCommand → RemoveOperation", () => {
    expect(removeOperationCommand("rec-1")).toEqual({ cmd: "removeOperation", record: "rec-1" });
  });

  it("editCommandLabel gives a human hint per command", () => {
    expect(editCommandLabel(removeOperationCommand("r"))).toBe("Delete feature");
    expect(editCommandLabel(suppressOperationCommand("r", true))).toBe("Suppress");
    expect(editCommandLabel(suppressOperationCommand("r", false))).toBe("Unsuppress");
    expect(editCommandLabel(rollbackToCursorCommand(2))).toBe("Rollback");
    expect(editCommandLabel(filletEdgeRebindCommand("r", 0, edgeElementRef("b", "e")))).toBe(
      "Repair reference",
    );
  });
});

// ── M6b op wire shapes — pinned against the Rust serde field names ────────────
//
// The names/units below are asserted 1:1 against record.rs (camelCase serde):
//   ShellParams          thickness:Scalar, openFaces:[ElementId], targetBodyId?
//   LinearPatternParams  sourceBodyId?, direction:Vec3, spacing:Scalar, count:u32, fuseResult
//   CircularPatternParams sourceBodyId?, axisOrigin:Vec3, axisDirection:Vec3, angleDeg:Scalar, count:u32, fuseResult
//   MirrorBodyParams     sourceBodyId?, planePoint:Vec3, planeNormal:Vec3, fuseWithOriginal
// Scalar wire form is `{value}`; Vec3 wire form is the `[x,y,z]` array; count is
// a BARE number (u32), not a Scalar.
describe("operationToEditCommand — M6b op wire mappings", () => {
  it("Shell maps thickness (Scalar) + openFaces + targetBodyId", () => {
    const op: OperationOp = {
      opType: "Shell",
      inputs: [{ primary: { bodyId: "body1", kind: "face" } }],
      params: { thickness: 2.5, openFaces: ["el_a", "el_b"], targetBodyId: "body1" },
    };
    expect(addedParams(op)).toEqual({
      thickness: { value: 2.5 },
      openFaces: ["el_a", "el_b"],
      targetBodyId: "body1",
    });
  });

  it("LinearPattern maps direction (Vec3 array) + spacing (Scalar) + count (bare u32) + fuseResult", () => {
    const op: OperationOp = {
      opType: "LinearPattern",
      params: { sourceBodyId: "body1", direction: [1, 0, 0], spacing: 20, count: 4 },
    };
    const params = addedParams(op);
    expect(params).toEqual({
      sourceBodyId: "body1",
      direction: [1, 0, 0],
      spacing: { value: 20 },
      count: 4,
      fuseResult: true, // Rust `default_true`
    });
    // count is a bare number, NOT a Scalar object.
    expect(typeof params.count).toBe("number");
  });

  it("CircularPattern maps axisOrigin/axisDirection (Vec3) + angleDeg (Scalar) + count", () => {
    const op: OperationOp = {
      opType: "CircularPattern",
      params: {
        sourceBodyId: "body1",
        axisOrigin: [0, 0, 0],
        axisDirection: [0, 0, 1],
        angleDeg: 360,
        count: 6,
        fuseResult: false,
      },
    };
    expect(addedParams(op)).toEqual({
      sourceBodyId: "body1",
      axisOrigin: [0, 0, 0],
      axisDirection: [0, 0, 1],
      angleDeg: { value: 360 },
      count: 6,
      fuseResult: false,
    });
  });

  it("MirrorBody maps planePoint/planeNormal (Vec3) + fuseWithOriginal (defaults false)", () => {
    const op: OperationOp = {
      opType: "MirrorBody",
      params: { sourceBodyId: "body1", planePoint: [0, 0, 0], planeNormal: [1, 0, 0] },
    };
    expect(addedParams(op)).toEqual({
      sourceBodyId: "body1",
      planePoint: [0, 0, 0],
      planeNormal: [1, 0, 0],
      fuseWithOriginal: false, // Rust default (NOT default_true)
    });
  });

  it("a featureId re-targets a pattern op via updateOperationParams (parametric edit)", () => {
    const op: OperationOp = {
      opType: "LinearPattern",
      featureId: "rec-9",
      params: { sourceBodyId: "body1", direction: [1, 0, 0], spacing: 10, count: 3 },
    };
    const cmd = operationToEditCommand(op);
    expect(cmd).toMatchObject({ cmd: "updateOperationParams", record: "rec-9", op: { opType: "LinearPattern" } });
  });

  it("opLabelFor gives friendly labels for the M6b ops", () => {
    expect(opLabelFor({ opType: "Shell", params: { thickness: 1, openFaces: [] } })).toBe("Shell");
    expect(
      opLabelFor({ opType: "LinearPattern", params: { direction: [1, 0, 0], spacing: 1, count: 2 } }),
    ).toBe("Linear Pattern");
    expect(
      opLabelFor({
        opType: "CircularPattern",
        params: { axisOrigin: [0, 0, 0], axisDirection: [0, 0, 1], angleDeg: 360, count: 2 },
      }),
    ).toBe("Circular Pattern");
    expect(opLabelFor({ opType: "MirrorBody", params: { planePoint: [0, 0, 0], planeNormal: [1, 0, 0] } })).toBe(
      "Mirror",
    );
  });
});
