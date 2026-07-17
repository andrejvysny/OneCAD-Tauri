import { cn } from "@/ui/cn";
import { Tooltip } from "@/ui/Tooltip";
import { Icon } from "@/icons/Icon";
import type { IconName } from "@/icons/paths";

type ToolButtonProps = {
  icon: IconName;
  label: string;
  shortcut: string;
  active: boolean;
  onClick: () => void;
};

/**
 * 34px floating-toolbar tool (prototype 1c). Active = selection-tint bg + accent
 * icon; hover surface otherwise. Tooltip shows "Label (Shortcut)" below.
 */
export function ToolButton({
  icon,
  label,
  shortcut,
  active,
  onClick,
}: ToolButtonProps) {
  return (
    <Tooltip label={`${label} (${shortcut})`}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={active}
        onClick={onClick}
        className={cn(
          "flex h-[34px] w-[34px] cursor-pointer items-center justify-center rounded-sm border-none transition-colors",
          "hover:bg-hover-3 focus-visible:shadow-focus-ring focus-visible:outline-none",
          active ? "bg-sel-bg text-accent" : "bg-transparent text-ink-4",
        )}
      >
        <Icon name={icon} size={18} strokeWidth={1.7} />
      </button>
    </Tooltip>
  );
}
