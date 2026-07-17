# STEP reference files — N/A (no code-free producer in OneCAD-CPP)

**Status: not captured.** No reference STEP file is included because no existing
`OneCAD-CPP` binary produces STEP output **without code changes**.

## Why

- STEP export **exists** in the codebase — `OneCAD-CPP/src/io/step/StepExporter.{h,cpp}`
  (OCCT `STEPControl`), and import via `StepImporter` /
  `src/app/commands/ImportStepCommand.cpp`.
- But it is reachable **only through the Qt GUI**: `src/ui/dialogs/StepExportDialog.{h,cpp}`
  (a modal file dialog inside `MainWindow`). There is:
  - **no CLI flag** for export — `src/main.cpp` parses no `--export-step` / argv path
    (grep for `argv|--export|--step` over `src/main.cpp` and `src/ui/mainwindow/MainWindow.cpp`
    at the frozen commit returns nothing relevant);
  - **no prototype/test** that writes a `.step`/`.stp` file (grep `\.step|\.stp|StepExporter`
    over `tests/` returns only unrelated `stepIndex` history fields, not STEP files);
  - **no headless mode** that exports — `ONECAD_HEADLESS=1 make run` is a GUI smoke
    test (per `OneCAD-CPP/CLAUDE.md`), not a STEP exporter.

Producing a STEP file would require either driving the Qt UI (out of scope for a
read-only, no-code-change capture) or writing a new small harness that links
`StepExporter` — which the work-package constraint forbids (no new tracked files in
`OneCAD-CPP`, and no in-repo place to add one without editing its CMake).

## For the new stack

STEP is an M2 deliverable in the migration plan (vertical slice ends with "STEP
export"). The protocol already defines the verbs — `protocol/SCHEMA.md` §7.8
`ExportStep` (`schema: "AP214IS"`) and `ImportStep`. When the new worker's
`io.step` capability lands, generate 1–2 canonical reference STEP files from a corpus
case (e.g. `case_a` box, `case_c` boolean result) and drop them here as golden export
fixtures. Until then this is deliberately empty.

Frozen commit: `b4ddcccc48134531f3ff80f11ddf9f42ad5a967e`.
