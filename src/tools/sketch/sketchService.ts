/*
 * Shared sketch-edit helpers usable from React (the badge layer's dimension
 * chip) without the imperative SketchController. Keeps store + engine updates in
 * one place so an edit outside the controller stays consistent.
 */
import type { CadClient } from "@/ipc/client";
import { getViewportEngine } from "@/viewport/engineBridge";
import { documentStore, docSketchStatus } from "@/stores/documentStore";
import { viewportStore } from "@/stores/viewportStore";
import { sketchStore } from "@/stores/sketchStore";

/** Edit a dimensional constraint's value → re-solve → refresh geometry + DOF. */
export async function editConstraintValue(
  client: CadClient,
  constraintId: string,
  value: number,
): Promise<void> {
  const session = sketchStore.getState().session;
  if (!session) return;
  const constraints = session.constraints.map((c) =>
    c.id === constraintId ? { ...c, value } : c,
  );
  const result = await client.sketchUpsert(session.sketchId, session.entities, constraints);
  if (!sketchStore.getState().session) return; // exited during await

  const next = { ...session, constraints, dof: result.dof, status: result.status };
  sketchStore.getState().setSession(next);
  getViewportEngine()?.updateSketchSession(next.plane, next.entities, next.status);
  documentStore.getState().setSketchSolve(session.sketchId, result.dof, docSketchStatus(result.status));
  viewportStore.setState({ dofBadge: result.dof });
}
