import { useRef, useState } from "react";
import { Icon } from "@/icons/Icon";
import { Popover } from "@/ui/Popover";

export type SortKey = "date" | "name";

const LABEL: Record<SortKey, string> = {
  date: "Date modified",
  name: "Name",
};

type SortMenuProps = {
  value: SortKey;
  onChange: (key: SortKey) => void;
};

/**
 * Recent-projects sort dropdown (prototype 1a, lines 63-70): a hairline chevron
 * button that opens a 150px menu of "Date modified" / "Name". The menu is
 * right-aligned (bottom-end) so it stays inside the card's right edge.
 */
export function SortMenu({ value, onChange }: SortMenuProps) {
  const [open, setOpen] = useState(false);
  const btn = useRef<HTMLButtonElement | null>(null);

  const pick = (key: SortKey) => {
    onChange(key);
    setOpen(false);
  };

  return (
    <div className="relative">
      <button
        ref={btn}
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex h-[32px] cursor-pointer items-center gap-1.5 rounded-sm border border-border-strong bg-white px-2.5 font-ui text-[12.5px] text-ink-2 hover:bg-hover"
      >
        {LABEL[value]}
        <Icon name="chevronDown" size={12} strokeWidth={2} className="text-ink-5" />
      </button>

      <Popover
        open={open}
        onClose={() => setOpen(false)}
        anchorRef={btn}
        placement="bottom-end"
        width={150}
        className="p-1"
      >
        <MenuItem label={LABEL.date} onClick={() => pick("date")} />
        <MenuItem label={LABEL.name} onClick={() => pick("name")} />
      </Popover>
    </div>
  );
}

function MenuItem({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <div
      role="menuitem"
      onClick={onClick}
      className="flex h-[30px] cursor-pointer items-center rounded-[5px] px-2.5 text-[12.5px] text-ink-2 hover:bg-well"
    >
      {label}
    </div>
  );
}
