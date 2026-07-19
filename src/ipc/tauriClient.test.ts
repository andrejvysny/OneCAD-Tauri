/*
 * tauriClient — real-backend CadClient over the Tauri command/event surface.
 *
 * Uses `@tauri-apps/api/mocks` (`mockIPC`) to intercept `invoke` + (opt-in) the
 * event plugin, so command marshalling, the ArrayBuffer mesh path, event
 * subscription, runtime selection and the solver-lane seam are all exercised
 * without a real Tauri runtime. The 278 mock-driven tests default to the mock
 * because they never install the IPC bridge.
 */
import { afterEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { createTauriClient, __setRegenTimeoutForTests, __lastSketchSolvedForTests } from "./tauriClient";
import { createClient } from "./client";
import { mockClient } from "./mockClient";
import { operationToEditCommand } from "./tauriCommandMap";
import { makeBoxMesh } from "./mockMeshes";
import { parseMeshPayload } from "@/viewport/mesh/parseMeshPayload";
import { documentStore, seedMockDocument } from "@/stores/documentStore";
import type { DocumentChange, OperationOp } from "./types";

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

function readyProjection(revision: number, features: unknown[] = []): unknown {
  return {
    status: "ready",
    revision,
    title: "Doc",
    dirty: true,
    bodies: {},
    sketches: {},
    features,
  };
}

afterEach(() => {
  clearMocks();
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  // Reset the shared projection store (the hydration test writes it).
  documentStore.getState().applySnapshot(seedMockDocument());
});

/** A full SketchPlane payload (XZ) an `enter_sketch` mock returns. */
const XZ_PLANE = { kind: "XZ", origin: [0, 0, 0], xAxis: [0, 1, 0], yAxis: [0, 0, 1], normal: [1, 0, 0] };

// ── Runtime selection ─────────────────────────────────────────────────────────

describe("createClient runtime selection", () => {
  it("returns the mock client when no Tauri bridge is present", () => {
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    expect(createClient()).toBe(mockClient);
  });

  it("returns the real tauri client when __TAURI_INTERNALS__ is injected", () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
    const client = createClient();
    expect(client).not.toBe(mockClient);
    expect(typeof client.getBodyMesh).toBe("function");
    expect(typeof client.applyOperation).toBe("function");
  });
});

// ── Command marshalling (camelCase args) ──────────────────────────────────────

describe("tauriClient command marshalling", () => {
  it("newDocument invokes new_document and returns the snapshot DTO", async () => {
    const seen: string[] = [];
    mockIPC((cmd) => {
      seen.push(cmd);
      if (cmd === "new_document") return { documentId: "doc-1", title: "Untitled" };
    });
    const snap = await createTauriClient().newDocument();
    expect(seen).toContain("new_document");
    expect(snap).toEqual({ documentId: "doc-1", title: "Untitled" });
  });

  it("openDocument / importStep pass the path as a camelCase arg", async () => {
    const args: Record<string, unknown> = {};
    mockIPC((cmd, payload) => {
      args[cmd] = payload;
      return { documentId: "d", title: "P" };
    });
    const client = createTauriClient();
    await client.openDocument("/tmp/a.onecad");
    await client.importStep("/tmp/b.step");
    expect(args["open_document"]).toEqual({ path: "/tmp/a.onecad" });
    expect(args["import_step"]).toEqual({ path: "/tmp/b.step" });
  });

  it("getBodyMesh marshals bodyId + lod + generation", async () => {
    let payload: unknown;
    mockIPC((cmd, p) => {
      if (cmd === "get_mesh") {
        payload = p;
        return makeBoxMesh();
      }
    });
    await createTauriClient().getBodyMesh("uuid-1", "coarse");
    expect(payload).toEqual({ bodyId: "uuid-1", lod: "coarse", generation: null });
  });

  it("listRecents + openFileDialog pass through", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_recents")
        return [{ id: "a", name: "A", path: "/a.onecad", modifiedAt: "2026-01-01T00:00:00Z" }];
      if (cmd === "open_file_dialog") return "/chosen.onecad";
    });
    const client = createTauriClient();
    expect(await client.listRecents()).toHaveLength(1);
    expect(await client.openFileDialog()).toBe("/chosen.onecad");
  });

  it("openFileDialog resolves null on cancel", async () => {
    mockIPC((cmd) => (cmd === "open_file_dialog" ? null : undefined));
    expect(await createTauriClient().openFileDialog()).toBeNull();
  });
});

