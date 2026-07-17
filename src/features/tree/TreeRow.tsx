import { cn } from "@/ui/cn";
import { Icon } from "@/icons/Icon";
import { EyeToggle } from "@/ui/EyeToggle";
import type { IconName } from "@/icons/paths";

type TreeRowProps = {
  name: string;
  icon: IconName;
  visible: boolean;
  selected: boolean;
  onSelect: () => void;
  onToggleVisible: (visible: boolean) => void;
  /** Double-click activator (sketch rows enter sketch mode). */
  onActivate?: () => void;
};

/**
 * 32px model-tree row (prototype 1c). Selected = sel-bg + sel-text; hover
 * surface otherwise. The eye is a nested toggle whose click must not bubble
 * into the row's select (prototype stops propagation).
 */
export function TreeRow({
  name,
  icon,
  visible,
  selected,
  onSelect,
  onToggleVisible,
  onActivate,
}: TreeRowProps) {
  return (
    <div
      role="option"
      aria-selected={selected}
      onClick={onSelect}
      onDoubleClick={onActivate}
      className={cn(
        "mx-2 my-px flex h-8 cursor-default items-center gap-2 rounded-sm px-2",
        selected ? "bg-sel-bg text-sel-text" : "text-tree-label hover:bg-hover-2",
      )}
    >
      <Icon name={icon} size={15} strokeWidth={1.6} className="flex-none" />
      <span className="flex-1 text-[13px]">{name}</span>
      <span
        className="flex"
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
      >
        <EyeToggle
          on={visible}
          onChange={onToggleVisible}
          ariaLabel={`Toggle ${name} visibility`}
        />
      </span>
    </div>
  );
}
