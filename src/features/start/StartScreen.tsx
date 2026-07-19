import { useEffect, useMemo, useState } from "react";
import { Button } from "@/ui/Button";
import { TextInput } from "@/ui/TextInput";
import { SectionLabel } from "@/ui/SectionLabel";
import { MonoValue } from "@/ui/MonoValue";
import { Icon } from "@/icons/Icon";
import { useAppStore } from "@/stores/appStore";
import type { RecentProject } from "@/ipc/types";
import { RecentGrid } from "./RecentGrid";
import { RecoveryCard } from "./RecoveryCard";
import { SortMenu, type SortKey } from "./SortMenu";

const APP_VERSION = "v0.1.0";

/** Filter (case-insensitive substring on name) + sort — mirrors prototype 1a. */
function filterSort(
  recents: RecentProject[],
  query: string,
  sort: SortKey,
): RecentProject[] {
  const q = query.toLowerCase();
  const list = recents.filter((p) => p.name.toLowerCase().includes(q));
  return [...list].sort(
    sort === "name"
      ? (a, b) => a.name.localeCompare(b.name)
      : (a, b) => Date.parse(b.modifiedAt) - Date.parse(a.modifiedAt),
  );
}

/**
 * Start screen — variant 1a "card on canvas": a centered 780px card holding the
 * wordmark, the New / Open / Import actions, and the searchable + sortable
 * recent-projects grid.
 */
export function StartScreen() {
  const recents = useAppStore((s) => s.recents);
  const recentsStatus = useAppStore((s) => s.recentsStatus);
  const loadRecents = useAppStore((s) => s.loadRecents);
  const newProject = useAppStore((s) => s.newProject);
  const openProject = useAppStore((s) => s.openProject);
  const openDialogAndOpen = useAppStore((s) => s.openDialogAndOpen);
  const importStep = useAppStore((s) => s.importStep);
  const recovery = useAppStore((s) => s.recovery);
  const recoveryStatus = useAppStore((s) => s.recoveryStatus);
  const checkRecovery = useAppStore((s) => s.checkRecovery);
  const recoverDocument = useAppStore((s) => s.recoverDocument);
  const discardRecovery = useAppStore((s) => s.discardRecovery);

  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>("date");

  useEffect(() => {
    if (recentsStatus === "idle") void loadRecents();
  }, [recentsStatus, loadRecents]);

  useEffect(() => {
    if (recoveryStatus === "idle") void checkRecovery();
  }, [recoveryStatus, checkRecovery]);

  const list = useMemo(
    () => filterSort(recents, query, sort),
    [recents, query, sort],
  );
  const ready = recentsStatus === "ready";

  return (
    <div className="flex h-full w-full items-center justify-center bg-canvas-start font-ui text-ink">
      <div className="w-[780px] max-w-[calc(100%-32px)] rounded-lg border border-border bg-white px-7 pb-7 pt-[26px] shadow-start-card">
        {/* Wordmark + version */}
        <div className="mb-[18px] flex items-baseline gap-2">
          <span className="text-[16px] font-bold tracking-[-0.01em] text-ink">
            OneCAD
          </span>
          <span className="flex-1" />
          <MonoValue className="text-[11px] text-ink-6">{APP_VERSION}</MonoValue>
        </div>

        {/* Primary actions */}
        <div className="mb-[22px] flex gap-2.5">
          <Button variant="primary" size="lg" onClick={() => void newProject()}>
            <Icon name="plus" size={15} strokeWidth={2} />
            New project
          </Button>
          <Button variant="secondary" size="lg" onClick={() => void openDialogAndOpen()}>
            Open…
          </Button>
          <Button variant="secondary" size="lg" onClick={() => void importStep()}>
            <Icon name="import" size={15} />
            Import STEP…
          </Button>
        </div>

        {/* Crash-recovery offer (a crashed session left an autosave) */}
        {recovery && (
          <RecoveryCard
            recovery={recovery}
            onRestore={recoverDocument}
            onDiscard={discardRecovery}
          />
        )}

        {/* Recent header: label · search · sort */}
        <div className="mb-[14px] flex items-center gap-2.5">
          <SectionLabel>Recent projects</SectionLabel>
          <span className="flex-1" />
          <TextInput
            leadingIcon="search"
            placeholder="Search projects…"
            aria-label="Search projects"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            wrapperClassName="w-[220px]"
          />
          <SortMenu value={sort} onChange={setSort} />
        </div>

        {ready && list.length > 0 && (
          <RecentGrid projects={list} onOpen={openProject} />
        )}
        {ready && list.length === 0 && (
          <EmptyState searching={query.length > 0} />
        )}
      </div>
    </div>
  );
}

/**
 * Empty grid state. When a search is active the prototype's exact copy is used;
 * with no recents at all we add a short hint pointing at the actions above.
 */
function EmptyState({ searching }: { searching: boolean }) {
  if (searching) {
    return (
      <div className="pb-5 pt-9 text-center text-[12.5px] text-ink-6">
        No projects match your search.
      </div>
    );
  }
  return (
    <div className="pb-5 pt-9 text-center">
      <div className="text-[12.5px] text-ink-6">No recent projects yet.</div>
      <div className="mt-1.5 text-[11.5px] text-ink-7">
        Start a new project or open an existing file.
      </div>
    </div>
  );
}
