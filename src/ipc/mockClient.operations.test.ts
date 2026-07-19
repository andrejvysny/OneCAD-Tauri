import { describe, it, expect, beforeEach } from "vitest";
import { mockClient, resetMockDocument, resetMockSketches, setMockLatency } from "./mockClient";
import type { OperationOp, SketchEntity } from "./types";

const CIRCLE: SketchEntity = { id: "e1", type: "Circle", center: [0, 0], radius: 10 };

/** Author + finish a one-circle sketch so an extrude has a region to consume. */
async function seedRegion(sketchId = "skA"): Promise<string> {
  await mockClient.enterSketch({ newOnPlane: "XY", sketchId });
  await mockClient.sketchUpsert(sketchId, [CIRCLE], []);
  const fin = await mockClient.finishSketch(sketchId);
  return fin.regions[0].regionId;
}

function extrudeOp(sketchId: string, regionId: string, distance: number): OperationOp {
  return { opType: "Extrude", sketchId, regionId, params: { distance } };
}

function revolveOp(sketchId: string, regionId: string, angleDeg: number, featureId?: string): OperationOp {
  return { opType: "Revolve", sketchId, regionId, featureId, params: { angleDeg, booleanMode: "NewBody" } };
}

describe("mockClient operations", () => {
  beforeEach(() => {
    setMockLatency(0);
    resetMockDocument();
    resetMockSketches();
  });

  it("applyOperation(Extrude) appends a feature + synthesizes a body", async () => {
    const regionId = await seedRegion();
    const res = await mockClient.applyOperation(extrudeOp("skA", regionId, 25));
    expect(res.changedBodies).toHaveLength(1);
    expect(res.features).toHaveLength(6); // 5 base + 1
    const feat = res.features.find((f) => f.kind === "extrude" && f.valueText === "25.0 mm");
    expect(feat).toBeTruthy();
    expect(res.opLabel).toBe("Extrude");

    // The synthesized body is fetchable as a real MESH1 blob.
    const bodyId = res.changedBodies[0].bodyId;
    const mesh = await mockClient.getBodyMesh(bodyId, "coarse");
    expect(mesh.byteLength).toBeGreaterThan(64);
  });

  it("undo removes the body + feature; redo restores them", async () => {
    const regionId = await seedRegion();
    const applied = await mockClient.applyOperation(extrudeOp("skA", regionId, 12));
    const bodyId = applied.changedBodies[0].bodyId;

    const undone = await mockClient.undo();
    expect(undone.removedBodies).toContain(bodyId);
    expect(undone.features).toHaveLength(5);
    expect(undone.opLabel).toBe("Extrude");

    const redone = await mockClient.redo();
    expect(redone.changedBodies.map((b) => b.bodyId)).toContain(bodyId);
    expect(redone.features).toHaveLength(6);
  });

  it("undo on an empty stack is a no-op", async () => {
    const res = await mockClient.undo();
    expect(res.changedBodies).toHaveLength(0);
    expect(res.removedBodies).toHaveLength(0);
    expect(res.features).toHaveLength(5);
  });

  it("Boolean removes the tool body and keeps the target", async () => {
    const regionId = await seedRegion();
    const b1 = (await mockClient.applyOperation(extrudeOp("skA", regionId, 10))).changedBodies[0].bodyId;
    const b2 = (await mockClient.applyOperation(extrudeOp("skA", regionId, 10))).changedBodies[0].bodyId;

    const res = await mockClient.applyOperation({
      opType: "Boolean",
      inputs: [
        { primary: { bodyId: b1, kind: "body" } },
        { primary: { bodyId: b2, kind: "body" } },
      ],
      params: { operation: "Union", targetBodyId: b1, toolBodyId: b2 },
    });
    expect(res.removedBodies).toContain(b2);
    expect(res.changedBodies.map((b) => b.bodyId)).toContain(b1);
    expect(res.features.some((f) => f.kind === "boolean" && f.label === "Union")).toBe(true);
  });

  it("applyOperation(Revolve) appends a revolve feature + synthesizes a body", async () => {
    const regionId = await seedRegion();
    const res = await mockClient.applyOperation(revolveOp("skA", regionId, 270));
    expect(res.changedBodies).toHaveLength(1);
    const feat = res.features.find((f) => f.kind === "revolve");
    expect(feat).toBeTruthy();
    expect(feat!.valueText).toBe("270°");
    expect(res.opLabel).toBe("Revolve");
    const bodyId = res.changedBodies[0].bodyId;
    const mesh = await mockClient.getBodyMesh(bodyId, "coarse");
    expect(mesh.byteLength).toBeGreaterThan(64); // a real MESH1 revolve body
  });

  it("re-editing a Revolve (featureId) updates the angle + reuses the same body", async () => {
    const regionId = await seedRegion();
    const created = await mockClient.applyOperation(revolveOp("skA", regionId, 360));
    const featureId = created.features.find((f) => f.kind === "revolve")!.id;
    const bodyId = created.changedBodies[0].bodyId;

    const edited = await mockClient.applyOperation(revolveOp("skA", regionId, 90, featureId));
    expect(edited.changedBodies.map((b) => b.bodyId)).toContain(bodyId); // rebuilt in place
    const revFeatures = edited.features.filter((f) => f.kind === "revolve");
    expect(revFeatures).toHaveLength(1); // no new row — same feature updated
    expect(revFeatures[0].valueText).toBe("90°");
  });

  it("Fillet re-emits the target body + adds a radius feature (documented mock limit)", async () => {
    const res = await mockClient.applyOperation({
      opType: "Fillet",
      inputs: [{ primary: { bodyId: "body1", kind: "edge" } }],
      params: { mode: "Fillet", radius: 3, edgeIds: ["e:2"] },
    });
    expect(res.changedBodies.map((b) => b.bodyId)).toContain("body1");
    expect(res.features.some((f) => f.kind === "fillet" && f.valueText === "3.0 mm")).toBe(true);
  });

  it("preview session commits with the latest streamed params", async () => {
    const regionId = await seedRegion();
    const session = await mockClient.beginPreview({
      opType: "Extrude",
      sketchId: "skA",
      regionId,
      params: { distance: 10 },
    });
    mockClient.updatePreview(session.sessionId, { distance: 30 }, 1);
    const res = await mockClient.endPreview(session.sessionId, true);
    expect(res).not.toBeNull();
    expect(res!.features.some((f) => f.kind === "extrude" && f.valueText === "30.0 mm")).toBe(true);
  });

  it("updatePreview delivers an exact result carrying its epoch", async () => {
    const regionId = await seedRegion();
    const epochs: number[] = [];
    const unsub = mockClient.onPreviewResult((r) => epochs.push(r.epoch));
    const session = await mockClient.beginPreview({
      opType: "Extrude",
      sketchId: "skA",
      regionId,
      params: { distance: 10 },
    });
    mockClient.updatePreview(session.sessionId, { distance: 20 }, 7);
    await new Promise((r) => setTimeout(r, 5)); // let the (0ms) latency fire
    expect(epochs).toContain(7);
    unsub();
  });

  it("a preview result after the session ends is dropped", async () => {
    const regionId = await seedRegion();
    const seen: number[] = [];
    const unsub = mockClient.onPreviewResult((r) => seen.push(r.epoch));
    const session = await mockClient.beginPreview({
      opType: "Extrude",
      sketchId: "skA",
      regionId,
      params: { distance: 10 },
    });
    mockClient.updatePreview(session.sessionId, { distance: 20 }, 3);
    await mockClient.endPreview(session.sessionId, false); // cancel before the 0ms timer
    await new Promise((r) => setTimeout(r, 5));
    expect(seen).not.toContain(3);
    unsub();
  });
});
