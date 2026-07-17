/*
 * Bridge to the live ModelToolController (mirrors engineBridge). ViewportRoot
 * registers the controller on init; React chrome outside the viewport (the
 * HistoryList double-click → re-edit extrude) reaches it here.
 */
import type { ModelToolController } from "./ModelToolController";

let current: ModelToolController | null = null;

export function setModelToolController(c: ModelToolController | null): void {
  current = c;
}

export function getModelToolController(): ModelToolController | null {
  return current;
}