// ── Save / Save As / Export STEP (Rust-owned dialogs + fs) ─────────────────────

describe("tauriClient file commands", () => {
  it("saveDocument marshals the path (null when reusing the last path)", async () => {
    const args: Record<string, unknown> = {};
    mockIPC((cmd, payload) => {
      if (cmd === "save_document") args[cmd] = payload;
    });
    const client = createTauriClient();
    await client.saveDocument();
    expect(args["save_document"]).toEqual({ path: null });
    await client.saveDocument("/tmp/a.onecad");
    expect(args["save_document"]).toEqual({ path: "/tmp/a.onecad" });
  });

  it("saveDocumentAs shows the save dialog, saves, and returns the chosen path", async () => {
    const seen: string[] = [];
    let saved: unknown;
    mockIPC((cmd, payload) => {
      seen.push(cmd);
      if (cmd === "save_file_dialog") return "/chosen.onecad";
      if (cmd === "save_document") saved = payload;
    });
    const path = await createTauriClient().saveDocumentAs();
    expect(path).toBe("/chosen.onecad");
    expect(saved).toEqual({ path: "/chosen.onecad" });
    expect(seen).toContain("save_file_dialog");
    expect(seen).toContain("save_document");
  });

  it("saveDocumentAs returns null and does NOT save when the dialog is cancelled", async () => {
    const seen: string[] = [];
    mockIPC((cmd) => {
      seen.push(cmd);
      if (cmd === "save_file_dialog") return null;
    });
    expect(await createTauriClient().saveDocumentAs()).toBeNull();
    expect(seen).not.toContain("save_document");
  });

  it("exportStep invokes export_step_file and returns the written path", async () => {
    let payload: unknown;
    mockIPC((cmd, p) => {
      if (cmd === "export_step_file") {
        payload = p;
        return "/out.step";
      }
    });
    expect(await createTauriClient().exportStep()).toBe("/out.step");
    expect(payload).toEqual({ path: null });
  });

  it("exportStep returns null on a cancelled export dialog", async () => {
    mockIPC((cmd) => (cmd === "export_step_file" ? null : undefined));
    expect(await createTauriClient().exportStep()).toBeNull();
  });

  it("exportStl invokes export_stl_file and returns the written path", async () => {
    let payload: unknown;
    mockIPC((cmd, p) => {
      if (cmd === "export_stl_file") {
        payload = p;
        return "/out.stl";
      }
    });
    expect(await createTauriClient().exportStl()).toBe("/out.stl");
    expect(payload).toEqual({ path: null });
  });

  it("exportStl returns null on a cancelled export dialog", async () => {
    mockIPC((cmd) => (cmd === "export_stl_file" ? null : undefined));
    expect(await createTauriClient().exportStl()).toBeNull();
  });

  it("exportObj invokes export_obj_file and returns the written path", async () => {
    let payload: unknown;
    mockIPC((cmd, p) => {
      if (cmd === "export_obj_file") {
        payload = p;
        return "/out.obj";
      }
    });
    expect(await createTauriClient().exportObj()).toBe("/out.obj");
    expect(payload).toEqual({ path: null });
  });

  it("exportObj returns null on a cancelled export dialog", async () => {
    mockIPC((cmd) => (cmd === "export_obj_file" ? null : undefined));
    expect(await createTauriClient().exportObj()).toBeNull();
  });
});

// ── worker-status event ───────────────────────────────────────────────────────

describe("tauriClient worker-status", () => {
  it("delivers worker-status to subscribers and stops after unsubscribe", async () => {
    mockIPC(() => undefined, { shouldMockEvents: true });
    const client = createTauriClient();
    const seen: { state: string; epoch: number }[] = [];
    const unsub = client.onWorkerStatus((s) => seen.push(s));
    await tick(); // let the lazy listen() register

    await emit("worker-status", { state: "restarting", epoch: 3 });
    expect(seen).toEqual([{ state: "restarting", epoch: 3 }]);

    unsub();
    await emit("worker-status", { state: "ready", epoch: 4 });
    expect(seen).toHaveLength(1); // no delivery after unsubscribe
  });
});

