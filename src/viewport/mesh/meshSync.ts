/*
 * MeshIngest — the app-level glue that turns backend `document-changed` events
 * into scene geometry (pull model, SCHEMA §7.6 / plan "Mesh transfer").
 *
 * Flow: onDocumentChanged → for each changed + VISIBLE body, fetch its MESH1
 * blob (getBodyMesh) → parse (zero-copy) → build registry entry → double-buffer
 * swap → (re)build its BodyObject in bodiesRoot → refresh highlights + invalidate.
 * Removed bodies are dropped from the registry and the scene. Visibility flips in
 * the document store toggle the BodyObject (lazy-loading a body the first time it
 * becomes visible).
 *
 * The engine stays graphics-only; this controller owns the client, the document
 * store subscription, the shared body materials, and the bodyId→BodyObject map.
 * `detach` clears the scene, disposes the registry (leak tripwire) and materials.
 */
import type { CadClient } from "@/ipc/client";
import type { DocumentChange, Lod } from "@/ipc/types";
import { documentStore } from "@/stores/documentStore";
import type { ViewportEngine } from "../engine/ViewportEngine";
import {
  buildBodyObject,
  createBodyMaterials,
  type BodyMaterials,
  type BodyObjectHandle,
} from "../engine/BodyObject";
import { parseMeshPayload } from "./parseMeshPayload";
import { buildBodyObjects, disposeAll, remove, swap } from "./meshRegistry";

const DEFAULT_LOD: Lod = "coarse";

export class MeshIngest {
  private engine: ViewportEngine | null = null;
  private client: CadClient | null = null;
  private materials: BodyMaterials | null = null;
  private readonly bodyObjects = new Map<string, BodyObjectHandle>();
  private readonly unsubs: Array<() => void> = [];
  private meshRev = 0;
  /** Per-body monotonic fetch token — a resolved fetch older than the latest is discarded. */
  private readonly loadSeq = new Map<string, number>();
  private detached = false;
  /** Fires after a body's mesh finishes loading into the scene (F-WP7 commit reconcile). */
  private readonly bodyLoadedListeners = new Set<(bodyId: string) => void>();

  /** Subscribe to "a body finished loading into bodiesRoot". Returns an unsubscribe. */
  onBodyLoaded(cb: (bodyId: string) => void): () => void {
    this.bodyLoadedListeners.add(cb);
    return () => this.bodyLoadedListeners.delete(cb);
  }

  attach(engine: ViewportEngine, client: CadClient): void {
    this.engine = engine;
    this.client = client;
    this.materials = createBodyMaterials();
    this.detached = false;

    this.unsubs.push(client.onDocumentChanged((c) => this.onDocumentChanged(c)));

    // Visibility flips come through the document store (tree eye toggle).
    let prev = documentStore.getState().bodies;
    this.unsubs.push(
      documentStore.subscribe((s) => {
        if (s.bodies !== prev) {
          this.onVisibilityChanged(prev, s.bodies);
          prev = s.bodies;
        }
      }),
    );
  }

  private onDocumentChanged(change: DocumentChange): void {
    for (const id of change.removedBodies) this.dropBody(id);
    const bodies = documentStore.getState().bodies;
    for (const ref of change.changedBodies) {
      const visible = bodies[ref.bodyId]?.visible ?? true;
      if (visible) void this.loadBody(ref.bodyId, DEFAULT_LOD);
    }
  }

  private onVisibilityChanged(
    prev: Record<string, { visible: boolean }>,
    next: Record<string, { visible: boolean }>,
  ): void {
    for (const [id, meta] of Object.entries(next)) {
      const was = prev[id]?.visible;
      if (was === meta.visible) continue;
      const handle = this.bodyObjects.get(id);
      if (handle) {
        handle.setVisible(meta.visible);
        this.engine?.invalidate();
      } else if (meta.visible) {
        void this.loadBody(id, DEFAULT_LOD); // lazy-load on first show
      }
    }
  }

  private async loadBody(bodyId: string, lod: Lod): Promise<void> {
    if (!this.client || !this.engine || !this.materials) return;
    const token = (this.loadSeq.get(bodyId) ?? 0) + 1;
    this.loadSeq.set(bodyId, token);

    const buffer = await this.client.getBodyMesh(bodyId, lod);
    // Discard if detached or superseded by a newer fetch for this body.
    if (this.detached || this.loadSeq.get(bodyId) !== token) return;

    const view = parseMeshPayload(buffer);
    const entry = buildBodyObjects(view, bodyId, ++this.meshRev);
    swap(bodyId, entry);

    // Rebuild the scene object (remove old, add new).
    const old = this.bodyObjects.get(bodyId);
    if (old) this.engine.bodiesRoot.remove(old.group);
    const handle = buildBodyObject(entry, this.materials);
    handle.setVisible(documentStore.getState().bodies[bodyId]?.visible ?? true);
    this.engine.bodiesRoot.add(handle.group);
    this.bodyObjects.set(bodyId, handle);

    this.engine.refreshHighlights();
    this.engine.invalidate();
    for (const cb of [...this.bodyLoadedListeners]) cb(bodyId);
  }

  private dropBody(bodyId: string): void {
    const handle = this.bodyObjects.get(bodyId);
    if (handle) {
      this.engine?.bodiesRoot.remove(handle.group);
      this.bodyObjects.delete(bodyId);
    }
    this.loadSeq.delete(bodyId);
    remove(bodyId);
    this.engine?.refreshHighlights();
    this.engine?.invalidate();
  }

  detach(): void {
    this.detached = true;
    this.bodyLoadedListeners.clear();
    for (const u of this.unsubs.splice(0)) u();
    // Clear highlights BEFORE disposing geometry so no clone references freed buffers.
    this.engine?.setHighlightState(null, []);
    for (const handle of this.bodyObjects.values()) {
      this.engine?.bodiesRoot.remove(handle.group);
    }
    this.bodyObjects.clear();
    this.loadSeq.clear();
    disposeAll(); // registry empty + leak tripwire
    this.materials?.dispose();
    this.materials = null;
    this.engine = null;
    this.client = null;
  }
}
