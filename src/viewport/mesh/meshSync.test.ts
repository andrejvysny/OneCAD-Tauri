/*
 * MeshIngest store wiring: document-changed → fetch visible bodies → registry
 * swap + scene object; removal + visibility lazy-load; detach empties the
 * registry. THREE is real (jsdom-safe geometry), the engine + client are fakes.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import * as THREE from "three";
import { MeshIngest } from "./meshSync";
import * as reg from "./meshRegistry";
import { makeBoxMesh } from "@/ipc/mockMeshes";
import { documentStore } from "@/stores/documentStore";
import type { CadClient } from "@/ipc/client";
import type { DocumentChange } from "@/ipc/types";
import type { ViewportEngine } from "../engine/ViewportEngine";

const tick = () => new Promise((r) => setTimeout(r, 0));

function fakeEngine() {
  const bodiesRoot = new THREE.Group();
  return {
    bodiesRoot,
    invalidate: vi.fn(),
    refreshHighlights: vi.fn(),
    setHighlightState: vi.fn(),
  } as unknown as ViewportEngine & {
    bodiesRoot: THREE.Group;
    refreshHighlights: ReturnType<typeof vi.fn>;
    setHighlightState: ReturnType<typeof vi.fn>;
  };
}

function fakeClient(getMesh = vi.fn(async () => makeBoxMesh())) {
  const listeners = new Set<(c: DocumentChange) => void>();
  const client = {
    getBodyMesh: getMesh,
    onDocumentChanged: (cb: (c: DocumentChange) => void) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  } as unknown as CadClient;
  return { client, getMesh, emit: (c: DocumentChange) => listeners.forEach((l) => l(c)) };
}

function setBodies(bodies: Record<string, boolean>) {
  const full: Record<string, { id: string; name: string; visible: boolean }> = {};
  for (const [id, visible] of Object.entries(bodies)) full[id] = { id, name: id, visible };
  documentStore.setState({ bodies: full });
}

const changed = (bodyId: string): DocumentChange => ({
  revision: 1,
  changedBodies: [{ bodyId, meshKey: `${bodyId}:coarse:1` }],
  removedBodies: [],
});

let ingest: MeshIngest | null = null;

beforeEach(() => {
  reg.disposeAll();
  reg.__resetRegistryForTests();
});
afterEach(() => {
  ingest?.detach();
  ingest = null;
});

describe("MeshIngest onDocumentChanged", () => {
  it("fetches + swaps + adds a scene object for a changed, visible body", async () => {
    setBodies({ body1: true });
    const engine = fakeEngine();
    const { client, getMesh, emit } = fakeClient();
    ingest = new MeshIngest();
    ingest.attach(engine, client);

    emit(changed("body1"));
    await tick();

    expect(getMesh).toHaveBeenCalledWith("body1", "coarse");
    expect(reg.getEntry("body1")).toBeDefined();
    expect(engine.bodiesRoot.children.length).toBe(1);
    expect(engine.refreshHighlights).toHaveBeenCalled();
  });

  it("skips an invisible changed body (nothing fetched)", async () => {
    setBodies({ body1: false });
    const engine = fakeEngine();
    const { client, getMesh, emit } = fakeClient();
    ingest = new MeshIngest();
    ingest.attach(engine, client);

    emit(changed("body1"));
    await tick();

    expect(getMesh).not.toHaveBeenCalled();
    expect(reg.getEntry("body1")).toBeUndefined();
    expect(engine.bodiesRoot.children.length).toBe(0);
  });

  it("drops a removed body from registry + scene", async () => {
    setBodies({ body1: true });
    const engine = fakeEngine();
    const { client, emit } = fakeClient();
    ingest = new MeshIngest();
    ingest.attach(engine, client);

    emit(changed("body1"));
    await tick();
    expect(reg.getEntry("body1")).toBeDefined();

    emit({ revision: 2, changedBodies: [], removedBodies: ["body1"] });
    await tick();
    expect(reg.getEntry("body1")).toBeUndefined();
    expect(engine.bodiesRoot.children.length).toBe(0);
  });
});

describe("MeshIngest visibility + detach", () => {
  it("lazy-loads a body the first time it becomes visible", async () => {
    setBodies({ body1: false });
    const engine = fakeEngine();
    const { client, getMesh, emit } = fakeClient();
    ingest = new MeshIngest();
    ingest.attach(engine, client);

    emit(changed("body1"));
    await tick();
    expect(getMesh).not.toHaveBeenCalled();

    setBodies({ body1: true }); // flip visible → lazy fetch
    await tick();
    expect(getMesh).toHaveBeenCalledWith("body1", "coarse");
    expect(reg.getEntry("body1")).toBeDefined();
  });

  it("detach clears the scene, disposes the registry, and clears highlights", async () => {
    setBodies({ body1: true });
    const engine = fakeEngine();
    const { client, emit } = fakeClient();
    ingest = new MeshIngest();
    ingest.attach(engine, client);

    emit(changed("body1"));
    await tick();
    expect(reg.registrySize()).toBe(1);

    ingest.detach();
    ingest = null;
    expect(reg.registrySize()).toBe(0);
    expect(engine.bodiesRoot.children.length).toBe(0);
    expect(engine.setHighlightState).toHaveBeenCalledWith(null, []);
  });
});
