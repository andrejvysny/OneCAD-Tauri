import type { RecentProject } from "@/ipc/types";

type ProjectCardProps = {
  project: RecentProject;
  onOpen: (path: string) => void;
};

// Faint 45° hatch that fills the empty preview well (prototype 1a, line 76).
const PREVIEW_HATCH =
  "repeating-linear-gradient(45deg, rgba(0,0,0,0.025) 0 10px, transparent 10px 20px)";

function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

/**
 * One recent-project tile: a preview well over a name + path/date meta line.
 * Hover lifts the card with an accent-tinted border + soft shadow.
 */
export function ProjectCard({ project, onOpen }: ProjectCardProps) {
  return (
    <button
      type="button"
      title={project.path}
      onClick={() => onOpen(project.path)}
      className="block w-full cursor-pointer overflow-hidden rounded-lg border border-border bg-white text-left transition-[box-shadow,border-color] duration-150 hover:border-card-hover-border hover:shadow-card-hover"
    >
      <div
        className="flex h-24 items-center justify-center border-b border-border-subtle bg-well"
        style={project.thumbnail ? undefined : { backgroundImage: PREVIEW_HATCH }}
      >
        {project.thumbnail ? (
          <img
            src={project.thumbnail}
            alt=""
            className="h-full w-full object-cover"
          />
        ) : (
          <span className="font-mono text-[10px] text-ink-7">model preview</span>
        )}
      </div>
      <div className="px-[11px] pb-[10px] pt-[9px]">
        <div className="truncate text-[13px] font-semibold text-ink">
          {project.name}
        </div>
        <div className="mt-[3px] flex gap-1.5 text-[11px] text-ink-5">
          <span className="flex-1 truncate">{project.path}</span>
          <span className="shrink-0">{formatDate(project.modifiedAt)}</span>
        </div>
      </div>
    </button>
  );
}
