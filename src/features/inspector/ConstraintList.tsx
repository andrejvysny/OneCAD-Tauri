import { MonoValue } from "@/ui/MonoValue";
import type { SketchConstraint, SketchConstraintType } from "@/ipc/types";

export interface ConstraintRow {
  label: string;
  /** Mono count/value, e.g. "×4" or "90.00". */
  count: string;
}

/** Dimensional constraint kinds whose value is folded into the row label. */
const DIMENSIONAL: ReadonlySet<SketchConstraintType> = new Set([
  "Distance",
  "HorizontalDistance",
  "VerticalDistance",
  "Angle",
  "Radius",
  "Diameter",
]);

/**
 * Summarize the live sketch constraints into inspector rows: one row per kind
 * (dimensional kinds keyed by kind+value so "Distance 90.00" groups separately),
 * counted, in first-seen order — the prototype's Coincident ×4 / Horizontal ×1 /
 * Distance 90.00 shape, now driven by the real sketch session.
 */
export function summarizeConstraints(constraints: SketchConstraint[]): ConstraintRow[] {
  const counts = new Map<string, number>();
  for (const c of constraints) {
    const label =
      DIMENSIONAL.has(c.type) && typeof c.value === "number"
        ? `${c.type} ${c.value.toFixed(2)}`
        : c.type;
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  return [...counts].map(([label, n]) => ({ label, count: `×${n}` }));
}

/** 30px constraint rows for the inspector SKETCH state (prototype 1c). */
export function ConstraintList({ items }: { items: ConstraintRow[] }) {
  return (
    <div>
      {items.map((c) => (
        <div
          key={c.label}
          className="mb-1 flex h-[30px] items-center gap-2 rounded-sm bg-chip px-2.5"
        >
          <span className="flex-1 text-[12.5px] text-ink-2">{c.label}</span>
          <MonoValue className="text-[11.5px] text-ink-5">{c.count}</MonoValue>
        </div>
      ))}
    </div>
  );
}
