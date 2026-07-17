import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { cn } from "./cn";

type Placement = "bottom-start" | "bottom-end" | "left-start";

type PopoverProps = {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  /** Element the popover is anchored to (for positioning + outside-click). */
  anchorRef?: RefObject<HTMLElement | null>;
  /** Panel width in px (prototype snap popover = 238). */
  width?: number;
  /** Show the right-pointing caret (prototype snap popover). */
  caret?: boolean;
  placement?: Placement;
  className?: string;
};

type Pos = { left: number; top: number };

/**
 * Anchored floating panel. Portals into document.body with fixed positioning.
 * Closes on Escape and outside pointer-down.
 */
export function Popover({
  open,
  onClose,
  children,
  anchorRef,
  width = 238,
  caret = false,
  placement = "bottom-start",
  className,
}: PopoverProps) {
  const panel = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<Pos>({ left: 0, top: 0 });

  useEffect(() => {
    if (!open) return;

    const anchor = anchorRef?.current ?? null;
    if (anchor) {
      const r = anchor.getBoundingClientRect();
      if (placement === "left-start") {
        setPos({ left: r.left - width - 12, top: r.top });
      } else if (placement === "bottom-end") {
        // Right edge aligned to the anchor's right edge (menus near a boundary).
        setPos({ left: r.right - width, top: r.bottom + 6 });
      } else {
        setPos({ left: r.left, top: r.bottom + 6 });
      }
    }

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (panel.current?.contains(t)) return;
      if (anchor?.contains(t)) return;
      onClose();
    };

    document.addEventListener("keydown", onKey);
    document.addEventListener("mousedown", onDown);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("mousedown", onDown);
    };
  }, [open, anchorRef, width, caret, placement, onClose]);

  if (!open) return null;

  return createPortal(
    <div
      ref={panel}
      role="dialog"
      style={{ position: "fixed", left: pos.left, top: pos.top, width }}
      className={cn(
        "rounded-md border border-border bg-white font-ui shadow-popover",
        className,
      )}
    >
      {caret && (
        <span
          aria-hidden="true"
          className="absolute -right-[6px] top-4 h-2.5 w-2.5 rotate-45 border-r border-t border-border bg-white"
        />
      )}
      {children}
    </div>,
    document.body,
  );
}
