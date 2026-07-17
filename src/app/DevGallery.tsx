import { useRef, useState, type ReactNode } from "react";
import { Icon } from "@/icons/Icon";
import { ICON_PATHS, type IconName } from "@/icons/paths";
import { Button } from "@/ui/Button";
import { SegmentedToggle } from "@/ui/SegmentedToggle";
import { Switch } from "@/ui/Switch";
import { Tooltip } from "@/ui/Tooltip";
import { Popover } from "@/ui/Popover";
import { SectionLabel } from "@/ui/SectionLabel";
import { MonoValue } from "@/ui/MonoValue";
import { TextInput } from "@/ui/TextInput";
import { EyeToggle } from "@/ui/EyeToggle";

/** Grouping card: a section label above a white panel of specimens. */
function Section({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3">
      <SectionLabel>{label}</SectionLabel>
      <div className="rounded-lg border border-border bg-white p-5 shadow-card">
        {children}
      </div>
    </section>
  );
}

/** A labelled specimen cell inside a section. */
function Cell({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-2">
      <span className="font-ui text-[11px] text-ink-6">{label}</span>
      <div className="flex flex-wrap items-center gap-3">{children}</div>
    </div>
  );
}

export default function DevGallery() {
  const [mode, setMode] = useState<"model" | "sketch">("model");
  const [proj, setProj] = useState<"persp" | "ortho">("persp");
  const [snapGrid, setSnapGrid] = useState(true);
  const [snapEdges, setSnapEdges] = useState(false);
  const [eyeA, setEyeA] = useState(true);
  const [eyeB, setEyeB] = useState(false);
  const [snapOpen, setSnapOpen] = useState(false);
  const [showGrid, setShowGrid] = useState(true);
  const [showHints, setShowHints] = useState(true);
  const snapBtn = useRef<HTMLButtonElement | null>(null);

  const iconNames = Object.keys(ICON_PATHS) as IconName[];

  return (
    <div className="min-h-full w-full overflow-auto bg-canvas font-ui text-ink">
      <div className="mx-auto flex max-w-[980px] flex-col gap-10 px-8 py-10">
        <header className="flex items-baseline gap-3">
          <h1 className="text-[20px] font-bold tracking-tight">
            OneCAD · DevGallery
          </h1>
          <MonoValue className="text-[11px] text-ink-6">
            UI primitives + icon system
          </MonoValue>
        </header>

        <Section label="Buttons">
          <div className="flex flex-col gap-6">
            <Cell label="Primary">
              <Button size="sm" variant="primary">
                <Icon name="check" size={12} strokeWidth={2.4} />
                Finish sketch
              </Button>
              <Button size="md" variant="primary">
                <Icon name="plus" size={14} strokeWidth={2} />
                New project
              </Button>
              <Button size="md" variant="primary" disabled>
                Disabled
              </Button>
            </Cell>
            <Cell label="Secondary">
              <Button size="sm" variant="secondary">
                <Icon name="x" size={11} strokeWidth={2.2} />
                Cancel
              </Button>
              <Button size="md" variant="secondary">
                <Icon name="import" size={15} />
                Import STEP…
              </Button>
              <Button size="md" variant="secondary" disabled>
                Open…
              </Button>
            </Cell>
            <Cell label="Ghost">
              <Button size="sm" variant="ghost">
                Ghost sm
              </Button>
              <Button size="md" variant="ghost">
                <Icon name="home" size={15} strokeWidth={1.7} />
                Home
              </Button>
              <Button size="md" variant="ghost" disabled>
                Disabled
              </Button>
            </Cell>
          </div>
        </Section>

        <Section label="Segmented toggle">
          <div className="flex flex-col gap-6">
            <Cell label={`Model / Sketch (md) — selected: ${mode}`}>
              <SegmentedToggle
                ariaLabel="Editing mode"
                value={mode}
                onChange={setMode}
                options={[
                  { value: "model", label: "Model" },
                  { value: "sketch", label: "Sketch" },
                ]}
              />
            </Cell>
            <Cell label={`Persp / Ortho (sm) — selected: ${proj}`}>
              <SegmentedToggle
                size="sm"
                ariaLabel="Projection"
                value={proj}
                onChange={setProj}
                options={[
                  { value: "persp", label: "Persp" },
                  { value: "ortho", label: "Ortho" },
                ]}
              />
            </Cell>
          </div>
        </Section>

        <Section label="Switch">
          <div className="flex flex-col gap-6">
            <Cell label="On">
              <Switch
                checked={snapGrid}
                onChange={setSnapGrid}
                ariaLabel="Snap to grid"
              />
            </Cell>
            <Cell label="Off">
              <Switch
                checked={snapEdges}
                onChange={setSnapEdges}
                ariaLabel="Snap to distant edges"
              />
            </Cell>
            <Cell label="Disabled">
              <Switch checked={false} disabled onChange={() => {}} ariaLabel="Disabled off" />
              <Switch checked disabled onChange={() => {}} ariaLabel="Disabled on" />
            </Cell>
          </div>
        </Section>

        <Section label="Tooltip (hover the buttons)">
          <div className="flex flex-wrap items-center gap-4">
            <Tooltip label="Extrude (E)">
              <button
                type="button"
                className="flex h-[34px] w-[34px] cursor-pointer items-center justify-center rounded-sm border-none bg-transparent text-ink-4 hover:bg-hover-3"
              >
                <Icon name="extrude" size={18} />
              </button>
            </Tooltip>
            <Tooltip label="Revolve (R)">
              <button
                type="button"
                className="flex h-[34px] w-[34px] cursor-pointer items-center justify-center rounded-sm border-none bg-transparent text-ink-4 hover:bg-hover-3"
              >
                <Icon name="revolve" size={18} />
              </button>
            </Tooltip>
            <Tooltip label="Always-on example" open>
              <span className="rounded-sm bg-chip px-2 py-1 text-[12px] text-ink-3">
                Forced open
              </span>
            </Tooltip>
          </div>
        </Section>

        <Section label="Popover (Escape / click-outside to close)">
          <div className="flex items-center gap-4">
            <button
              ref={snapBtn}
              type="button"
              onClick={() => setSnapOpen((v) => !v)}
              className="flex h-[36px] w-[36px] cursor-pointer items-center justify-center rounded-md border border-border bg-white text-ink-4 shadow-ctrl hover:bg-hover"
              aria-label="Snap settings"
            >
              <Icon name="snap" size={17} strokeWidth={1.6} />
            </button>
            <span className="font-ui text-[12px] text-ink-5">
              Toggle the snap popover
            </span>
            <Popover
              open={snapOpen}
              onClose={() => setSnapOpen(false)}
              anchorRef={snapBtn}
              placement="bottom-start"
              caret
              className="py-2"
            >
              <div className="px-3.5 pb-1 pt-1">
                <SectionLabel>Snap to</SectionLabel>
              </div>
              <PopRow
                label="Grid"
                checked={showGrid}
                onChange={setShowGrid}
              />
              <PopRow
                label="Distant edges"
                checked={snapEdges}
                onChange={setSnapEdges}
              />
              <div className="mx-3.5 my-1.5 h-px bg-border-subtle" />
              <div className="px-3.5 pb-1 pt-0.5">
                <SectionLabel>Show</SectionLabel>
              </div>
              <PopRow
                label="Snapping hints"
                checked={showHints}
                onChange={setShowHints}
              />
            </Popover>
          </div>
        </Section>

        <Section label="Section label & mono value">
          <div className="flex flex-col gap-4">
            <SectionLabel>Bodies</SectionLabel>
            <div className="flex items-center gap-6">
              <div className="flex items-center gap-2">
                <span className="text-[12.5px] text-ink-2">Extrude</span>
                <MonoValue className="text-[11.5px]">83.3 mm</MonoValue>
              </div>
              <MonoValue className="text-[11.5px] whitespace-pre">
                X 273.00 Y 210.00 Z 0.00
              </MonoValue>
            </div>
          </div>
        </Section>

        <Section label="Text input">
          <div className="flex flex-wrap items-center gap-6">
            <Cell label="Plain (32px)">
              <TextInput placeholder="Project name" wrapperClassName="w-[220px]" />
            </Cell>
            <Cell label="With leading icon">
              <TextInput
                leadingIcon="search"
                placeholder="Search projects…"
                wrapperClassName="w-[220px]"
              />
            </Cell>
          </div>
        </Section>

        <Section label="Eye toggle">
          <div className="flex items-center gap-6">
            <div className="flex items-center gap-2">
              <span className="text-[13px] text-ink-2">Body 1</span>
              <EyeToggle on={eyeA} onChange={setEyeA} ariaLabel="Body 1 visibility" />
            </div>
            <div className="flex items-center gap-2">
              <span className="text-[13px] text-ink-2">Sketch 5</span>
              <EyeToggle on={eyeB} onChange={setEyeB} ariaLabel="Sketch 5 visibility" />
            </div>
          </div>
        </Section>

        <Section label={`Icons (${iconNames.length})`}>
          <div className="grid grid-cols-[repeat(auto-fill,minmax(96px,1fr))] gap-2">
            {iconNames.map((name) => (
              <div
                key={name}
                className="flex flex-col items-center gap-2 rounded-md border border-border-subtle bg-panel px-2 py-3"
              >
                <span className="text-ink-3">
                  <Icon name={name} size={20} strokeWidth={1.7} />
                </span>
                <span className="font-mono text-[10.5px] text-ink-5">
                  {name}
                </span>
              </div>
            ))}
          </div>
        </Section>
      </div>
    </div>
  );
}

/** One switch row inside the snap popover specimen. */
function PopRow({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex h-[32px] items-center gap-2 px-3.5">
      <span className="flex-1 text-[13px] text-ink-2">{label}</span>
      <Switch checked={checked} onChange={onChange} ariaLabel={label} />
    </div>
  );
}
