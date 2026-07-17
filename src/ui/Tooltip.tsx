import { useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

type TooltipProps = {
  label: string;
  children: ReactNode;
  /** Force-open (mainly for showcase/testing). */
  open?: boolean;
};

type Pos = { x: number; y: number };

/**
 * Dark tooltip positioned centered below the anchor. Portals into document.body
 * with fixed positioning (no portal library). Shows on hover/focus.
 */
export function Tooltip({ label, children, open }: TooltipProps) {
  const anchor = useRef<HTMLSpanElement>(null);
  const [pos, setPos] = useState<Pos | null>(null);

  const show = () => {
    const el = anchor.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({ x: r.left + r.width / 2, y: r.bottom + 8 });
  };
  const hide = () => setPos(null);

  const visible = open || pos !== null;

  return (
    <span
      ref={anchor}
      className="inline-flex"
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={show}
      onBlur={hide}
    >
      {children}
      {visible &&
        createPortal(
          <div
            role="tooltip"
            style={{
              position: "fixed",
              left: pos?.x ?? 0,
              top: pos?.y ?? 0,
              transform: "translateX(-50%)",
            }}
            className="pointer-events-none z-[60] whitespace-nowrap rounded-[5px] bg-tooltip px-2 py-1 font-ui text-[11px] text-tooltip-text"
          >
            {label}
          </div>,
          document.body,
        )}
    </span>
  );
}
