import { cn } from "@/ui/cn";
import { SegmentedToggle } from "@/ui/SegmentedToggle";
import { MonoValue } from "@/ui/MonoValue";
import { useToolStore } from "@/stores/toolStore";
import { useSelectionStore, primarySelection } from "@/stores/selectionStore";
import { useDocumentStore } from "@/stores/documentStore";
import { useWorkerStore, type WorkerLifecycleState } from "@/stores/workerStore";
import {
  useViewportStore,
  formatCursor,
  type Projection,
} from "@/stores/viewportStore";

/**
 * 34px status bar (prototype 1c): status text · DOF (warn when >0) · spacer ·
 * Persp/Ortho toggle · FOV (dimmed in ortho) · live mono X/Y/Z read-out.
 */
export function StatusBar() {
  const mode = useToolStore((s) => s.mode);
  const sel = useSelectionStore(primarySelection);
  const projection = useViewportStore((s) => s.projection);
  const setProjection = useViewportStore((s) => s.setProjection);
  const fov = useViewportStore((s) => s.fov);
  const cursor = useViewportStore((s) => s.cursor);
  const dofBadge = useViewportStore((s) => s.dofBadge);
  const statusHint = useViewportStore((s) => s.statusHint);
  const workerState = useWorkerStore((s) => s.state);
  const activeSketchId = useViewportStore((s) => s.activeSketchId);
  const activeSketch = useDocumentStore((s) =>
    activeSketchId ? s.sketches[activeSketchId] : undefined,
  );

  const sketching = mode === "sketch";
  // Prototype: showDof = sketch-mode OR (a non-body entity is selected).
  const showDof = sketching || (!!sel && sel.kind !== "body");
  const persp = projection === "persp";
  const statusLeft = sketching
    ? `Sketch mode — ${activeSketch?.name ?? "Sketch"}`
    : "Ready";
  const dofText = showDof ? `DOF: ${dofBadge ?? 0}` : "DOF: —";

  return (
    <div className="absolute inset-x-0 bottom-0 z-[26] flex h-[34px] items-center gap-3 border-t border-border bg-statusbar px-3.5 text-[12px]">
      <span className="text-ink-3">{statusLeft}</span>
      {statusHint && (
        <>
          <span aria-hidden="true" className="h-[14px] w-px bg-border" />
          <span className="text-ink-5">{statusHint}</span>
        </>
      )}
      <WorkerStatusIndicator state={workerState} />
      <span aria-hidden="true" className="h-[14px] w-px bg-border" />
      <span className={cn("font-medium", showDof ? "text-warn" : "text-ink-6")}>
        {dofText}
      </span>
      <span className="flex-1" />
      <SegmentedToggle
        ariaLabel="Projection"
        size="sm"
        value={projection}
        onChange={(p: Projection) => setProjection(p)}
        options={[
          { value: "persp", label: "Persp" },
          { value: "ortho", label: "Ortho" },
        ]}
      />
      <span
        data-testid="fov"
        className="text-ink-5"
        style={{ opacity: persp ? 1 : 0.35 }}
      >
        FOV <MonoValue>{fov}°</MonoValue>
      </span>
      <span aria-hidden="true" className="h-[14px] w-px bg-border" />
      <MonoValue className="whitespace-pre text-[11.5px]">
        {formatCursor(cursor)}
      </MonoValue>
    </div>
  );
}

/**
 * Worker-status indicator (a small dot + label). Shown ONLY for the attention
 * states — the sidecar restarting (amber) or failed (red); the healthy
 * starting/ready path stays quiet. The mock never emits, so this renders nothing
 * in a plain browser / vitest.
 */
function WorkerStatusIndicator({ state }: { state: WorkerLifecycleState }) {
  if (state !== "restarting" && state !== "failed") return null;
  const failed = state === "failed";
  return (
    <>
      <span aria-hidden="true" className="h-[14px] w-px bg-border" />
      <span
        role="status"
        className={cn("flex items-center gap-1.5", failed ? "text-traffic-close" : "text-warn")}
      >
        <span
          aria-hidden="true"
          className={cn("h-[7px] w-[7px] rounded-full", failed ? "bg-traffic-close" : "bg-warn")}
        />
        {failed ? "Worker offline" : "Worker restarting…"}
      </span>
    </>
  );
}
