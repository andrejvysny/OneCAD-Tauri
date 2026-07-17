import type { ButtonHTMLAttributes, Ref } from "react";
import { cn } from "./cn";

type Variant = "primary" | "secondary" | "ghost";
type Size = "sm" | "md" | "lg";

// primary  = accent fill, white text (prototype New project / Finish sketch)
// secondary = white fill, hairline border (prototype Open… / Import STEP… / Cancel)
// ghost    = transparent, hover surface (prototype nav / toolbar buttons)
const VARIANT: Record<Variant, string> = {
  primary: "bg-accent text-white font-semibold hover:bg-accent-hover",
  secondary:
    "bg-white text-ink-2 font-medium border border-border-strong hover:bg-hover",
  ghost: "bg-transparent text-ink-4 font-medium hover:bg-hover",
};

// sm = 26px chrome/segmented-height · md = 32px general controls
// lg = 36px start-screen action row + corner-cluster buttons (prototype 1a/1c)
const SIZE: Record<Size, string> = {
  sm: "h-[26px] px-2.5 text-[12px]",
  md: "h-[32px] px-3.5 text-[13px]",
  lg: "h-[36px] px-4 text-[13px] gap-2",
};

type ButtonProps = {
  variant?: Variant;
  size?: Size;
  ref?: Ref<HTMLButtonElement>;
} & ButtonHTMLAttributes<HTMLButtonElement>;

export function Button({
  variant = "primary",
  size = "md",
  type = "button",
  className,
  children,
  ref,
  ...rest
}: ButtonProps) {
  return (
    <button
      ref={ref}
      type={type}
      className={cn(
        "inline-flex cursor-pointer select-none items-center justify-center gap-1.5",
        "rounded-sm font-ui leading-none transition-colors",
        "focus-visible:shadow-focus-ring focus-visible:outline-none",
        "disabled:pointer-events-none disabled:opacity-50",
        VARIANT[variant],
        SIZE[size],
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
