/*
 * ConstraintBadgeLayer — HTML overlay glyphs for the sketch's constraints
 * (H/V, coincident dot, dimensional value), placed at entity midpoints and
 * driven by the engine's HtmlOverlayDriver (per-frame world→screen transforms,
 * no React churn on camera move). Dimensional badges render an editable
 * DimensionInput chip. Shown only in sketch mode.
 *
 * The layer overlays EXACTLY the viewport area (top-0 → bottom-[34px]) so its
 * (0,0)…(w,h) matches the driver's projection space (canvas size).
 */
import { useEffect, useMemo, useRef } from "react";
import { useToolStore } from "@/stores/toolStore";
import { useSketchStore } from "@/stores/sketchStore";
import { useViewportEngine } from "@/viewport/engineBridge";
import { planePointToWorld } from "@/viewport/engine/sketchBasis";
import { createClient } from "@/ipc/client";
import { editConstraintValue } from "@/tools/sketch/sketchService";
import { layoutBadges } from "./badgeLayout";
import { DimensionInput } from "./DimensionInput";

export function ConstraintBadgeLayer() {
  const mode = useToolStore((s) => s.mode);
  const session = useSketchStore((s) => s.session);
  const engine = useViewportEngine();
  const clientRef = useRef<ReturnType<typeof createClient> | null>(null);
  if (!clientRef.current) {
    try {
      clientRef.current = createClient();
    } catch {
      clientRef.current = null;
    }
  }

  const badges = useMemo(() => layoutBadges(session), [session]);
  const plane = session?.plane ?? null;
  const refs = useRef(new Map<string, HTMLDivElement>());

  // Register each badge wrapper with the overlay driver (it owns positioning).
  useEffect(() => {
    if (!engine || !plane || mode !== "sketch") return;
    const overlay = engine.overlay;
    const ids: string[] = [];
    for (const b of badges) {
      const el = refs.current.get(b.id);
      if (!el) continue;
      overlay.register(b.id, el, planePointToWorld(plane, b.at));
      ids.push(b.id);
    }
    engine.invalidate();
    return () => {
      for (const id of ids) overlay.unregister(id);
    };
  }, [engine, plane, badges, mode]);

  if (mode !== "sketch" || !session) return null;

  return (
    <div
      data-testid="constraint-badges"
      className="pointer-events-none absolute inset-x-0 bottom-[34px] top-0 z-[3] overflow-hidden"
    >
      {badges.map((b) => (
        <div
          key={b.id}
          ref={(el) => {
            if (el) refs.current.set(b.id, el);
            else refs.current.delete(b.id);
          }}
        >
          {b.editable && b.value !== undefined ? (
            <span className="inline-block -translate-y-4 translate-x-2">
              <DimensionInput
                value={b.value}
                suffix={b.kind === "Angle" ? "" : "mm"}
                onCommit={(v) => {
                  if (clientRef.current) void editConstraintValue(clientRef.current, b.id, v);
                }}
              />
            </span>
          ) : (
            <span
              title={b.kind}
              className="inline-flex h-4 min-w-4 -translate-y-3.5 translate-x-2 items-center justify-center rounded-sm border border-border bg-white px-1 text-[10px] font-semibold leading-none text-accent shadow-ctrl"
            >
              {b.glyph}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}
