import { MonoValue } from "@/ui/MonoValue";

export interface ConstraintRow {
  label: string;
  /** Mono count/value, e.g. "×4" or "90.00". */
  count: string;
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
