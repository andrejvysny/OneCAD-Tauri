/*
 * ModelToolChips — the floating overlay chips for the three model tools
 * (F-WP7): the extrude depth chip, the fillet radius chip, and the boolean
 * op picker. Content is React; POSITIONING is imperative — an engine-owned host
 * node is registered with the HTML overlay driver so it tracks a world anchor
 * every frame with no React re-render.
 *
 * The host node is created once (never part of React's managed layout); the
 * engine appends it to the overlay and the driver transforms it. We `createPortal`
 * the chip content INTO that host, so React only manages the content, never the
 * moved node — avoiding the "removeChild: not a child" reconciliation crash.
 *
 * The boolean chip's 3-button op picker is intentionally minimal — design TODO
 * acknowledged (a proper op popover lands with the extrude/param dialog work).
 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/ui/cn";
import { DimensionInput } from "@/features/sketch/DimensionInput";
import { useToolChipStore, toolChipStore } from "@/stores/toolChipStore";
import { useViewportEngine } from "@/viewport/engineBridge";
import type { BooleanOperation } from "@/ipc/types";

const CHIP_ID = "__model_tool_chip";
const BOOLEAN_OPS: BooleanOperation[] = ["Union", "Cut", "Intersect"];

export function ModelToolChips() {
  const engine = useViewportEngine();
  const kind = useToolChipStore((s) => s.kind);
  const value = useToolChipStore((s) => s.value);
  const op = useToolChipStore((s) => s.op);
  const worldPos = useToolChipStore((s) => s.worldPos);
  // A plain DOM host, created once; the engine owns its DOM position.
  const [host] = useState(() => {
    const el = document.createElement("div");
    el.dataset.testid = "model-tool-chip";
    return el;
  });

  const anchorKey = worldPos ? worldPos.join(",") : "";

  useEffect(() => {
    if (!engine || kind === "none" || !worldPos) return;
    engine.mountChip(CHIP_ID, host, worldPos);
    return () => engine.unmountChip(CHIP_ID, host);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [engine, kind, anchorKey, host]);

  if (kind === "none" || !worldPos) return null;

  const content =
    kind === "extrudeDepth" || kind === "filletRadius" ? (
      <DimensionInput value={value} suffix="mm" onCommit={(v) => toolChipStore.getState().onValue?.(v)} />
    ) : (
      <div className="pointer-events-auto inline-flex items-center gap-1 rounded-md border border-border bg-white p-1 shadow-panel">
        <div className="flex overflow-hidden rounded-sm">
          {BOOLEAN_OPS.map((o) => (
            <button
              key={o}
              type="button"
              onClick={() => toolChipStore.getState().onOp?.(o)}
              className={cn(
                "px-2 py-1 text-[11.5px] font-medium",
                o === op ? "bg-sel-bg text-sel-text" : "bg-chip text-ink-3 hover:bg-hover-2",
              )}
            >
              {o}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={() => toolChipStore.getState().onApply?.()}
          className="rounded-sm bg-accent px-2 py-1 text-[11.5px] font-medium text-white hover:opacity-90"
        >
          Apply
        </button>
      </div>
    );

  return createPortal(content, host);
}
