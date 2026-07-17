import { useRef, type KeyboardEvent, type Ref } from "react";
import { cn } from "./cn";

export type SegmentedOption<T extends string> = {
  value: T;
  label: string;
};

type Size = "sm" | "md";

type SegmentedToggleProps<T extends string> = {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** Accessible name for the tablist. */
  ariaLabel: string;
  /** md = 26px Model/Sketch pill · sm = 20px status-bar Persp/Ortho pill. */
  size?: Size;
  className?: string;
  ref?: Ref<HTMLDivElement>;
};

const WELL: Record<Size, string> = {
  sm: "gap-0 rounded-sm p-0.5",
  md: "gap-0 rounded-[7px] p-0.5",
};

const SEGMENT: Record<Size, string> = {
  sm: "h-[20px] rounded-[4px] px-2.5 text-[11px]",
  md: "h-[26px] rounded-[5px] px-4 text-[12.5px]",
};

export function SegmentedToggle<T extends string>({
  options,
  value,
  onChange,
  ariaLabel,
  size = "md",
  className,
  ref,
}: SegmentedToggleProps<T>) {
  const btns = useRef<(HTMLButtonElement | null)[]>([]);
  const active = options.findIndex((o) => o.value === value);

  const move = (next: number) => {
    const n = (next + options.length) % options.length;
    onChange(options[n].value);
    btns.current[n]?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    switch (e.key) {
      case "ArrowRight":
      case "ArrowDown":
        e.preventDefault();
        move(active + 1);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        e.preventDefault();
        move(active - 1);
        break;
      case "Home":
        e.preventDefault();
        move(0);
        break;
      case "End":
        e.preventDefault();
        move(options.length - 1);
        break;
    }
  };

  return (
    <div
      ref={ref}
      role="tablist"
      aria-label={ariaLabel}
      className={cn("inline-flex bg-canvas font-ui", WELL[size], className)}
    >
      {options.map((opt, i) => {
        const selected = opt.value === value;
        return (
          <button
            key={opt.value}
            ref={(el) => {
              btns.current[i] = el;
            }}
            type="button"
            role="tab"
            aria-selected={selected}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(opt.value)}
            onKeyDown={onKeyDown}
            className={cn(
              "cursor-pointer border-none font-semibold transition-colors",
              "focus-visible:shadow-focus-ring focus-visible:outline-none",
              SEGMENT[size],
              selected ? "bg-sel-bg text-accent" : "bg-transparent text-ink-4",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