// ── enter_sketch constraint reverse-map (re-entry hydration) ──────────────────

describe("tauriClient enter_sketch constraints", () => {
  it("reverse-maps the worker-wire constraints into frontend constraints", async () => {
    mockIPC(
      (cmd, payload) => {
        if (cmd === "apply_edit_command") return readyProjection(1);
        if (cmd === "enter_sketch")
          return {
            sketchId: (payload as { sketchId: string }).sketchId,
            plane: XZ_PLANE,
            entities: [],
            constraints: [
              { id: "cc", type: "Coincident", entities: ["p1", "p2"] },
              { id: "cd", type: "Distance", entities: ["p1", "p2"], value: 90 },
              { id: "bad", type: "NotAThing", entities: ["x"] },
            ],
            dof: 2,
            status: "UnderConstrained",
          };
      },
      { shouldMockEvents: true },
    );
    const session = await createTauriClient().enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    // Known kinds map through; the unknown kind is dropped.
    expect(session.constraints).toEqual([
      { id: "cc", type: "Coincident", entities: ["p1", "p2"] },
      { id: "cd", type: "Distance", entities: ["p1", "p2"], value: 90 },
    ]);
  });
});

// ── Mesh ArrayBuffer path through the MESH1 parser ────────────────────────────

describe("tauriClient mesh path", () => {
  it("returns the get_mesh ArrayBuffer verbatim and the MESH1 parser accepts it", async () => {
    const bytes = makeBoxMesh();
    mockIPC((cmd) => (cmd === "get_mesh" ? bytes : undefined));
    const buf = await createTauriClient().getBodyMesh("uuid", "coarse");
    expect(buf.byteLength).toBeGreaterThan(64);
    const view = parseMeshPayload(buf);
    expect(view.positions.length).toBeGreaterThan(0);
    expect(view.indices.length % 3).toBe(0);
  });
});

// ── document-changed event subscription ───────────────────────────────────────

describe("tauriClient events", () => {
  it("delivers document-changed to subscribers and stops after unsubscribe", async () => {
    mockIPC(() => undefined, { shouldMockEvents: true });
    const client = createTauriClient();
    const seen: DocumentChange[] = [];
    const unsub = client.onDocumentChanged((c) => seen.push(c));
    await tick(); // let the lazy listen() register

    await emit("document-changed", {
      revision: 7,
      changedBodies: [{ bodyId: "b1", meshKey: "b1:coarse:7" }],
      removedBodies: [],
    });
    expect(seen).toHaveLength(1);
    expect(seen[0].revision).toBe(7);
    expect(seen[0].changedBodies[0].bodyId).toBe("b1");

    unsub();
    await emit("document-changed", { revision: 8, changedBodies: [], removedBodies: [] });
    expect(seen).toHaveLength(1); // no delivery after unsubscribe
  });
});

// ── OperationOp → EditCommand mapping (pure) ──────────────────────────────────

