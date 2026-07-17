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
import { createTauriClient, __setRegenTimeoutForTests } from "./tauriClient";
import { createClient } from "./client";
import { mockClient } from "./mockClient";
import { operationToEditCommand } from "./tauriCommandMap";
import { makeBoxMesh } from "./mockMeshes";
import { parseMeshPayload } from "@/viewport/mesh/parseMeshPayload";
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
});

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

describe("tauriClient solver-lane seam", () => {
  it("runs the sketch solver lane locally (no backend command)", async () => {
    const seen: string[] = [];
    mockIPC((cmd) => {
      seen.push(cmd);
      return undefined;
    });
    const client = createTauriClient();
    const s = await client.enterSketch({ newOnPlane: "XZ", sketchId: "sk" });
    expect(s.plane.kind).toBe("XZ");
    const up = await client.sketchUpsert(
      "sk",
      [{ id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] }],
      [{ id: "c1", type: "Horizontal", entities: ["e1"] }],
    );
    expect(up.sketchRevision).toBe(1);
    expect(up.dof).toBe(3); // line (4) − 1
    const fin = await client.finishSketch("sk");
    expect(Array.isArray(fin.regions)).toBe(true);
    // The sketch lane never touches an invoke command.
    expect(seen).not.toContain("apply_edit_command");
  });

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
