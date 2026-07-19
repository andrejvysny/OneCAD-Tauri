/*
 * historyActions — the history-row + rebind edit dispatch (M4b). Verifies each
 * action sends the RIGHT raw EditCommand through the client (rollback/suppress/
 * delete command mapping) and reflects the result in the stores.
 */
import { beforeEach, describe, it, expect, vi } from "vitest";
import { suppressFeature, rollToIndex, deleteFeature, rebindCandidate } from "./historyActions";
import { mockClient } from "@/ipc/mockClient";
import { historyStore } from "@/stores/historyStore";
import { documentStore } from "@/stores/documentStore";
import { resetStores } from "@/test/resetStores";
import type { NeedsRepairItem, ResolveCandidate } from "@/ipc/types";

beforeEach(() => resetStores());

describe("historyActions — command mapping", () => {
  it("suppressFeature sends SetOperationSuppression + optimistically dims", async () => {
    const apply = vi.spyOn(mockClient, "applyEditCommand");
    await suppressFeature("f3", true);
    expect(apply.mock.calls[0][0]).toMatchObject({
      cmd: "setOperationSuppression",
      record: "f3",
      suppressed: true,
    });
    expect(historyStore.getState().suppressed.f3).toBe(true);
    apply.mockRestore();
  });

  it("rollToIndex sends SetRollback with cursor = index + 1 (applied op count)", async () => {
    const apply = vi.spyOn(mockClient, "applyEditCommand");
    await rollToIndex(2);
    expect(apply.mock.calls[0][0]).toEqual({ cmd: "setRollback", cursor: 3 });
    apply.mockRestore();
  });

  it("deleteFeature sends RemoveOperation and drops the feature from the store", async () => {
    const apply = vi.spyOn(mockClient, "applyEditCommand");
    expect(documentStore.getState().features.some((f) => f.id === "f3")).toBe(true);
    await deleteFeature("f3");
    expect(apply.mock.calls[0][0]).toEqual({ cmd: "removeOperation", record: "f3" });
    expect(documentStore.getState().features.some((f) => f.id === "f3")).toBe(false);
    apply.mockRestore();
  });
});

describe("historyActions — rebind flow (promote → EditOperationInput)", () => {
  const item: NeedsRepairItem = {
    opId: "f3",
    refId: "f3.input1",
    reason: "ambiguous",
    candidateCount: 2,
  };
  const candidate: ResolveCandidate = {
    topoKey: "e:7",
    score: 0.9,
    margin: 0.02,
    worldPos: [4, 5, 6],
    summary: "linear edge",
  };

  it("promotes the candidate then sends EditOperationInput at the parsed slot", async () => {
    const promote = vi.spyOn(mockClient, "promoteSelection");
    const apply = vi.spyOn(mockClient, "applyEditCommand");
    const ok = await rebindCandidate(item, candidate);
    expect(ok).toBe(true);
    // (a) promotion: topoKey + worldPos anchor on the (single) seed body.
    expect(promote.mock.calls[0][0]).toBe("body1");
    expect(promote.mock.calls[0][1]).toEqual([{ topoKey: "e:7", anchor: { worldPoint: [4, 5, 6] } }]);
    // (c) rebind: EditOperationInput{FilletEdges{index:1}} carrying a typed edge ref
    // (primary edge + anchor worldPoint) — the minted elementId + worldPos.
    const cmd = apply.mock.calls[0][0];
    expect(cmd).toMatchObject({
      cmd: "editOperationInput",
      record: "f3",
      path: { path: "filletEdges", index: 1 },
    });
    expect((cmd as { reference: { element: { primary: { kind: string } } } }).reference.element.primary.kind).toBe(
      "edge",
    );
    promote.mockRestore();
    apply.mockRestore();
  });
});