describe("operationToEditCommand", () => {
  it("maps Extrude to addOperation with camelCase Scalar params + profile ref", () => {
    const cmd = operationToEditCommand({
      opType: "Extrude",
      sketchId: "sk",
      regionId: "r1",
      params: { distance: 25, extrudeMode: "Blind", booleanMode: "NewBody" },
    });
    expect(cmd.cmd).toBe("addOperation");
    if (cmd.cmd !== "addOperation") throw new Error("unreachable");
    expect(cmd.atCursor).toBe(false);
    expect(cmd.record.recordId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    expect(cmd.record.opType).toBe("Extrude");
    const p = cmd.record.params as unknown as Record<string, unknown>;
    expect(p.distance).toEqual({ value: 25 });
    expect(p.draftAngleDeg).toEqual({ value: 0 });
    expect(p.extrudeMode).toBe("Blind");
    expect(p.booleanMode).toBe("NewBody");
    expect(p.twoDirections).toBe(false);
    expect(p.profile).toEqual({ sketchId: "sk", regionId: "r1" });
  });

  it("maps Revolve to addOperation with angleDeg (DEGREES), sketchLine axis + profile", () => {
    const cmd = operationToEditCommand({
      opType: "Revolve",
      sketchId: "sk",
      regionId: "r1",
      params: {
        angleDeg: 270,
        axis: { kind: "sketchLine", sketchId: "sk", lineId: "line-7" },
        booleanMode: "NewBody",
      },
    });
    expect(cmd.cmd).toBe("addOperation");
    if (cmd.cmd !== "addOperation") throw new Error("unreachable");
    expect(cmd.record.opType).toBe("Revolve");
    const p = cmd.record.params as unknown as Record<string, unknown>;
    // Unit pinned: angle passes through as a Scalar in DEGREES (no radians).
    expect(p.angleDeg).toEqual({ value: 270 });
    expect(p.axis).toEqual({ kind: "sketchLine", sketchId: "sk", lineId: "line-7" });
    expect(p.booleanMode).toBe("NewBody");
    expect(p.profile).toEqual({ sketchId: "sk", regionId: "r1" });
  });

  it("defaults Revolve booleanMode + omits an absent axis", () => {
    const cmd = operationToEditCommand({
      opType: "Revolve",
      sketchId: "sk",
      regionId: "r1",
      params: { angleDeg: 360 },
    });
    if (cmd.cmd !== "addOperation") throw new Error("unreachable");
    const p = cmd.record.params as unknown as Record<string, unknown>;
    expect(p.angleDeg).toEqual({ value: 360 });
    expect(p.booleanMode).toBe("NewBody");
    expect("axis" in p).toBe(false);
    expect("targetBodyId" in p).toBe(false);
  });

  it("maps a Revolve featureId edit to updateOperationParams (param-only re-edit)", () => {
    const cmd = operationToEditCommand({
      opType: "Revolve",
      featureId: "rev-record-uuid",
      sketchId: "sk",
      regionId: "r1",
      params: { angleDeg: 90 },
    });
    if (cmd.cmd !== "updateOperationParams") throw new Error("unreachable");
    expect(cmd.record).toBe("rev-record-uuid");
    expect(cmd.op.opType).toBe("Revolve");
    expect((cmd.op.params as unknown as Record<string, unknown>).angleDeg).toEqual({ value: 90 });
  });

  it("maps Boolean to addOperation with real body refs + operation", () => {
    const cmd = operationToEditCommand({
      opType: "Boolean",
      inputs: [],
      params: { operation: "Cut", targetBodyId: "body-a", toolBodyId: "body-b" },
    });
    if (cmd.cmd !== "addOperation") throw new Error("unreachable");
    expect(cmd.record.opType).toBe("Boolean");
    expect(cmd.record.params).toEqual({
      operation: "Cut",
      targetBodyId: "body-a",
      toolBodyId: "body-b",
    });
  });

  it("maps a featureId edit to updateOperationParams targeting the record id", () => {
    const cmd = operationToEditCommand({
      opType: "Extrude",
      featureId: "record-uuid",
      sketchId: "s",
      regionId: "r",
      params: { distance: 5 },
    });
    if (cmd.cmd !== "updateOperationParams") throw new Error("unreachable");
    expect(cmd.record).toBe("record-uuid");
    expect(cmd.op.opType).toBe("Extrude");
  });
});

// ── applyOperation / undo / redo — command + regen correlation ────────────────

describe("tauriClient edit + correlation", () => {
  it("applyOperation invokes apply_edit_command and correlates the regen", async () => {
    let command: { cmd?: string } | undefined;
    mockIPC(
      (cmd, payload) => {
        if (cmd === "apply_edit_command") {
          command = (payload as { command: { cmd?: string } }).command;
          setTimeout(() => {
            void emit("document-changed", {
              revision: 6,
              changedBodies: [{ bodyId: "nb", meshKey: "nb:coarse:6" }],
              removedBodies: [],
            });
          }, 0);
          return readyProjection(5, [
            { id: "f", kind: "extrude", label: "Extrude", valueText: "25.0 mm", status: "dirty" },
          ]);
        }
      },
      { shouldMockEvents: true },
    );
    __setRegenTimeoutForTests(300);
    const op: OperationOp = {
      opType: "Boolean",
      inputs: [],
      params: { operation: "Union", targetBodyId: "t", toolBodyId: "u" },
    };
    const res = await createTauriClient().applyOperation(op);
    expect(command?.cmd).toBe("addOperation");
    expect(res.revision).toBe(6);
    expect(res.changedBodies).toEqual([{ bodyId: "nb", meshKey: "nb:coarse:6" }]);
    expect(res.removedBodies).toEqual([]);
    expect(res.features).toHaveLength(1);
    expect(res.opLabel).toBe("Union");
  });

  it("applyOperation falls back to the pre-regen projection when no regen fires", async () => {
    mockIPC((cmd) => (cmd === "apply_edit_command" ? readyProjection(5) : undefined), {
      shouldMockEvents: true,
    });
    __setRegenTimeoutForTests(20);
    const res = await createTauriClient().applyOperation({
      opType: "Boolean",
      params: { operation: "Union", targetBodyId: "t", toolBodyId: "u" },
    } as OperationOp);
    expect(res.revision).toBe(5);
    expect(res.changedBodies).toEqual([]);
  });

  it("undo invokes undo and reports the removed bodies from the regen", async () => {
    mockIPC(
      (cmd) => {
        if (cmd === "undo") {
          setTimeout(() => {
            void emit("document-changed", { revision: 4, changedBodies: [], removedBodies: ["gone"] });
          }, 0);
          return readyProjection(4);
        }
      },
      { shouldMockEvents: true },
    );
    __setRegenTimeoutForTests(300);
    const res = await createTauriClient().undo();
    expect(res.removedBodies).toEqual(["gone"]);
    expect(res.revision).toBe(4);
  });

  it("surfaces a rejected command as an Error carrying the ApiError kind", async () => {
    mockIPC(
      (cmd) => {
        if (cmd === "apply_edit_command") return Promise.reject({ kind: "opFailed", message: "boom" });
      },
      { shouldMockEvents: true },
    );
    __setRegenTimeoutForTests(50);
    await expect(
      createTauriClient().applyOperation({
        opType: "Boolean",
        params: { operation: "Union", targetBodyId: "t", toolBodyId: "u" },
      } as OperationOp),
    ).rejects.toThrow(/opFailed: boom/);
  });
});

// ── Solver-lane seam (local) + preview-commit delegation ──────────────────────

describe("tauriClient sketch solver lane (real commands)", () => {
  const xzPlane = { kind: "XZ", origin: [0, 0, 0], xAxis: [0, 1, 0], yAxis: [0, 0, 1], normal: [1, 0, 0] };

  it("enters via AddSketch + enter_sketch, marshals upserts to SketchEditOp[], finishes", async () => {
    const seen: string[] = [];
    let upsertArgs: { sketchId?: string; ops?: { op: string; entity?: { kind: string } }[] } | undefined;
    mockIPC(
      (cmd, payload) => {
        seen.push(cmd);
        if (cmd === "apply_edit_command") return readyProjection(1);
        if (cmd === "enter_sketch")
          return {
            sketchId: (payload as { sketchId: string }).sketchId,
            plane: xzPlane,
            entities: [],
            constraints: [],
            dof: 4,
            status: "UnderConstrained",
          };
        if (cmd === "sketch_upsert") {
          upsertArgs = payload as typeof upsertArgs;
          return { sketchId: (payload as { sketchId: string }).sketchId, sketchRevision: 1, dof: 3, status: "UnderConstrained", solvedPositions: {} };
        }
        if (cmd === "finish_sketch") return { regions: [] };
      },
      { shouldMockEvents: true },
    );
    const client = createTauriClient();
    const s = await client.enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    expect(s.plane.kind).toBe("XZ");
    expect(s.sketchId).toBe("sk"); // keeps the FRONTEND id (map holds the UUID)
    expect(seen).toContain("apply_edit_command"); // AddSketch created the sketch
    expect(seen).toContain("enter_sketch");

    const up = await client.sketchUpsert(
      "sk",
      [{ id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] }],
      [{ id: "c1", type: "Horizontal", entities: ["e1"] }],
    );
    expect(up.dof).toBe(3);
    expect(up.sketchId).toBe("sk");
    // A Line marshals to two synthesized Points + the Line; H → an AddConstraint.
    const ops = upsertArgs?.ops ?? [];
    expect(ops.filter((o) => o.op === "addEntity" && o.entity?.kind === "point")).toHaveLength(2);
    expect(ops.some((o) => o.op === "addEntity" && o.entity?.kind === "line")).toBe(true);
    expect(ops.some((o) => o.op === "addConstraint")).toBe(true);

    const fin = await client.finishSketch("sk");
    expect(Array.isArray(fin.regions)).toBe(true);
    expect(seen).toContain("sketch_upsert");
    expect(seen).toContain("finish_sketch");
  });

  it("cancelSketch invokes cancel_sketch on the backend sketch id", async () => {
    const seen: string[] = [];
    mockIPC(
      (cmd, payload) => {
        seen.push(cmd);
        if (cmd === "apply_edit_command") return readyProjection(1);
        if (cmd === "enter_sketch")
          return { sketchId: (payload as { sketchId: string }).sketchId, plane: xzPlane, entities: [], constraints: [], dof: 0, status: "FullyConstrained" };
        if (cmd === "cancel_sketch") return null;
      },
      { shouldMockEvents: true },
    );
    const client = createTauriClient();
    await client.enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    await client.cancelSketch("sk");
    expect(seen).toContain("cancel_sketch");
  });
});

