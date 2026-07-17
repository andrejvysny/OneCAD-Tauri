import { cn } from "@/ui/cn";
import { useToolStore, activeTool, type Tool } from "@/stores/toolStore";
import { ToolButton } from "./ToolButton";
import { toolsForMode, isSeparator } from "./toolbarConfig";

/**
 * Centered floating tool pill (prototype 1c). Swaps its tool set with the mode
 * and tints its background in sketch mode (toolbar-sketch token). The Model
 * "New sketch" tool enters sketch mode (same as the S shortcut); every other
 * tool just arms itself.
 */
export function FloatingToolbar() {
  const mode = useToolStore((s) => s.mode);
  const current = useToolStore(activeTool);
  const setTool = useToolStore((s) => s.setTool);
  const setMode = useToolStore((s) => s.setMode);

  const entries = toolsForMode(mode);

  const pick = (id: Tool) => {
    if (mode === "model" && id === "sketch") setMode("sketch");
    else setTool(id);
  };

  return (
    <div
      role="toolbar"
      aria-label="Tools"
      className={cn(
        "absolute left-1/2 top-3 z-30 flex -translate-x-1/2 items-center gap-0.5",
        "rounded-lg border border-border p-1 shadow-card",
        mode === "sketch" ? "bg-toolbar-sketch" : "bg-white",
      )}
    >
      {entries.map((e, i) =>
        isSeparator(e) ? (
          <span
            key={`sep-${i}`}
            aria-hidden="true"
            className="mx-1 h-5 w-px bg-border"
          />
        ) : (
          <ToolButton
            key={e.id}
            icon={e.icon}
            label={e.label}
            shortcut={e.shortcut}
            active={current === e.id}
            onClick={() => pick(e.id)}
          />
        ),
      )}
    </div>
  );
}
