import { useRepairStore, repairStore } from "@/stores/repairStore";

/**
 * Compact amber "N references need repair" pill (M4b). Shown only when the live
 * `needs-repair` state has items; clicking opens the repair panel. Auto-dismisses
 * when a later event carries empty items (the store drops the items → this
 * unmounts). Mirrors the StatusBar worker-dot pattern (a small dot + text, warn
 * tokens), so it reads as the same attention affordance.
 */
export function RepairBanner() {
  const count = useRepairStore((s) => s.items.length);
  if (count === 0) return null;
  const label = count === 1 ? "1 reference needs repair" : `${count} references need repair`;
  return (
    <button
      type="button"
      data-testid="repair-banner"
      onClick={() => repairStore.getState().openPanel()}
      className="absolute left-1/2 top-3 z-30 flex -translate-x-1/2 items-center gap-2 rounded-full border border-warn-border bg-warn-surface px-3 py-1.5 text-[12.5px] font-medium text-warn shadow-panel hover:border-warn"
    >
      <span aria-hidden="true" className="h-[7px] w-[7px] rounded-full bg-warn" />
      {label}
    </button>
  );
}
