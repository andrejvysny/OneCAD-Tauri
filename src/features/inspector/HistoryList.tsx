import { useState } from "react";
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
  shell: "shell",
  linearPattern: "linearPattern",
  circularPattern: "circularPattern",
  mirror: "mirrorBody",
};

/** Per-row history affordances (M4b): suppress toggle · roll-to-here · delete. */
export interface HistoryRowActions {
  /** Whether this feature is (optimistically) suppressed — dims the row + icon. */
  suppressed: boolean;
  onToggleSuppress: (item: FeatureMeta) => void;
  onRoll: (item: FeatureMeta) => void;
  onDelete: (item: FeatureMeta) => void;
}

/** 32px history chip (prototype 1c). Selected feature = sel-bg + sel-text. */
function FeatureRow({
  item,
  selected,
  onSelect,
  onEdit,
  actions,
}: {
  item: FeatureMeta;
  selected: boolean;
  onSelect?: (id: string) => void;
  onEdit?: (item: FeatureMeta) => void;
  actions?: HistoryRowActions;
}) {
  const interactive = Boolean(onSelect || onEdit);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const suppressed = actions?.suppressed ?? false;

  return (
    <div
      role={interactive ? "button" : undefined}
      tabIndex={interactive ? 0 : undefined}
      data-testid={`history-row-${item.id}`}
      onClick={() => onSelect?.(item.id)}
      onDoubleClick={() => onEdit?.(item)}
      className={cn(
        "group relative mb-1 flex h-8 items-center gap-2 rounded-sm px-2.5",
        interactive && "cursor-pointer",
        selected ? "bg-sel-bg" : "bg-chip hover:bg-hover-2",
        suppressed && "opacity-60",
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
          suppressed && "line-through",
        )}
      >
        {item.label}
      </span>

      {item.valueText && !actions && (
        <MonoValue className={cn("text-[11.5px]", selected ? "text-sel-text" : "text-ink-4")}>
          {item.valueText}
        </MonoValue>
      )}

      {actions && (
        <div
          // Revealed on row hover; the suppress toggle stays visible when active so
          // a suppressed row keeps its "dimmed + icon" signal (design §5.1).
          className={cn(
            "flex items-center gap-0.5",
            suppressed ? "opacity-100" : "opacity-0 group-hover:opacity-100",
          )}
          onClick={(e) => e.stopPropagation()}
        >
          <RowIconButton
            testid={`history-suppress-${item.id}`}
            icon="eye"
            title={suppressed ? "Unsuppress" : "Suppress"}
            active={suppressed}
            onClick={() => {
              setConfirmingDelete(false);
              actions.onToggleSuppress(item);
            }}
          />
          <RowIconButton
            testid={`history-roll-${item.id}`}
            icon="clock"
            title="Roll to here"
            onClick={() => {
              setConfirmingDelete(false);
              actions.onRoll(item);
            }}
          />
          {confirmingDelete ? (
            <RowIconButton
              testid={`history-delete-confirm-${item.id}`}
              icon="check"
              title="Confirm delete"
              danger
              onClick={() => {
                setConfirmingDelete(false);
                actions.onDelete(item);
              }}
            />
          ) : (
            <RowIconButton
              testid={`history-delete-${item.id}`}
              icon="x"
              title="Delete"
              onClick={() => setConfirmingDelete(true)}
            />
          )}
        </div>
      )}
    </div>
  );
}

/** A tiny 20px icon button used in the history-row affordance cluster. */
function RowIconButton({
  testid,
  icon,
  title,
  onClick,
  active,
  danger,
}: {
  testid: string;
  icon: IconName;
  title: string;
  onClick: () => void;
  active?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      data-testid={testid}
      title={title}
      aria-label={title}
      onClick={onClick}
      className={cn(
        "flex h-5 w-5 items-center justify-center rounded-sm hover:bg-hover-3",
        danger ? "text-traffic-close" : active ? "text-warn" : "text-ink-5",
      )}
    >
      <Icon name={icon} size={13} strokeWidth={1.8} />
    </button>
  );
}

/**
 * Feature-timeline chips for the inspector SELECTION state. Now LIVE: fed from
 * documentStore.features. Click selects a feature; double-clicking an editable
 * feature re-enters its drag edit (parametric-edit seed). When `rowActions` is
 * provided (full-timeline view) each row grows hover affordances: suppress,
 * roll-to-here, delete (M4b).
 */
export function HistoryList({
  items,
  selectedId,
  onSelect,
  onEdit,
  rowActions,
}: {
  items: FeatureMeta[];
  selectedId?: string;
  onSelect?: (id: string) => void;
  onEdit?: (item: FeatureMeta) => void;
  /** Builds the per-row affordances for a feature (omit to hide the menu). */
  rowActions?: (item: FeatureMeta) => HistoryRowActions;
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
          actions={rowActions?.(f)}
        />
      ))}
    </div>
  );
}
