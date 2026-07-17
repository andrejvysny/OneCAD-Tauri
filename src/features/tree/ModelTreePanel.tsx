import { SectionLabel } from "@/ui/SectionLabel";
import { useDocumentStore } from "@/stores/documentStore";
import {
  useSelectionStore,
  type EntityKind,
} from "@/stores/selectionStore";
import { useToolStore } from "@/stores/toolStore";
import { TreeRow } from "./TreeRow";

/**
 * Floating model tree (prototype 1c): BODIES + SKETCHES sections driven by the
 * document projection. Click selects; double-clicking a sketch enters sketch
 * mode; the eye toggles visibility in the document store.
 */
export function ModelTreePanel() {
  const bodies = useDocumentStore((s) => s.bodies);
  const sketches = useDocumentStore((s) => s.sketches);
  const setVisibility = useDocumentStore((s) => s.setVisibility);
  const selected = useSelectionStore((s) => s.selected);
  const select = useSelectionStore((s) => s.set);
  const setMode = useToolStore((s) => s.setMode);

  const isSelected = (kind: EntityKind, id: string) =>
    selected.some((r) => r.kind === kind && r.id === id);

  return (
    <div className="absolute bottom-24 left-3 top-3 z-20 w-[220px] overflow-auto rounded-md border border-border bg-white pb-1.5 shadow-panel">
      <SectionLabel className="px-[14px] pb-1 pt-3">Bodies</SectionLabel>
      <div role="listbox" aria-label="Bodies">
        {Object.values(bodies).map((b) => (
          <TreeRow
            key={b.id}
            name={b.name}
            icon="cube"
            visible={b.visible}
            selected={isSelected("body", b.id)}
            onSelect={() => select([{ kind: "body", id: b.id }])}
            onToggleVisible={(v) => setVisibility(b.id, v)}
          />
        ))}
      </div>

      <SectionLabel className="px-[14px] pb-1 pt-3">Sketches</SectionLabel>
      <div role="listbox" aria-label="Sketches">
        {Object.values(sketches).map((s) => (
          <TreeRow
            key={s.id}
            name={s.name}
            icon="pen"
            visible={s.visible}
            selected={isSelected("sketch", s.id)}
            onSelect={() => select([{ kind: "sketch", id: s.id }])}
            onToggleVisible={(v) => setVisibility(s.id, v)}
            onActivate={() => setMode("sketch", s.id)}
          />
        ))}
      </div>
    </div>
  );
}