describe("tauriClient preview seam (local) + commit delegation", () => {
  it("endPreview(commit) materializes the op through apply_edit_command", async () => {
    const cmds: string[] = [];
    mockIPC(
      (cmd) => {
        cmds.push(cmd);
        if (cmd === "apply_edit_command") {
          setTimeout(() => {
            void emit("document-changed", {
              revision: 9,
              changedBodies: [{ bodyId: "b9", meshKey: "b9:coarse:9" }],
              removedBodies: [],
            });
          }, 0);
          return readyProjection(8);
        }
      },
      { shouldMockEvents: true },
    );
    __setRegenTimeoutForTests(300);
    const client = createTauriClient();
    const session = await client.beginPreview({
      opType: "Extrude",
      sketchId: "sk",
      regionId: "r",
      params: { distance: 10 },
    });
    client.updatePreview(session.sessionId, { distance: 20 }, 1);
    const res = await client.endPreview(session.sessionId, true);
    expect(res).not.toBeNull();
    expect(cmds).toContain("apply_edit_command");
    expect(res?.changedBodies[0].bodyId).toBe("b9");
  });

  it("endPreview(cancel) resolves null without a command", async () => {
    const cmds: string[] = [];
    mockIPC((cmd) => {
      cmds.push(cmd);
      return undefined;
    });
    const client = createTauriClient();
    const session = await client.beginPreview({
      opType: "Extrude",
      sketchId: "sk",
      regionId: "r",
      params: { distance: 10 },
    });
    expect(await client.endPreview(session.sessionId, false)).toBeNull();
    expect(cmds).not.toContain("apply_edit_command");
  });
});

