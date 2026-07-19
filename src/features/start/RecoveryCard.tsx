import { Button } from "@/ui/Button";
import type { RecoveryInfo } from "@/ipc/types";

type RecoveryCardProps = {
  recovery: RecoveryInfo;
  onRestore: () => void;
  onDiscard: () => void;
};

/** File name (no extension) from an absolute path, or a fallback when never saved. */
function documentName(originalPath?: string): string {
  if (!originalPath) return "Untitled document";
  const file = originalPath.split(/[\\/]/).pop() ?? originalPath;
  return file.replace(/\.[^.]+$/, "");
}

/** Human date for the autosave mtime (mirrors ProjectCard's en-US short form). */
function formatModified(modifiedMs: number): string {
  return new Date(modifiedMs).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

/**
 * Crash-recovery attention card (start screen). Shown above the recent-projects
 * list when a crashed session left an autosave behind; offers to Restore it or
 * Discard it. Warn-token styling mirrors RepairBanner / RepairPanel so it reads as
 * the same "needs attention" affordance.
 */
export function RecoveryCard({ recovery, onRestore, onDiscard }: RecoveryCardProps) {
  return (
    <div className="mb-[22px] rounded-md border border-warn-border bg-warn-surface px-[15px] py-3">
      <div className="flex items-center gap-2.5">
        <span aria-hidden="true" className="h-[7px] w-[7px] shrink-0 rounded-full bg-warn" />
        <div className="flex-1">
          <div className="text-[13px] font-semibold text-warn-strong">
            Unsaved changes recovered
          </div>
          <div className="mt-[3px] text-[11.5px] text-warn">
            <span className="font-medium">{documentName(recovery.originalPath)}</span>
            {" · autosaved "}
            {formatModified(recovery.modifiedMs)}
          </div>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button variant="ghost" size="sm" onClick={onDiscard}>
            Discard
          </Button>
          <Button variant="primary" size="sm" onClick={onRestore}>
            Restore
          </Button>
        </div>
      </div>
    </div>
  );
}
