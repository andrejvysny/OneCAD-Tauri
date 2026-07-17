import type { ReactNode, Ref } from "react";
import { cn } from "./cn";

type SectionLabelProps = {
  children: ReactNode;
  className?: string;
  ref?: Ref<HTMLDivElement>;
};

/** Quiet 11px uppercase section header (prototype "BODIES", "SNAP TO", ...). */
export function SectionLabel({ children, className, ref }: SectionLabelProps) {
  return (
    <div
      ref={ref}
      className={cn(
        "font-ui text-[11px] font-medium uppercase tracking-[0.07em] text-ink-5",
        className,
      )}
    >
      {children}
    </div>
  );
}
