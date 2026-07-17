/*
 * DimensionInput — the editable mono chip on a dimensional constraint badge
 * (Distance/Radius/Angle/…). Commits on Enter/blur → sketchUpsert with the new
 * value (design acknowledges a full dimension DIALOG lands later; this is the
 * inline chip).
 */
import { useEffect, useRef, useState } from "react";

export interface DimensionInputProps {
  value: number;
  suffix?: string;
  onCommit(value: number): void;
}

export function DimensionInput({ value, suffix = "", onCommit }: DimensionInputProps) {
  const [text, setText] = useState(() => value.toFixed(1));
  const ref = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setText(value.toFixed(1));
  }, [value]);

  const commit = () => {
    const n = Number.parseFloat(text);
    if (Number.isFinite(n) && n !== value) onCommit(n);
    else setText(value.toFixed(1));
  };

  return (
    <span className="pointer-events-auto inline-flex items-center gap-0.5 rounded-sm border border-accent bg-white px-1 font-mono text-[11px] text-sel-text shadow-ctrl">
      <input
        ref={ref}
        aria-label="Dimension value"
        className="w-9 bg-transparent text-right outline-none"
        value={text}
        inputMode="decimal"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            commit();
            ref.current?.blur();
          } else if (e.key === "Escape") {
            setText(value.toFixed(1));
            ref.current?.blur();
          }
          e.stopPropagation();
        }}
        onBlur={commit}
      />
      {suffix && <span className="text-ink-5">{suffix}</span>}
    </span>
  );
}
