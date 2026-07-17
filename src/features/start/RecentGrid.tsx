import type { RecentProject } from "@/ipc/types";
import { ProjectCard } from "./ProjectCard";

type RecentGridProps = {
  projects: RecentProject[];
  onOpen: (path: string) => void;
};

/** 3-column grid of recent-project cards (prototype 1a, line 73). */
export function RecentGrid({ projects, onOpen }: RecentGridProps) {
  return (
    <div className="grid grid-cols-3 gap-3">
      {projects.map((p) => (
        <ProjectCard key={p.id} project={p} onOpen={onOpen} />
      ))}
    </div>
  );
}
