import type { Ref } from "react";
import { cn } from "./cn";
import { Icon } from "@/icons/Icon";

type EyeToggleProps = {
  /** true = visible (eye shown at 0.85 opacity), false = hidden (0.3). */
  on: boolean;
  onChange: (on: boolean) => void;
  ariaLabel?: string;
  className?: string;
  ref?: Ref<HTMLButtonElement>;
};

/**
 * Visibility toggle rendered as the single `eye` glyph whose opacity encodes
 * state (prototype tree rows: 0.85 on ↔ 0.3 off, hover → 1). No eye-off glyph
 * exists in the prototype.
 */
export function EyeToggle({
  on,
  onChange,
  ariaLabel,
  className,
  ref,
}: EyeToggleProps) {
  return (
    <button
      ref={ref}
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={ariaLabel ?? (on ? "Hide" : "Show")}
      onClick={() => onChange(!on)}
      className={cn(
        "inline-flex cursor-pointer items-center border-none bg-transparent text-ink-4 transition-opacity",
        "hover:opacity-100 focus-visible:opacity-100 focus-visible:outline-none",
        on ? "opacity-[0.85]" : "opacity-30",
        className,
      )}
    >
      <Icon name="eye" size={14} strokeWidth={1.6} />
    </button>
  );
}
