import { useRef, useState } from "react";
import { Icon } from "@/icons/Icon";
import { Popover } from "@/ui/Popover";
import { MonoValue } from "@/ui/MonoValue";
import {
  exportObj,
  exportStep,
  exportStl,
  openDocumentDialog,
  saveDocument,
  saveDocumentAs,
} from "./fileActions";

/**
 * Compact File menu in the title bar: Open… / Save / Save As… / Export STEP… /
 * Export STL… / Export OBJ…, each routed through the shared `fileActions` bridge
 * (same path the ⌘O/⌘S/⇧⌘S shortcuts use). Mirrors the start-screen SortMenu pattern
 * (a hairline trigger + anchored Popover) so it reuses the existing primitives +
 * design tokens.
 */
export function FileMenu() {
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement | null>(null);

  const run = (action: () => void | Promise<void>) => {
    setOpen(false);
    void action();
  };

  return (
    // Not a drag region (no data-tauri-drag-region), so the trigger stays clickable
    // inside the title bar's drag surface.
    <div className="relative">
      <button
        ref={btn}
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex h-[26px] cursor-pointer items-center gap-1 rounded-sm px-2 font-ui text-[12.5px] font-medium text-ink-3 hover:bg-hover"
      >
        File
        <Icon name="chevronDown" size={11} strokeWidth={2} className="text-ink-5" />
      </button>

      <Popover
        open={open}
        onClose={() => setOpen(false)}
        anchorRef={btn}
        placement="bottom-start"
        width={190}
        className="p-1"
      >
        <MenuItem label="Open…" shortcut="⌘O" onClick={() => run(openDocumentDialog)} />
        <MenuItem label="Save" shortcut="⌘S" onClick={() => run(saveDocument)} />
        <MenuItem label="Save As…" shortcut="⇧⌘S" onClick={() => run(saveDocumentAs)} />
        <div aria-hidden="true" className="my-1 h-px bg-border" />
        <MenuItem label="Export STEP…" onClick={() => run(exportStep)} />
        <MenuItem label="Export STL…" onClick={() => run(exportStl)} />
        <MenuItem label="Export OBJ…" onClick={() => run(exportObj)} />
      </Popover>
    </div>
  );
}

function MenuItem({
  label,
  shortcut,
  onClick,
}: {
  label: string;
  shortcut?: string;
  onClick: () => void;
}) {
  return (
    <div
      role="menuitem"
      onClick={onClick}
      className="flex h-[30px] cursor-pointer items-center gap-2 rounded-[5px] px-2.5 text-[12.5px] text-ink-2 hover:bg-well"
    >
      <span className="flex-1">{label}</span>
      {shortcut && <MonoValue className="text-[11px] text-ink-6">{shortcut}</MonoValue>}
    </div>
  );
}
