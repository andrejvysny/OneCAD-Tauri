import { useCallback, useState } from "react";
import { cn } from "@/ui/cn";
import { Icon } from "@/icons/Icon";
import { SectionLabel } from "@/ui/SectionLabel";
import { useRepairStore, repairStore } from "@/stores/repairStore";
import { useDocumentStore } from "@/stores/documentStore";
import { createClient } from "@/ipc/client";
import { rebindCandidate } from "@/features/inspector/historyActions";
import type { NeedsRepairItem, ResolveCandidate } from "@/ipc/types";

type CandidateLoad =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; candidates: ResolveCandidate[] };

/** Humanize a repair reason token for display. */
function reasonText(reason: string): string {
  switch (reason) {
    case "ambiguous":
      return "Ambiguous — several candidates match";
    case "no-candidates":
      return "No candidate matched";
    case "low-confidence":
      return "Low confidence — no clear match";
    default:
      return reason;
  }
}

/** The trailing `input<k>` slot of a refId (or the whole id if it has no dot). */
function refTail(refId: string): string {
  const dot = refId.lastIndexOf(".");
  return dot >= 0 ? refId.slice(dot + 1) : refId;
}

/**
 * The inspector REPAIR state (M4b): one card per NeedsRepair ref. Expanding a card
 * fetches the ranked candidates via `resolveRefs` and renders each as a score
 * meter + summary; clicking a candidate rebinds the ref (promote → EditOperationInput).
 */
export function RepairPanel() {
  const items = useRepairStore((s) => s.items);
  const expandedRefId = useRepairStore((s) => s.expandedRefId);
  const features = useDocumentStore((s) => s.features);
  const [loads, setLoads] = useState<Record<string, CandidateLoad>>({});
  const [busyRefId, setBusyRefId] = useState<string | null>(null);

  const labelFor = useCallback(
    (item: NeedsRepairItem): string => {
      const feat = features.find((f) => f.id === item.opId);
      // Fall back to a short opId prefix when the feature is not in the projection.
      return feat?.label ?? `Feature ${item.opId.slice(0, 6)}`;
    },
    [features],
  );

  const expand = useCallback((item: NeedsRepairItem) => {
    const next = repairStore.getState().expandedRefId === item.refId ? null : item.refId;
    repairStore.getState().setExpanded(next);
    if (next && !loads[item.refId]) {
      setLoads((s) => ({ ...s, [item.refId]: { status: "loading" } }));
      createClient()
        .resolveRefs([{ refId: item.refId }])
        .then((results) => {
          const r = results.find((x) => x.refId === item.refId) ?? results[0];
          const candidates = [...(r?.candidates ?? [])].sort((a, b) => b.score - a.score);
          setLoads((s) => ({ ...s, [item.refId]: { status: "ready", candidates } }));
        })
        .catch((e: unknown) => {
          setLoads((s) => ({
            ...s,
            [item.refId]: { status: "error", message: e instanceof Error ? e.message : String(e) },
          }));
        });
    }
  }, [loads]);

  const choose = useCallback(async (item: NeedsRepairItem, candidate: ResolveCandidate) => {
    setBusyRefId(item.refId);
    try {
      await rebindCandidate(item, candidate);
    } finally {
      setBusyRefId(null);
      repairStore.getState().setHoveredWorldPos(null);
    }
  }, []);

  return (
    <>
      <div className="flex items-center justify-between">
        <div className="text-[15px] font-semibold text-ink">Repair references</div>
        <button
          type="button"
          aria-label="Close repair panel"
          data-testid="repair-close"
          onClick={() => repairStore.getState().closePanel()}
          className="flex h-6 w-6 items-center justify-center rounded-sm text-ink-5 hover:bg-hover-2"
        >
          <Icon name="x" size={14} strokeWidth={1.8} />
        </button>
      </div>
      <div className="mt-0.5 text-[12px] text-warn">
        {items.length === 1 ? "1 reference" : `${items.length} references`} could not be re-bound after
        the last edit.
      </div>

      <SectionLabel className="pb-1.5 pt-4">References</SectionLabel>
      <div>
        {items.map((item) => {
          const open = expandedRefId === item.refId;
          const load = loads[item.refId];
          const busy = busyRefId === item.refId;
          return (
            <div
              key={item.refId}
              data-testid={`repair-item-${item.refId}`}
              className={cn(
                "mb-1.5 rounded-sm border",
                open ? "border-warn-border bg-warn-surface" : "border-border bg-chip",
              )}
            >
              <button
                type="button"
                data-testid={`repair-item-head-${item.refId}`}
                onClick={() => expand(item)}
                disabled={busy}
                className="flex w-full items-center gap-2 px-2.5 py-2 text-left"
              >
                <Icon
                  name="chevronDown"
                  size={13}
                  strokeWidth={1.8}
                  className={cn("text-ink-5 transition-transform", open ? "" : "-rotate-90")}
                />
                <span className="flex-1">
                  <span className="block text-[12.5px] font-medium text-ink-2">{labelFor(item)}</span>
                  <span className="block text-[11.5px] text-ink-5">
                    {reasonText(item.reason)} · {refTail(item.refId)} · {item.candidateCount}{" "}
                    {item.candidateCount === 1 ? "candidate" : "candidates"}
                  </span>
                </span>
              </button>

              {open && (
                <div className="border-t border-warn-border px-2.5 py-2">
                  {(!load || load.status === "loading") && (
                    <div className="text-[11.5px] text-ink-5">Resolving candidates…</div>
                  )}
                  {load?.status === "error" && (
                    <div className="text-[11.5px] text-warn-strong">{load.message}</div>
                  )}
                  {load?.status === "ready" && load.candidates.length === 0 && (
                    <div className="text-[11.5px] text-ink-5">No candidates to choose from.</div>
                  )}
                  {load?.status === "ready" &&
                    load.candidates.map((c, i) => (
                      <CandidateRow
                        key={`${c.topoKey}-${i}`}
                        refId={item.refId}
                        candidate={c}
                        disabled={busy}
                        onChoose={() => void choose(item, c)}
                      />
                    ))}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </>
  );
}

/** One candidate row: a score meter + summary + topoKey; click rebinds. */
function CandidateRow({
  refId,
  candidate,
  disabled,
  onChoose,
}: {
  refId: string;
  candidate: ResolveCandidate;
  disabled: boolean;
  onChoose: () => void;
}) {
  const pct = Math.max(0, Math.min(100, Math.round(candidate.score * 100)));
  return (
    <button
      type="button"
      data-testid={`repair-candidate-${refId}-${candidate.topoKey}`}
      disabled={disabled}
      onClick={onChoose}
      // Hover publishes the candidate world position to the repair store — a clean
      // DATA seam a future engine marker can subscribe to (no engine coupling here).
      onMouseEnter={() => repairStore.getState().setHoveredWorldPos(candidate.worldPos)}
      onMouseLeave={() => repairStore.getState().setHoveredWorldPos(null)}
      className={cn(
        "mb-1 flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left",
        disabled ? "opacity-50" : "bg-white hover:bg-hover-2",
      )}
    >
      <span className="flex-1">
        <span className="block text-[12px] text-ink-2">{candidate.summary}</span>
        <span className="mt-1 block h-[4px] w-full overflow-hidden rounded-full bg-well">
          <span className="block h-full rounded-full bg-accent" style={{ width: `${pct}%` }} />
        </span>
      </span>
      <span className="w-9 text-right text-[11px] font-medium tabular-nums text-ink-4">{pct}%</span>
    </button>
  );
}
