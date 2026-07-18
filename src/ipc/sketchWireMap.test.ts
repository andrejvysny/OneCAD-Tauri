/*
 * sketchWireMap — the PURE frontend → Rust-typed sketch marshaller (F-WP9).
 * Deterministic ids (`uuid-N`) make the AddEntity/AddConstraint output assertable.
 */
import { beforeEach, describe, expect, it } from "vitest";
import {
  buildAddSketch,
  createIdMap,
  frontendEntitiesFromDto,
  isDimensional,
  marshalUpsert,
} from "./sketchWireMap";
import type { SketchConstraint, SketchEntity } from "./types";

let seq = 0;
const mint = () => `uuid-${++seq}`;
beforeEach(() => {
  seq = 0;
});

const line = (id: string, p0: [number, number], p1: [number, number]): SketchEntity => ({ id, type: "Line", p0, p1 });
const circle = (id: string, center: [number, number], radius: number): SketchEntity => ({ id, type: "Circle", center, radius });

describe("marshalUpsert — entities", () => {
  it("marshals a Line into two synthesized Points + a point-referenced Line", () => {
    const map = createIdMap("sk-uuid", "XY");
    const ops = marshalUpsert(map, { entities: [line("e1", [0, 0], [40, 0])], constraints: [] }, mint);
    expect(ops).toHaveLength(3);
    expect(ops[0]).toEqual({ op: "addEntity", entity: { kind: "point", id: "uuid-1", at: [0, 0] } });
    expect(ops[1]).toEqual({ op: "addEntity", entity: { kind: "point", id: "uuid-2", at: [40, 0] } });
    expect(ops[2]).toEqual({ op: "addEntity", entity: { kind: "line", id: "uuid-3", start: "uuid-1", end: "uuid-2" } });
    expect(map.entity.get("e1")).toBe("uuid-3");
    expect(map.point.get("e1.Start")).toBe("uuid-1");
    expect(map.point.get("e1.End")).toBe("uuid-2");
  });

  it("marshals a Circle into a center Point + a circle", () => {
    const map = createIdMap("sk", "XY");
    const ops = marshalUpsert(map, { entities: [circle("e1", [10, 10], 3)], constraints: [] }, mint);
    expect(ops[0]).toEqual({ op: "addEntity", entity: { kind: "point", id: "uuid-1", at: [10, 10] } });
    expect(ops[1]).toEqual({ op: "addEntity", entity: { kind: "circle", id: "uuid-2", center: "uuid-1", radius: 3 } });
  });

  it("emits only NEW entities on a second upsert (diff by id)", () => {
    const map = createIdMap("sk", "XY");
    marshalUpsert(map, { entities: [line("e1", [0, 0], [40, 0])], constraints: [] }, mint);
    const ops = marshalUpsert(map, { entities: [line("e1", [0, 0], [40, 0]), circle("e2", [5, 5], 2)], constraints: [] }, mint);
    expect(ops).toHaveLength(2); // e1 already mapped; only e2's point + circle
    expect(ops.every((o) => o.op === "addEntity")).toBe(true);
  });

  it("emits RemoveEntity when a mapped entity leaves the array", () => {
    const map = createIdMap("sk", "XY");
    marshalUpsert(map, { entities: [line("e1", [0, 0], [40, 0])], constraints: [] }, mint);
    const lineId = map.entity.get("e1")!;
    const ops = marshalUpsert(map, { entities: [], constraints: [] }, mint);
    expect(ops).toEqual([{ op: "removeEntity", entity: lineId }]);
    expect(map.entity.has("e1")).toBe(false);
    expect(map.point.has("e1.Start")).toBe(false);
  });
});

