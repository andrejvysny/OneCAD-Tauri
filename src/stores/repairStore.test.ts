import { beforeEach, describe, it, expect } from "vitest";
import { repairStore } from "./repairStore";
import type { NeedsRepairEvent } from "@/ipc/types";

const item = (refId: string, opId = "op_5", candidateCount = 2) => ({
  opId,
  refId,
  reason: "ambiguous",
  scoringVersion: 1,
  candidateCount,
});

const event = (revision: number, refIds: string[]): NeedsRepairEvent => ({
  revision,
  items: refIds.map((r) => item(r)),
});

beforeEach(() => repairStore.getState().reset());

describe("repairStore", () => {
  it("applyEvent stores the items + revision", () => {
    repairStore.getState().applyEvent(event(7, ["op_5.input0", "op_5.input1"]));
    const s = repairStore.getState();
    expect(s.revision).toBe(7);
    expect(s.items.map((i) => i.refId)).toEqual(["op_5.input0", "op_5.input1"]);
  });

  it("an empty event auto-dismisses the panel + clears items (repairs cleared)", () => {
    repairStore.getState().applyEvent(event(7, ["op_5.input0"]));
    repairStore.getState().openPanel();
    repairStore.getState().setExpanded("op_5.input0");
    expect(repairStore.getState().panelOpen).toBe(true);

    repairStore.getState().applyEvent({ revision: 8, items: [] });
    const s = repairStore.getState();
    expect(s.items).toHaveLength(0);
    expect(s.panelOpen).toBe(false);
    expect(s.expandedRefId).toBeNull();
  });

  it("open/close panel toggles panelOpen and clears expansion on close", () => {
    repairStore.getState().applyEvent(event(1, ["op_5.input0"]));
    repairStore.getState().openPanel();
    repairStore.getState().setExpanded("op_5.input0");
    repairStore.getState().closePanel();
    const s = repairStore.getState();
    expect(s.panelOpen).toBe(false);
    expect(s.expandedRefId).toBeNull();
    expect(s.hoveredWorldPos).toBeNull();
  });

  it("setExpanded toggles the same ref off", () => {
    repairStore.getState().applyEvent(event(1, ["op_5.input0"]));
    repairStore.getState().setExpanded("op_5.input0");
    expect(repairStore.getState().expandedRefId).toBe("op_5.input0");
    repairStore.getState().setExpanded("op_5.input0");
    expect(repairStore.getState().expandedRefId).toBeNull();
  });

  it("a follow-up event that still lists the expanded ref keeps it expanded", () => {
    repairStore.getState().applyEvent(event(1, ["op_5.input0", "op_5.input1"]));
    repairStore.getState().openPanel();
    repairStore.getState().setExpanded("op_5.input0");
    // Next regen still leaves input0 unresolved (input1 was repaired).
    repairStore.getState().applyEvent(event(2, ["op_5.input0"]));
    const s = repairStore.getState();
    expect(s.panelOpen).toBe(true);
    expect(s.expandedRefId).toBe("op_5.input0");
    expect(s.items).toHaveLength(1);
  });

  it("collapses an expanded ref that a follow-up event no longer lists", () => {
    repairStore.getState().applyEvent(event(1, ["op_5.input0", "op_5.input1"]));
    repairStore.getState().setExpanded("op_5.input1");
    repairStore.getState().applyEvent(event(2, ["op_5.input0"]));
    expect(repairStore.getState().expandedRefId).toBeNull();
  });
});
