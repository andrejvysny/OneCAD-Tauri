/*
 * tauriCommandMap — M4b raw EditCommand builders (repair rebind + history
 * affordances). The dual `edge_ids`/`edges` lockstep rule is the pinned invariant.
 */
import { describe, it, expect } from "vitest";
import {
  edgeElementRef,
  rewriteFilletEdgeParams,
  updateFilletParamsCommand,
  filletEdgeRebindCommand,
  suppressOperationCommand,
  rollbackToCursorCommand,
  removeOperationCommand,
  parseRefId,
  editCommandLabel,
  type CurrentFilletParams,
} from "./tauriCommandMap";

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
