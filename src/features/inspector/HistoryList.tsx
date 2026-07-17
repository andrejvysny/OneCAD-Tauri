import { cn } from "@/ui/cn";
import { Icon } from "@/icons/Icon";
import { MonoValue } from "@/ui/MonoValue";
import type { IconName } from "@/icons/paths";
import type { FeatureKind, FeatureMeta } from "@/stores/documentStore";

const FEATURE_ICON: Record<FeatureKind, IconName> = {
  sketch: "pen",
  extrude: "extrude",
  revolve: "revolve",
  fillet: "fillet",
  boolean: "boolean",
};

/** 32px history chip (prototype 1c). Selected feature = sel-bg + sel-text. */
function FeatureRow({
  item,
  selected,
  onSelect,
  onEdit,
}: {
  item: FeatureMeta;
  selected: boolean;
  onSelect?: (id: string) => void;
  onEdit?: (item: FeatureMeta) => void;
}) {
  const interactive = Boolean(onSelect || onEdit);
  return (
    <div
      role={interactive ? "button" : undefined}
      tabIndex={interactive ? 0 : undefined}
      data-testid={`history-row-${item.id}`}
      onClick={() => onSelect?.(item.id)}
      onDoubleClick={() => onEdit?.(item)}
      className={cn(
        "mb-1 flex h-8 items-center gap-2 rounded-sm px-2.5",
        interactive && "cursor-pointer",
        selected ? "bg-sel-bg" : "bg-chip hover:bg-hover-2",
      )}
    >
      <Icon
        name={FEATURE_ICON[item.kind]}
        size={14}
        strokeWidth={1.7}
        className={selected ? "text-sel-text" : "text-ink-4"}
      />
      <span
        className={cn(
          "flex-1 text-[12.5px]",
          selected ? "text-sel-text" : "text-ink-2",
        )}
      >
        {item.label}
      </span>
      {item.valueText && (
        <MonoValue
          className={cn(
            "text-[11.5px]",
            selected ? "text-sel-text" : "text-ink-4",
          )}
        >
          {item.valueText}
        </MonoValue>
      )}
    </div>
  );
}

/**
 * Feature-timeline chips for the inspector SELECTION state. Now LIVE: fed from
 * documentStore.features. Click selects a feature; double-clicking an Extrude
 * feature re-enters its drag edit (parametric-edit seed, F-WP7).
 */
export function HistoryList({
  items,
  selectedId,
  onSelect,
  onEdit,
}: {
  items: FeatureMeta[];
  selectedId?: string;
  onSelect?: (id: string) => void;
  onEdit?: (item: FeatureMeta) => void;
}) {
  return (
    <div>
      {items.map((f) => (
        <FeatureRow
          key={f.id}
          item={f}
          selected={f.id === selectedId}
          onSelect={onSelect}
          onEdit={onEdit}
        />
      ))}
    </div>
  );
}
