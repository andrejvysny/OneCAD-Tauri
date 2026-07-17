import { SegmentedToggle } from "@/ui/SegmentedToggle";
import { useDocumentStore } from "@/stores/documentStore";
import { useToolStore, type EditorMode } from "@/stores/toolStore";

/**
 * 44px overlay title bar (prototype 1c). Reserves the left inset for the native
 * macOS traffic lights (Tauri titleBarStyle Overlay draws them over the webview,
 * so the app does not paint its own) and is a drag region. Centered-left doc
 * title + dirty dot; right-aligned Model⇄Sketch toggle.
 */
export function TitleBar() {
  const title = useDocumentStore((s) => s.title);
  const dirty = useDocumentStore((s) => s.dirty);
  const mode = useToolStore((s) => s.mode);
  const setMode = useToolStore((s) => s.setMode);

  return (
    <div
      data-tauri-drag-region
      className="flex h-[44px] flex-none select-none items-center gap-2 border-b border-border bg-titlebar px-4"
    >
      {/* Native traffic-light reservation (OS-drawn in overlay mode). */}
      <span data-tauri-drag-region aria-hidden="true" className="w-[54px] flex-none" />
      <span
        data-tauri-drag-region
        className="flex items-center gap-2 text-[13px] font-semibold text-titlebar-text"
      >
        OneCAD — {title}
        {dirty && (
          <span
            aria-label="Unsaved changes"
            className="h-[7px] w-[7px] rounded-full bg-ink-5"
          />
        )}
      </span>
      <span data-tauri-drag-region className="flex-1" />
      <SegmentedToggle
        ariaLabel="Editing mode"
        size="md"
        value={mode}
        onChange={(m: EditorMode) => setMode(m)}
        options={[
          { value: "model", label: "Model" },
          { value: "sketch", label: "Sketch" },
        ]}
      />
    </div>
  );
}
