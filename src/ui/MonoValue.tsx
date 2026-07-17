import type { ReactNode, Ref } from "react";
import { cn } from "./cn";

type MonoValueProps = {
  children: ReactNode;
  className?: string;
  ref?: Ref<HTMLSpanElement>;
};

/** Monospace numeric/value text (prototype "83.3 mm", "X 273.00", ...). */
export function MonoValue({ children, className, ref }: MonoValueProps) {
  return (
    <span
      ref={ref}
      className={cn("font-mono tabular-nums text-ink-4", className)}
    >
      {children}
    </span>
  );
}
