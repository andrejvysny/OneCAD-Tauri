/*
 * Sketch constraint-state → display text (chrome bar + inspector). One source so
 * the DOF wording stays consistent as the solver state updates (F-WP6).
 */
import type { SketchStatus } from "@/stores/documentStore";

export type StatusTone = "warn" | "ok";

/** Short pill: "Under-constrained · DOF 3" / "Fully constrained · DOF 0" / … */
export function sketchStatusText(status: SketchStatus, dof: number): { label: string; tone: StatusTone } {
  switch (status) {
    case "ok":
      return { label: `Fully constrained · DOF ${dof}`, tone: "ok" };
    case "over":
      return { label: `Over-constrained · DOF ${dof}`, tone: "warn" };
    case "error":
      return { label: `Conflicting · DOF ${dof}`, tone: "warn" };
    default:
      return { label: `Under-constrained · DOF ${dof}`, tone: "warn" };
  }
}

/** Inspector card body sentence. */
export function sketchStatusSentence(status: SketchStatus, dof: number): string {
  if (status === "ok") return "Sketch is fully defined.";
  if (status === "over") return `Over-constrained by ${dof}. Remove a conflicting constraint.`;
  if (status === "error") return "Conflicting constraints. Remove one to resolve.";
  return `${dof} degrees of freedom remain. Add distance or coincident constraints to fully define.`;
}
