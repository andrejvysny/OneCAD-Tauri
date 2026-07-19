/*
 * DimensionInput — the editable mono chip on a dimensional constraint badge
 * (Distance/Radius/Angle/…). Commits on Enter/blur → sketchUpsert with the new
 * value (design acknowledges a full dimension DIALOG lands later; this is the
 * inline chip).
 *
 * The Dimension TOOL reuses this chip with `commitOnBlur={false}` + `onCancel`:
 * while placing a dimension, a canvas click must NOT blur-commit the seeded value
 * (a second line click upgrades a length into an angle), and Esc must cancel the
 * whole pick, not just reset the text.
 */
import { useEffect, useRef, useState } from "react";

export interface DimensionInputProps {
  value: number;
  suffix?: string;
  onCommit(value: number): void;
  /** Commit the current text when the input loses focus (default true). */
  commitOnBlur?: boolean;
  /** Esc handler — when provided, Esc calls this instead of resetting the text. */
  onCancel?: () => void;
  /** Focus + select the field on mount (the dimension tool opens ready to type). */
  autoFocus?: boolean;
}

export function DimensionInput({
  value,
  suffix = "",
  onCommit,
  commitOnBlur = true,
  onCancel,
  autoFocus = false,
}: DimensionInputProps) {
  const [text, setText] = useState(() => value.toFixed(1));
  const ref = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setText(value.toFixed(1));
  }, [value]);

  useEffect(() => {
    if (autoFocus) {
      ref.current?.focus();
      ref.current?.select();
    }
  }, [autoFocus]);

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
            if (onCancel) {
              onCancel();
            } else {
              setText(value.toFixed(1));
              ref.current?.blur();
            }
          }
          e.stopPropagation();
        }}
        onBlur={() => {
          if (commitOnBlur) commit();
        }}
      />
      {suffix && <span className="text-ink-5">{suffix}</span>}
    </span>
  );
}
