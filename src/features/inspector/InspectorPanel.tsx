import { Icon } from "@/icons/Icon";
import { SectionLabel } from "@/ui/SectionLabel";
import {
  useDocumentStore,
  type BodyMeta,
  type FeatureMeta,
  type SketchMeta,
} from "@/stores/documentStore";
import {
  useSelectionStore,
  primarySelection,
  selectionStore,
  type EntityRef,
} from "@/stores/selectionStore";
import { useToolStore } from "@/stores/toolStore";
import { useViewportStore } from "@/stores/viewportStore";
import { useSketchStore } from "@/stores/sketchStore";
import { getModelToolController } from "@/tools/modelTools/modelToolBridge";
import { HistoryList } from "./HistoryList";
import { ConstraintList, summarizeConstraints } from "./ConstraintList";
import { cn } from "@/ui/cn";
import { sketchStatusText, sketchStatusSentence } from "@/features/sketch/constraintStatus";
import type { SketchStatus } from "@/stores/documentStore";
import type { SketchConstraint } from "@/ipc/types";

/** Click a history chip → select that feature; double-click Extrude → re-edit. */
function selectFeature(id: string): void {
  selectionStore.getState().set([{ kind: "feature", id }]);
}
function editFeature(item: FeatureMeta): void {
  if (item.kind === "extrude") getModelToolController()?.editExtrudeFeature(item.id);
}

/**
 * Context-aware inspector (prototype 1c), three states:
 *  - EMPTY     — nothing selected in model mode
 *  - SELECTION — a body/sketch selected in model mode (status + HISTORY)
 *  - SKETCH    — sketch mode (DOF warn card + CONSTRAINTS)
 */
export function InspectorPanel() {
  const mode = useToolStore((s) => s.mode);
  const sel = useSelectionStore(primarySelection);
  const bodies = useDocumentStore((s) => s.bodies);
  const sketches = useDocumentStore((s) => s.sketches);
  const features = useDocumentStore((s) => s.features);
  const activeSketchId = useViewportStore((s) => s.activeSketchId);
  const constraints = useSketchStore((s) => s.session?.constraints);

  const sketching = mode === "sketch";

  return (
    <div className="absolute right-3 top-3 z-20 box-border w-[260px] rounded-md border border-border bg-white p-4 shadow-panel">
      {sketching ? (
        <SketchState
          sketchName={sketches[activeSketchId ?? ""]?.name ?? "Sketch"}
          dof={sketches[activeSketchId ?? ""]?.dof ?? 0}
          status={sketches[activeSketchId ?? ""]?.status ?? "under"}
          constraints={constraints ?? []}
        />
      ) : sel && sel.kind === "feature" ? (
        <FeatureState featureId={sel.id} features={features} />
      ) : sel ? (
        <SelectionState sel={sel} bodies={bodies} sketches={sketches} features={features} />
      ) : (
        <EmptyState />
      )}
    </div>
  );
}

function EmptyState() {
  return (
    <div className="px-2 py-[26px] text-center">
      <div className="mx-auto mb-2.5 flex h-10 w-10 items-center justify-center rounded-full bg-well">
        <Icon name="select" size={18} strokeWidth={1.7} className="text-ink-6" />
      </div>
      <div className="text-[13px] font-semibold text-ink-3">Nothing selected</div>
      <div className="mt-1 text-[12px] leading-normal text-ink-6">
        Select a body, sketch, face or edge to see its parameters and history.
      </div>
    </div>
  );
}

function SelectionState({
  sel,
  bodies,
  sketches,
  features,
}: {
  sel: EntityRef;
  bodies: Record<string, BodyMeta>;
  sketches: Record<string, SketchMeta>;
  features: FeatureMeta[];
}) {
  const isBody = sel.kind === "body";
  const name = bodies[sel.id]?.name ?? sketches[sel.id]?.name ?? "";
  const statusName = isBody ? "Solid body · 6 faces" : "Sketch · 2 profiles";
  // Body → its full lineage (Sketch 1 / Extrude / Fillet); sketch → the extrude
  // that consumed it (prototype's two hardcoded HISTORY arrays).
  const history = isBody
    ? features.slice(0, 3)
    : features.filter((f) => f.kind === "extrude").slice(0, 1);
  const showDof = !isBody;
  const dof = sketches[sel.id]?.dof ?? 0;

  return (
    <>
      <div className="text-[15px] font-semibold text-ink">{name}</div>
      <div className="mt-0.5 text-[12px] text-ink-5">{statusName}</div>
      {showDof && (
        <div className="mt-1 text-[12px] font-medium text-warn">
          Under-constrained · DOF {dof}
        </div>
      )}

      <SectionLabel className="pb-1.5 pt-4">History</SectionLabel>
      <HistoryList items={history} onSelect={selectFeature} onEdit={editFeature} />

      {showDof && (
        <>
          <SectionLabel className="pb-1.5 pt-3.5">Constraints</SectionLabel>
          <div className="text-[12px] leading-normal text-ink-6">
            Select geometry to constrain.
          </div>
        </>
      )}
    </>
  );
}

function FeatureState({
  featureId,
  features,
}: {
  featureId: string;
  features: FeatureMeta[];
}) {
  const feat = features.find((f) => f.id === featureId);
  return (
    <>
      <div className="text-[15px] font-semibold text-ink">{feat?.label ?? "Feature"}</div>
      <div className="mt-0.5 text-[12px] text-ink-5">
        {feat?.kind ? `${cap(feat.kind)} feature` : "Feature"}
        {feat?.valueText ? ` · ${feat.valueText}` : ""}
      </div>
      {feat?.kind === "extrude" && (
        <div className="mt-1 text-[12px] text-ink-6">Double-click to edit the depth.</div>
      )}
      <SectionLabel className="pb-1.5 pt-4">History</SectionLabel>
      <HistoryList items={features} selectedId={featureId} onSelect={selectFeature} onEdit={editFeature} />
    </>
  );
}

const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

function SketchState({
  sketchName,
  dof,
  status,
  constraints,
}: {
  sketchName: string;
  dof: number;
  status: SketchStatus;
  constraints: SketchConstraint[];
}) {
  const { label, tone } = sketchStatusText(status, dof);
  const solved = status === "ok";
  const rows = summarizeConstraints(constraints);
  return (
    <>
      <div className="text-[15px] font-semibold text-ink">{sketchName}</div>

      {/* DOF state card (1e treatment folded into 1c per WP spec). */}
      <div
        className={cn(
          "mt-3 rounded-md border px-3 py-2.5",
          solved ? "border-border bg-well" : "border-warn-border bg-warn-surface",
        )}
      >
        <div className={cn("text-[12px] font-medium", tone === "ok" ? "text-ink-4" : "text-warn")}>
          {label}
        </div>
        <div className={cn("mt-1 text-[12px] leading-normal", solved ? "text-ink-5" : "text-warn-strong")}>
          {sketchStatusSentence(status, dof)}
        </div>
      </div>

      <SectionLabel className="pb-1.5 pt-4">Constraints</SectionLabel>
      {rows.length > 0 ? (
        <ConstraintList items={rows} />
      ) : (
        <div className="text-[12px] leading-normal text-ink-6">
          No constraints yet.
        </div>
      )}
      <div className="mt-2 text-[11.5px] leading-normal text-ink-6">
        Drag geometry or add constraints until DOF reaches 0.
      </div>
    </>
  );
}