describe("marshalUpsert — constraints", () => {
  it("Horizontal references the line entity id", () => {
    const map = createIdMap("sk", "XY");
    const ops = marshalUpsert(
      map,
      { entities: [line("e1", [0, 0], [40, 0])], constraints: [{ id: "c1", type: "Horizontal", entities: ["e1"] }] },
      mint,
    );
    const c = ops.find((o) => o.op === "addConstraint");
    expect(c).toEqual({ op: "addConstraint", constraint: { kind: "horizontal", id: "uuid-4", line: "uuid-3" } });
  });

  it("Coincident resolves endpoint POINT ids via the positions selectors", () => {
    const map = createIdMap("sk", "XY");
    const constraints: SketchConstraint[] = [
      { id: "c1", type: "Coincident", entities: ["e1", "e2"], positions: ["End", "Start"] },
    ];
    const ops = marshalUpsert(
      map,
      { entities: [line("e1", [0, 0], [40, 0]), line("e2", [40, 0], [40, 40])], constraints },
      mint,
    );
    // e1: p uuid-1(Start), uuid-2(End), line uuid-3; e2: p uuid-4(Start), uuid-5(End), line uuid-6.
    const c = ops.find((o) => o.op === "addConstraint");
    expect(c).toEqual({ op: "addConstraint", constraint: { kind: "coincident", id: "uuid-7", point1: "uuid-2", point2: "uuid-4" } });
  });

  it("emits SetDimension when a dimensional value changes in place", () => {
    const map = createIdMap("sk", "XY");
    const radius = (v: number): SketchConstraint => ({ id: "c1", type: "Radius", entities: ["e1"], value: v });
    marshalUpsert(map, { entities: [circle("e1", [0, 0], 5)], constraints: [radius(5)] }, mint);
    const cId = map.constraint.get("c1")!;
    const ops = marshalUpsert(map, { entities: [circle("e1", [0, 0], 5)], constraints: [radius(8)] }, mint);
    expect(ops).toEqual([{ op: "setDimension", constraint: cId, value: { value: 8 } }]);
  });

  it("skips an unmappable constraint (arc-endpoint coincidence) without throwing", () => {
    const map = createIdMap("sk", "XY");
    const arc: SketchEntity = { id: "e1", type: "Arc", center: [0, 0], radius: 5, start: [5, 0], end: [0, 5] };
    // Coincident on the arc START has no Rust point id (arc references only center).
    const ops = marshalUpsert(
      map,
      { entities: [arc], constraints: [{ id: "c1", type: "Coincident", entities: ["e1", "e1"], positions: ["Start", "End"] }] },
      mint,
    );
    expect(ops.some((o) => o.op === "addConstraint")).toBe(false);
    expect(map.constraint.has("c1")).toBe(false);
  });
});

describe("buildAddSketch / frontendEntitiesFromDto / isDimensional", () => {
  it("builds a minimal world-plane AddSketch (custom → XY)", () => {
    expect(buildAddSketch("id-1", "Sketch 1", "XZ")).toEqual({
      cmd: "addSketch",
      sketch: { id: "id-1", name: "Sketch 1", attachment: { kind: "world", plane: "XZ" } },
    });
    expect(buildAddSketch("id-1", "S", "custom").sketch.attachment.plane).toBe("XY");
  });

  it("reverse-maps worker-wire entities to the frontend inlined form; [] for empty", () => {
    expect(frontendEntitiesFromDto([])).toEqual([]);
    const wire = [
      { id: "p1", type: "Point", at: [0, 0] },
      { id: "p2", type: "Point", at: [40, 0] },
      { id: "l1", type: "Line", p0Ref: "p1", p1Ref: "p2" },
      { id: "c1", type: "Circle", center: [10, 10], radius: 3 },
    ];
    const fe = frontendEntitiesFromDto(wire);
    expect(fe.find((e) => e.id === "l1")).toMatchObject({ type: "Line", p0: [0, 0], p1: [40, 0] });
    expect(fe.find((e) => e.id === "c1")).toMatchObject({ type: "Circle", center: [10, 10], radius: 3 });
  });

  it("classifies dimensional constraint types", () => {
    expect(isDimensional("Radius")).toBe(true);
    expect(isDimensional("Distance")).toBe(true);
    expect(isDimensional("Horizontal")).toBe(false);
    expect(isDimensional("Coincident")).toBe(false);
  });
});
