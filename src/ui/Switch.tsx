import type { Ref } from "react";
import { cn } from "./cn";

type SwitchProps = {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  ariaLabel?: string;
  className?: string;
  ref?: Ref<HTMLButtonElement>;
};

/**
 * 34x20 track, 16px knob, 14px travel (prototype snap popover toggles).
 * Accent track when on, toggle-off token when off.
 */
export function Switch({
  checked,
  onChange,
  disabled = false,
  ariaLabel,
  className,
  ref,
}: SwitchProps) {
  return (
    <button
      ref={ref}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-[20px] w-[34px] flex-none cursor-pointer rounded-[10px] border-none transition-colors",
        "focus-visible:shadow-focus-ring focus-visible:outline-none",
        "disabled:cursor-not-allowed disabled:opacity-50",
        checked ? "bg-accent" : "bg-toggle-off",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white shadow-knob transition-transform",
          checked ? "translate-x-[14px]" : "translate-x-0",
        )}
      />
    </button>
  );
}
