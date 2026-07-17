import { Tooltip } from "@/ui/Tooltip";
import { Icon } from "@/icons/Icon";
import type { IconName } from "@/icons/paths";
import { useViewportStore } from "@/stores/viewportStore";

function NavButton({
  icon,
  label,
  strokeWidth = 1.7,
  onClick,
}: {
  icon: IconName;
  label: string;
  strokeWidth?: number;
  onClick: () => void;
}) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-sm border-none bg-transparent text-ink-4 transition-colors hover:bg-hover-2 focus-visible:shadow-focus-ring focus-visible:outline-none"
      >
        <Icon name={icon} size={17} strokeWidth={strokeWidth} />
      </button>
    </Tooltip>
  );
}

/**
 * Bottom-left navigation pill (prototype 1c): home, zoom-to-fit, view presets.
 * Zoom-to-fit is Shift+F because plain F is the Fillet tool (see keymap note),
 * so the tooltip reads "(⇧F)".
 */
export function NavPill() {
  const zoomFit = useViewportStore((s) => s.zoomFit);
  const homeView = useViewportStore((s) => s.homeView);

  return (
    <div className="absolute bottom-[46px] left-3 z-[25] flex gap-0.5 rounded-md border border-border bg-white p-[3px] shadow-ctrl">
      <NavButton icon="home" label="Home view (H)" onClick={homeView} />
      <NavButton icon="fit" label="Zoom to fit (⇧F)" onClick={zoomFit} />
      <NavButton
        icon="layers"
        label="View presets"
        strokeWidth={1.6}
        onClick={() => {}}
      />
    </div>
  );
}
