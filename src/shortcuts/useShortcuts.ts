/*
 * Global keyboard handler (F-WP3). Installs one window keydown listener that
 * resolves bindings mode-scoped and runs the matched action against the stores.
 * Bails when a text input is focused and leaves OS/app chords (Cmd/Ctrl/Alt)
 * untouched so a later Save/Undo layer can own them.
 */
import { useEffect } from "react";
import { toolStore, activeTool } from "@/stores/toolStore";
import { selectionStore } from "@/stores/selectionStore";
import { viewportStore } from "@/stores/viewportStore";
import { getModelToolController } from "@/tools/modelTools/modelToolBridge";
import {
  openDocumentDialog,
  saveDocument,
  saveDocumentAs,
} from "@/features/shell/fileActions";
import { resolveBinding, type ShortcutAction } from "./keymap";

function isEditableTarget(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  const tag = el.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    el.isContentEditable
  );
}

/** Esc ladder: cancel active tool → deselect → exit sketch mode. */
function runCancel(): void {
  const tool = toolStore.getState();
  if (activeTool(tool) !== "select") {
    tool.setTool("select");
    return;
  }
  const sel = selectionStore.getState();
  if (sel.selected.length > 0) {
    sel.clear();
    return;
  }
  if (tool.mode === "sketch") {
    tool.setMode("model");
  }
}

export function runAction(action: ShortcutAction): void {
  const tool = toolStore.getState();
  switch (action.type) {
    case "tool":
      tool.setTool(action.tool);
      break;
    case "enterSketch":
      if (tool.mode === "model") tool.setMode("sketch");
      break;
    case "finishSketch":
      if (tool.mode === "sketch") {
        // Hand the just-finished sketch to the model layer to auto-arm extrude
        // (Shapr3D flow). The ModelToolController consumes this on mode → model.
        const sketchId = viewportStore.getState().activeSketchId;
        if (sketchId) viewportStore.getState().setPendingExtrude(sketchId);
        tool.setMode("model");
      }
      break;
    case "cancel":
      runCancel();
      break;
    case "zoomFit":
      viewportStore.getState().zoomFit();
      break;
    case "home":
      viewportStore.getState().homeView();
      break;
  }
}

export function useShortcuts(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // Undo / redo own the ⌘Z / ⇧⌘Z (and Ctrl+Y) chords (F-WP7).
      const mod = e.metaKey || e.ctrlKey;
      if (mod && (e.key === "z" || e.key === "Z")) {
        e.preventDefault();
        const ctrl = getModelToolController();
        if (e.shiftKey) void ctrl?.redo();
        else void ctrl?.undo();
        return;
      }
      if (mod && (e.key === "y" || e.key === "Y")) {
        e.preventDefault();
        void getModelToolController()?.redo();
        return;
      }
      // File chords own ⌘S (Save) / ⇧⌘S (Save As) / ⌘O (Open) in every mode; they
      // route through the shared fileActions bridge (Rust owns dialogs + fs).
      if (mod && (e.key === "s" || e.key === "S")) {
        e.preventDefault();
        if (e.shiftKey) void saveDocumentAs();
        else void saveDocument();
        return;
      }
      if (mod && !e.shiftKey && (e.key === "o" || e.key === "O")) {
        e.preventDefault();
        void openDocumentDialog();
        return;
      }
      // Leave remaining OS / app chords (Cmd/Ctrl/Alt) to their owners; Shift ok.
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (isEditableTarget(e.target)) return;
      if (e.repeat) return;
      const action = resolveBinding(e.key, e.shiftKey, toolStore.getState().mode);
      if (!action) return;
      e.preventDefault();
      runAction(action);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