// ── Sketch drag gesture — latest-wins reconciliation ──────────────────────────

describe("tauriClient drag gesture (latest-wins)", () => {
  async function enteredClient(onDrag: () => unknown) {
    let beginArgs: { dragPoint?: string } | undefined;
    mockIPC(
      (cmd, payload) => {
        if (cmd === "apply_edit_command") return readyProjection(1);
        if (cmd === "enter_sketch")
          return { sketchId: (payload as { sketchId: string }).sketchId, plane: XZ_PLANE, entities: [], constraints: [], dof: 4, status: "UnderConstrained" };
        if (cmd === "sketch_upsert")
          return { sketchId: (payload as { sketchId: string }).sketchId, sketchRevision: 1, dof: 3, status: "UnderConstrained", solvedPositions: {} };
        if (cmd === "begin_gesture") {
          beginArgs = payload as { dragPoint: string };
          return { gestureId: 7, ready: true };
        }
        if (cmd === "solve_drag") return onDrag();
        if (cmd === "end_gesture")
          // The worker keys solvedPositions by backend POINT UUID (the id begin
          // translated "e1.Start" to); the client reverse-maps it to "e1.Start".
          return {
            sketchId: "u",
            sketchRevision: 9,
            dof: 0,
            status: "FullyConstrained",
            solvedPositions: beginArgs?.dragPoint ? { [beginArgs.dragPoint]: [8, 8] } : {},
          };
      },
      { shouldMockEvents: true },
    );
    const client = createTauriClient();
    await client.enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    // Create a Line so the point map has "e1.Start" for the drag-point translation.
    await client.sketchUpsert("sk", [{ id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] }], []);
    return { client, getBeginArgs: () => beginArgs };
  }

  const drag = (seq: number, superseded = false) => ({
    gestureId: 7,
    seq,
    status: superseded ? "superseded" : "success",
    dof: 1,
    conflicting: [] as string[],
    positions: superseded ? {} : { p: [seq, seq] },
    solveMicros: 5,
    superseded,
  });

  it("begins on a translated point id, drops stale + superseded seq, commits on end", async () => {
    let resp: unknown;
    const { client, getBeginArgs } = await enteredClient(() => resp);

    const begin = await client.beginGesture("sk", "e1.Start");
    expect(begin.ready).toBe(true);
    // The frontend "e1.Start" was translated to a minted point UUID (not passed raw).
    expect(getBeginArgs()?.dragPoint).not.toBe("e1.Start");
    expect(getBeginArgs()?.dragPoint).toMatch(/^[0-9a-f-]{36}$/);

    resp = drag(5);
    expect((await client.solveDrag([5, 5]))?.seq).toBe(5); // applied

    resp = drag(3);
    expect(await client.solveDrag([3, 3])).toBeNull(); // stale seq → dropped

    resp = drag(9, true);
    expect(await client.solveDrag([9, 9])).toBeNull(); // superseded flag → dropped

    resp = drag(8);
    expect((await client.solveDrag([8, 8]))?.seq).toBe(8); // newer seq → applied

    const end = await client.endGesture([8, 8]);
    expect(end.status).toBe("FullyConstrained");
    // F-WP9: the backend point UUID is reverse-mapped to the frontend point key.
    expect(end.solvedPositions?.["e1.Start"]).toEqual([8, 8]);
  });
});

