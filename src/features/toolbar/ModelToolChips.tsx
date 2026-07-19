/*
 * ModelToolChips — the floating overlay chips for the model tools (F-WP7 +
 * M6b): extrude depth, fillet radius, revolve angle, the boolean op picker, and
 * the M6b shell-thickness / linear-pattern / circular-pattern / mirror chips.
 * Content is React; POSITIONING is imperative — an engine-owned host node is
 * registered with the HTML overlay driver so it tracks a world anchor every
 * frame with no React re-render.
 *
 * The host node is created once (never part of React's managed layout); the
 * engine appends it to the overlay and the driver transforms it. We `createPortal`
 * the chip content INTO that host, so React only manages the content, never the
 * moved node — avoiding the "removeChild: not a child" reconciliation crash.
 *
 * The multi-control chips (boolean op, patterns, mirror) are intentionally
 * minimal button rows — a proper op popover / gizmo lands with the param-dialog
 * work (design TODO acknowledged).
 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/ui/cn";
import { DimensionInput } from "@/features/sketch/DimensionInput";
import { useToolChipStore, toolChipStore } from "@/stores/toolChipStore";
import { useViewportEngine } from "@/viewport/engineBridge";
import type { BooleanOperation } from "@/ipc/types";
import type { PatternAxis, MirrorPlane } from "@/tools/modelTools/modelToolMachine";

const CHIP_ID = "__model_tool_chip";
const BOOLEAN_OPS: BooleanOperation[] = ["Union", "Cut", "Intersect"];
const PATTERN_AXES: PatternAxis[] = ["X", "Y", "Z"];
const MIRROR_PLANES: MirrorPlane[] = ["XY", "XZ", "YZ"];

/** A segmented toggle row (axis / plane pickers), styled like the boolean op row. */
function SegmentToggle<T extends string>({
  options,
  active,
  onPick,
  label,
}: {
  options: readonly T[];
  active: T;
  onPick: (v: T) => void;
  label: string;
}) {
  return (
    <div className="flex overflow-hidden rounded-sm" role="group" aria-label={label}>
      {options.map((o) => (
        <button
          key={o}
          type="button"
          aria-pressed={o === active}
          onClick={() => onPick(o)}
          className={cn(
            "px-2 py-1 text-[11.5px] font-medium",
            o === active ? "bg-sel-bg text-sel-text" : "bg-chip text-ink-3 hover:bg-hover-2",
          )}
        >
          {o}
        </button>
      ))}
    </div>
  );
}

/** A −/n/+ count stepper for the pattern instance count. */
function CountStepper({ count, onCount }: { count: number; onCount: (n: number) => void }) {
  return (
    <div className="inline-flex items-center gap-0.5">
      <button
        type="button"
        aria-label="Fewer instances"
        onClick={() => onCount(count - 1)}
        className="flex h-5 w-5 items-center justify-center rounded-sm bg-chip text-ink-3 hover:bg-hover-2"
      >
        −
      </button>
      <span
        data-testid="pattern-count"
        className="min-w-[16px] text-center font-mono text-[11.5px] text-ink-2"
      >
        {count}
      </span>
      <button
        type="button"
        aria-label="More instances"
        onClick={() => onCount(count + 1)}
        className="flex h-5 w-5 items-center justify-center rounded-sm bg-chip text-ink-3 hover:bg-hover-2"
      >
        +
      </button>
    </div>
  );
}

/** The accent Apply button shared by the boolean / pattern / mirror chips. */
function ApplyButton() {
  return (
    <button
      type="button"
      onClick={() => toolChipStore.getState().onApply?.()}
      className="rounded-sm bg-accent px-2 py-1 text-[11.5px] font-medium text-white hover:opacity-90"
    >
      Apply
    </button>
  );
}

export function ModelToolChips() {
  const engine = useViewportEngine();
  const kind = useToolChipStore((s) => s.kind);
  const value = useToolChipStore((s) => s.value);
  const count = useToolChipStore((s) => s.count);
  const axis = useToolChipStore((s) => s.axis);
  const plane = useToolChipStore((s) => s.plane);
  const op = useToolChipStore((s) => s.op);
  const suffix = useToolChipStore((s) => s.suffix);
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

  const numericChip = (suffix: string) => (
    <DimensionInput value={value} suffix={suffix} onCommit={(v) => toolChipStore.getState().onValue?.(v)} />
  );

  const panel = (children: React.ReactNode) => (
    <div className="pointer-events-auto inline-flex items-center gap-1 rounded-md border border-border bg-white p-1 shadow-panel">
      {children}
    </div>
  );

  let content: React.ReactNode;
  if (kind === "revolveAngle") {
    content = (
      <div className="pointer-events-auto inline-flex items-center gap-1">
        {numericChip("°")}
        <button
          type="button"
          onClick={() => toolChipStore.getState().onResetAxis?.()}
          className="rounded-sm bg-chip px-2 py-1 text-[11px] font-medium text-ink-3 hover:bg-hover-2"
        >
          Axis
        </button>
      </div>
    );
  } else if (kind === "extrudeDepth" || kind === "filletRadius" || kind === "shellThickness") {
    content = numericChip("mm");
  } else if (kind === "dimension") {
    // Sketch Dimension tool: seeded + auto-focused; Enter commits, Esc cancels,
    // and a canvas click must NOT blur-commit (a 2nd line click upgrades a length
    // into an angle), so `commitOnBlur` is off. Keying by anchor+value remounts
    // (re-focuses) on each new pick — e.g. when a length upgrades to an angle —
    // but stays stable while typing (the value prop is unchanged mid-edit).
    content = (
      <DimensionInput
        key={`dim-${anchorKey}-${value}`}
        value={value}
        suffix={suffix}
        autoFocus
        commitOnBlur={false}
        onCommit={(v) => toolChipStore.getState().onValue?.(v)}
        onCancel={() => toolChipStore.getState().onCancel?.()}
      />
    );
  } else if (kind === "linearPattern") {
    content = panel(
      <>
        <SegmentToggle
          options={PATTERN_AXES}
          active={axis}
          label="Pattern axis"
          onPick={(a) => toolChipStore.getState().onAxis?.(a)}
        />
        <CountStepper count={count} onCount={(n) => toolChipStore.getState().onCount?.(n)} />
        {numericChip("mm")}
        <ApplyButton />
      </>,
    );
  } else if (kind === "circularPattern") {
    content = panel(
      <>
        <SegmentToggle
          options={PATTERN_AXES}
          active={axis}
          label="Pattern axis"
          onPick={(a) => toolChipStore.getState().onAxis?.(a)}
        />
        <CountStepper count={count} onCount={(n) => toolChipStore.getState().onCount?.(n)} />
        {numericChip("°")}
        <ApplyButton />
      </>,
    );
  } else if (kind === "mirror") {
    content = panel(
      <>
        <SegmentToggle
          options={MIRROR_PLANES}
          active={plane}
          label="Mirror plane"
          onPick={(p) => toolChipStore.getState().onPlane?.(p)}
        />
        <ApplyButton />
      </>,
    );
  } else {
    // booleanOp
    content = panel(
      <>
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
        <ApplyButton />
      </>,
    );
  }

  return createPortal(content, host);
}
