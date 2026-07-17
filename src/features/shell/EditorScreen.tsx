import { useShortcuts } from "@/shortcuts/useShortcuts";
import { TitleBar } from "./TitleBar";
import { StatusBar } from "./StatusBar";
import { NavPill } from "./NavPill";
import { CornerCluster } from "./CornerCluster";
import { FloatingToolbar } from "@/features/toolbar/FloatingToolbar";
import { ModelToolChips } from "@/features/toolbar/ModelToolChips";
import { ModelTreePanel } from "@/features/tree/ModelTreePanel";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { SketchChromeBar } from "@/features/sketch/SketchChromeBar";
import { ConstraintBadgeLayer } from "@/features/sketch/ConstraintBadgeLayer";
import { ViewportRoot } from "@/viewport/ViewportRoot";

/**
 * Editor shell (design variant 1c) — the full floating chrome over the live
 * Three.js viewport (ViewportRoot). ViewportRoot renders the real canvas and
 * falls back to a hatched placeholder while the engine loads. The status-bar
 * strip occupies the bottom 34px, so the viewport sits above it.
 */
export function EditorScreen() {
  useShortcuts();

  return (
    <div className="flex h-full w-full select-none flex-col overflow-hidden bg-white font-ui">
      <TitleBar />
      <div className="relative min-h-0 flex-1">
        <ViewportRoot className="absolute inset-x-0 bottom-[34px] top-0" />
        <ConstraintBadgeLayer />
        <ModelToolChips />

        <FloatingToolbar />
        <SketchChromeBar />
        <ModelTreePanel />
        <InspectorPanel />
        <CornerCluster />
        <NavPill />
        <StatusBar />
      </div>
    </div>
  );
}
