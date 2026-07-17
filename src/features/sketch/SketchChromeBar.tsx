import { Button } from "@/ui/Button";
import { Icon } from "@/icons/Icon";
import { cn } from "@/ui/cn";
import { useToolStore } from "@/stores/toolStore";
import { useViewportStore } from "@/stores/viewportStore";
import { useDocumentStore } from "@/stores/documentStore";
import { sketchStatusText } from "./constraintStatus";

/**
 * Floating sketch chrome pill (prototype 1c), shown only in sketch mode.
 * Cancel / Finish both exit to model mode (matching Esc / Enter). Compact
 * layout per 1c — no flex spacer (that is the docked-bar 1d variant).
 */
export function SketchChromeBar() {
  const mode = useToolStore((s) => s.mode);
  const setMode = useToolStore((s) => s.setMode);
  const activeSketchId = useViewportStore((s) => s.activeSketchId);
  const sketch = useDocumentStore((s) =>
    activeSketchId ? s.sketches[activeSketchId] : undefined,
  );

  if (mode !== "sketch") return null;

  const name = sketch?.name ?? "Sketch";
  const dof = sketch?.dof ?? 0;
  const { label, tone } = sketchStatusText(sketch?.status ?? "under", dof);
  const exit = () => setMode("model");

  return (
    <div className="absolute left-1/2 top-[62px] z-[29] flex h-[38px] -translate-x-1/2 items-center gap-2.5 rounded-md border border-sketch-chrome-border bg-sketch-chrome pl-3.5 pr-1.5 shadow-sketch-pill">
      <Icon name="penEdit" size={15} strokeWidth={1.8} className="text-accent" />
      <span className="text-[12.5px] font-semibold text-sel-text">
        Editing {name}
      </span>
      <span className={cn("text-[12px] font-medium", tone === "ok" ? "text-ink-4" : "text-warn")}>
        {label}
      </span>
      <Button size="sm" variant="secondary" className="text-ink-3" onClick={exit}>
        <Icon name="x" size={11} strokeWidth={2.2} />
        Cancel
      </Button>
      <Button size="sm" variant="primary" onClick={exit}>
        <Icon name="check" size={12} strokeWidth={2.4} />
        Finish sketch
      </Button>
    </div>
  );
}
