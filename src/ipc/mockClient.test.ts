/*
 * mockClient — the new file/worker seam methods (F-WP "make it a real app").
 *
 * The mock keeps vitest deterministic with no backend: save is a no-op, Save As /
 * Export return fake paths, and worker-status never fires (the mock has no worker).
 */
import { describe, it, expect } from "vitest";
import { mockClient, setMockRecovery } from "./mockClient";

describe("mockClient file seam", () => {
  it("saveDocument resolves (no-op, no throw) with or without a path", async () => {
    await expect(mockClient.saveDocument()).resolves.toBeUndefined();
    await expect(mockClient.saveDocument("/tmp/x.onecad")).resolves.toBeUndefined();
  });

  it("saveDocumentAs returns a fake .onecad path", async () => {
    const path = await mockClient.saveDocumentAs();
    expect(path).toMatch(/\.onecad$/);
  });

  it("exportStep returns a fake .step path", async () => {
    const path = await mockClient.exportStep();
    expect(path).toMatch(/\.step$/);
  });

  it("exportStl returns a fake .stl path", async () => {
    const path = await mockClient.exportStl();
    expect(path).toMatch(/\.stl$/);
  });

  it("exportObj returns a fake .obj path", async () => {
    const path = await mockClient.exportObj();
    expect(path).toMatch(/\.obj$/);
  });

  it("onWorkerStatus never fires and returns a no-op unsubscribe", () => {
    let fired = false;
    const unsub = mockClient.onWorkerStatus(() => {
      fired = true;
    });
    expect(typeof unsub).toBe("function");
    unsub();
    expect(fired).toBe(false);
  });
});

describe("mockClient crash recovery", () => {
  it("checkRecovery is null by default (no banner unless a test opts in)", async () => {
    setMockRecovery(null);
    expect(await mockClient.checkRecovery()).toBeNull();
  });

  it("checkRecovery reports the seeded info; recoverDocument(true) restores + clears", async () => {
    const info = {
      autosavePath: "/x/autosave/foo.onecad",
      originalPath: "/docs/Bracket.onecad",
      modifiedMs: 1_700_000_000_000,
    };
    setMockRecovery(info);
    expect(await mockClient.checkRecovery()).toEqual(info);

    const snap = await mockClient.recoverDocument(true);
    expect(snap).not.toBeNull();
    expect(snap?.title).toBe("Bracket"); // derived from originalPath basename

    // Consumed: a follow-up check sees nothing.
    expect(await mockClient.checkRecovery()).toBeNull();
  });

  it("recoverDocument(false) discards the offer and resolves null", async () => {
    setMockRecovery({ autosavePath: "/x/autosave/foo.onecad", modifiedMs: 1 });
    expect(await mockClient.recoverDocument(false)).toBeNull();
    expect(await mockClient.checkRecovery()).toBeNull();
  });
});
