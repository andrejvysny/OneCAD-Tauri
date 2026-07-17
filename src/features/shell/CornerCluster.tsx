import { useRef, useState, type Ref } from "react";
import { cn } from "@/ui/cn";
import { Tooltip } from "@/ui/Tooltip";
import { Icon } from "@/icons/Icon";
import type { IconName } from "@/icons/paths";
import { useViewportStore } from "@/stores/viewportStore";
import { SnapPopover } from "@/features/snap/SnapPopover";
import { ViewCube } from "@/features/viewcube/ViewCube";

function ClusterButton({
  icon,
  label,
  strokeWidth = 1.6,
  active = false,
  onClick,
  ref,
}: {
  icon: IconName;
  label: string;
  strokeWidth?: number;
  active?: boolean;
  onClick: () => void;
  ref?: Ref<HTMLButtonElement>;
}) {
  return (
    <Tooltip label={label}>
      <button
        ref={ref}
        type="button"
        aria-label={label}
        aria-pressed={active}
        onClick={onClick}
        className={cn(
          "flex h-9 w-9 cursor-pointer items-center justify-center rounded-md border border-border shadow-ctrl transition-colors",
          "focus-visible:shadow-focus-ring focus-visible:outline-none",
          active ? "bg-sel-bg text-accent" : "bg-white text-ink-4 hover:bg-hover",
        )}
      >
        <Icon name={icon} size={17} strokeWidth={strokeWidth} />
      </button>
    </Tooltip>
  );
}

/**
 * Top-right reserved corner (prototype 1c): ViewCube placeholder + a column of
 * display-mode / grid / snap buttons. Grid + snap show a pressed (accent) state;
 * the snap button opens the settings popover to its left.
 */
export function CornerCluster() {
  const gridVisible = useViewportStore((s) => s.gridVisible);
  const toggleGrid = useViewportStore((s) => s.toggleGrid);
  const cycleDisplayMode = useViewportStore((s) => s.cycleDisplayMode);

  const [snapOpen, setSnapOpen] = useState(false);
  const snapBtnRef = useRef<HTMLButtonElement | null>(null);

  return (
    <div className="absolute right-[296px] top-3 z-[25] flex flex-col items-center gap-2">
      <ViewCube />

      <ClusterButton
        icon="display"
        label="Display mode"
        onClick={cycleDisplayMode}
      />
      <ClusterButton
        icon="grid"
        label="Toggle grid"
        strokeWidth={1.5}
        active={gridVisible}
        onClick={toggleGrid}
      />
      <ClusterButton
        ref={snapBtnRef}
        icon="snap"
        label="Snap settings"
        active={snapOpen}
        onClick={() => setSnapOpen((v) => !v)}
      />

      <SnapPopover
        open={snapOpen}
        onClose={() => setSnapOpen(false)}
        anchorRef={snapBtnRef}
      />
    </div>
  );
}