// ── Promotion (pick → ElementId) ──────────────────────────────────────────────

describe("tauriClient promoteSelection", () => {
  it("prefixes the bodyId, marshals picks, returns minted element ids", async () => {
    let args: { snapshotId?: number; bodyId?: string; picks?: { topoKey: string }[] } | undefined;
    mockIPC((cmd, payload) => {
      if (cmd === "promote_selection") {
        args = payload as typeof args;
        return [{ topoKey: "f:2", elementId: "el_abc", kind: "face", bodyId: "body_uuid-1" }];
      }
    });
    const out = await createTauriClient().promoteSelection("uuid-1", [
      { topoKey: "f:2", anchor: { worldPoint: [1, 2, 3] } },
    ]);
    expect(out[0].elementId).toBe("el_abc");
    expect(args?.bodyId).toBe("body_uuid-1"); // bare uuid prefixed to the wire form
    expect(args?.snapshotId).toBe(0); // no regen published yet ⇒ default snapshot 0
    expect(args?.picks?.[0].topoKey).toBe("f:2");
  });

  it("forwards the published snapshotId from document-changed (not 0)", async () => {
    let args: { snapshotId?: number } | undefined;
    mockIPC((cmd, payload) => {
      if (cmd === "promote_selection") {
        args = payload as typeof args;
        return [{ topoKey: "f:2", elementId: "el_abc", kind: "face", bodyId: "body_uuid-1" }];
      }
    }, { shouldMockEvents: true });
    const client = createTauriClient();
    // Subscribing starts the lazy event listeners so document-changed is observed.
    const unsub = client.onDocumentChanged(() => {});
    await tick();

    // A regen publishes snapshot 5012 (the mesh the pick is scoped to).
    await emit("document-changed", {
      revision: 3,
      snapshotId: 5012,
      changedBodies: [{ bodyId: "b1", meshKey: "b1:coarse:3" }],
      removedBodies: [],
    });
    await tick();

    await client.promoteSelection("uuid-1", [{ topoKey: "f:2" }]);
    expect(args?.snapshotId).toBe(5012); // the REAL published snapshot, not 0
    unsub();
  });
});

