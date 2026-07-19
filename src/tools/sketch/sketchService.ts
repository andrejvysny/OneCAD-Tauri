/*
 * Shared sketch-edit helpers usable from React (the badge layer's dimension
 * chip) without the imperative SketchController. Keeps store + engine updates in
 * one place so an edit outside the controller stays consistent.
 */
import type { CadClient } from "@/ipc/client";
import type { SketchConstraint } from "@/ipc/types";
import { getViewportEngine } from "@/viewport/engineBridge";
import { documentStore, docSketchStatus } from "@/stores/documentStore";
import { viewportStore } from "@/stores/viewportStore";
import { sketchStore } from "@/stores/sketchStore";
import { applySolvedPositions } from "@/ipc/sketchWireMap";
import { isConflictStatus } from "./dimensionTool";

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

/**
 * Author a NEW dimensional constraint (Dimension tool) → re-solve, refresh
 * geometry + DOF. If the solve reports over-constrained/conflicting, REJECT it:
 * remove the constraint, re-solve to the prior state, and surface a status hint
 * (`{ rejected: true }`). The solver's status is the only signal the mock lane
 * exposes — see `isConflictStatus` for the granularity seam.
 */
export async function commitDimensionConstraint(
  client: CadClient,
  constraint: SketchConstraint,
): Promise<{ rejected: boolean }> {
  const session = sketchStore.getState().session;
  if (!session) return { rejected: false };

  const constraints = [...session.constraints, constraint];
  const result = await client.sketchUpsert(session.sketchId, session.entities, constraints);
  if (!sketchStore.getState().session) return { rejected: false }; // exited during await

  if (isConflictStatus(result.status)) {
    // Reject-on-conflict: drop the dimension and restore the previous solve.
    const restore = await client.sketchUpsert(session.sketchId, session.entities, session.constraints);
    if (!sketchStore.getState().session) return { rejected: true };
    const reverted = { ...session, dof: restore.dof, status: restore.status };
    sketchStore.getState().setSession(reverted);
    getViewportEngine()?.updateSketchSession(reverted.plane, reverted.entities, reverted.status);
    documentStore.getState().setSketchSolve(session.sketchId, restore.dof, docSketchStatus(restore.status));
    viewportStore.setState({ dofBadge: restore.dof });
    return { rejected: true };
  }

  const solvedEntities = applySolvedPositions(session.entities, result.solvedPositions ?? {});
  const next = { ...session, entities: solvedEntities, constraints, dof: result.dof, status: result.status };
  sketchStore.getState().setSession(next);
  getViewportEngine()?.updateSketchSession(next.plane, solvedEntities, next.status);
  documentStore.getState().setSketchSolve(session.sketchId, result.dof, docSketchStatus(result.status));
  viewportStore.setState({ dofBadge: result.dof });
  return { rejected: false };
}