// ── Projection hydration bridge (projection-updated → documentStore) ──────────

describe("tauriClient projection hydration", () => {
  it("hydrates documentStore from projection-updated and drops stale revisions", async () => {
    mockIPC(() => undefined, { shouldMockEvents: true });
    const client = createTauriClient();
    const seen: unknown[] = [];
    const unsub = client.onProjectionUpdated((p) => seen.push(p));
    await tick(); // let the lazy listen() register

    documentStore.getState().applySnapshot({ ...seedMockDocument(), revision: 2 });
    await emit("projection-updated", {
      status: "ready",
      revision: 5,
      title: "Opened",
      dirty: true,
      bodies: { b1: { id: "b1", name: "B1", visible: true } },
      sketches: {},
      features: [{ id: "f1", kind: "extrude", label: "Extrude", valueText: "10.0 mm", status: "ok" }],
    });
    expect(documentStore.getState().revision).toBe(5);
    expect(documentStore.getState().bodies.b1.name).toBe("B1");
    expect(documentStore.getState().title).toBe("Opened");
    expect(seen).toHaveLength(1);

    // A stale (lower-revision) projection must NOT clobber the newer state.
    await emit("projection-updated", { status: "ready", revision: 3, title: "STALE", dirty: false, bodies: {}, sketches: {}, features: [] });
    expect(documentStore.getState().revision).toBe(5);
    expect(documentStore.getState().title).toBe("Opened");

    unsub();
  });
});

// ── regen-finished correlation (prompt, no 8 s wait) ──────────────────────────

describe("tauriClient regen-finished correlation", () => {
  it("resolves a no-geometry edit from regen-finished (not the timeout)", async () => {
    mockIPC(
      (cmd) => {
        if (cmd === "apply_edit_command") {
          setTimeout(() => void emit("regen-finished", { revision: 12, outcome: "noop" }), 0);
          return readyProjection(11);
        }
      },
      { shouldMockEvents: true },
    );
    __setRegenTimeoutForTests(5000); // long: regen-finished must win, not the timeout
    const res = await createTauriClient().applyOperation({
      opType: "Boolean",
      params: { operation: "Union", targetBodyId: "t", toolBodyId: "u" },
    } as OperationOp);
    expect(res.revision).toBe(12); // from regen-finished, not the pre-regen 11
    expect(res.changedBodies).toEqual([]); // noop → no body delta
  });
});

// ── sketch-solved event + sketch error path ───────────────────────────────────

describe("tauriClient sketch-solved + errors", () => {
  it("receives the sketch-solved event (mirrors the solve result)", async () => {
    mockIPC(
      (cmd, payload) => {
        if (cmd === "apply_edit_command") return readyProjection(1);
        if (cmd === "enter_sketch")
          return { sketchId: (payload as { sketchId: string }).sketchId, plane: XZ_PLANE, entities: [], constraints: [], dof: 0, status: "FullyConstrained" };
      },
      { shouldMockEvents: true },
    );
    const client = createTauriClient();
    await client.enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    await emit("sketch-solved", { sketchId: "u", sketchRevision: 3, dof: 1, status: "UnderConstrained", solvedPositions: {} });
    await tick();
    expect(__lastSketchSolvedForTests()?.sketchRevision).toBe(3);
  });

  it("surfaces a rejected sketch_upsert as an Error with the ApiError kind", async () => {
    mockIPC(
      (cmd, payload) => {
        if (cmd === "apply_edit_command") return readyProjection(1);
        if (cmd === "enter_sketch")
          return { sketchId: (payload as { sketchId: string }).sketchId, plane: XZ_PLANE, entities: [], constraints: [], dof: 0, status: "FullyConstrained" };
        if (cmd === "sketch_upsert") return Promise.reject({ kind: "opFailed", message: "solve boom" });
      },
      { shouldMockEvents: true },
    );
    const client = createTauriClient();
    await client.enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    await expect(
      client.sketchUpsert("sk", [{ id: "e1", type: "Line", p0: [0, 0], p1: [1, 0] }], []),
    ).rejects.toThrow(/opFailed: solve boom/);
  });
});
