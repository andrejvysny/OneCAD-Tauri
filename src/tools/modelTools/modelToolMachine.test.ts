import { describe, it, expect } from "vitest";
import {
  extrudeInit,
  extrudeStep,
  filletInit,
  filletStep,
  booleanInit,
  booleanStep,
  revolveInit,
  revolveStep,
  shellInit,
  shellStep,
  linearPatternInit,
  linearPatternStep,
  circularPatternInit,
  circularPatternStep,
  mirrorInit,
  mirrorStep,
  DEFAULT_EXTRUDE_DEPTH,
  DEFAULT_REVOLVE_ANGLE,
  DEFAULT_SHELL_THICKNESS,
  type ExtrudeFsm,
  type FilletFsm,
  type BooleanFsm,
  type RevolveFsm,
  type ShellFsm,
  type LinearPatternFsm,
  type CircularPatternFsm,
  type MirrorFsm,
} from "./modelToolMachine";

describe("extrude FSM", () => {
  it("runs the full pointer script arm → grab → drag → release → settle", () => {
    let s = extrudeInit();
    expect(s.phase).toBe("idle");

    let step = extrudeStep(s, { kind: "arm" });
    expect(step.effect).toBe("begin");
    expect(step.state.phase).toBe("armed");
    expect(step.state.depth).toBe(DEFAULT_EXTRUDE_DEPTH);
    s = step.state;

    s = extrudeStep(s, { kind: "grab" }).state;
    expect(s.phase).toBe("dragging");

    step = extrudeStep(s, { kind: "drag", depth: 25, symmetric: true });
    expect(step.effect).toBe("update");
    expect(step.state.depth).toBe(25);
    expect(step.state.symmetric).toBe(true);
    s = step.state;

    step = extrudeStep(s, { kind: "release" });
    expect(step.effect).toBe("commit");
    expect(step.state.phase).toBe("committing");
    s = step.state;

    s = extrudeStep(s, { kind: "settle" }).state;
    expect(s.phase).toBe("idle");
  });

  it("drag is ignored unless dragging; setDepth works while armed", () => {
    const armed: ExtrudeFsm = extrudeStep(extrudeInit(), { kind: "arm" }).state;
    expect(extrudeStep(armed, { kind: "drag", depth: 9 }).effect).toBe("none");
    const set = extrudeStep(armed, { kind: "setDepth", depth: 12 });
    expect(set.effect).toBe("update");
    expect(set.state.depth).toBe(12);
  });

  it("cancel from any active phase resets + emits cancel", () => {
    const dragging = extrudeStep(extrudeStep(extrudeInit(), { kind: "arm" }).state, { kind: "grab" }).state;
    const step = extrudeStep(dragging, { kind: "cancel" });
    expect(step.effect).toBe("cancel");
    expect(step.state.phase).toBe("idle");
    // Cancel while idle is a no-op.
    expect(extrudeStep(extrudeInit(), { kind: "cancel" }).effect).toBe("none");
  });
});

describe("fillet FSM", () => {
  it("arms only with ≥1 edge, then drags radius to commit", () => {
    expect(filletStep(filletInit(), { kind: "arm", edgeCount: 0 }).state.phase).toBe("idle");

    let s: FilletFsm = filletStep(filletInit(), { kind: "arm", edgeCount: 2, radius: 3 }).state;
    expect(s.phase).toBe("armed");
    expect(s.edgeCount).toBe(2);
    expect(s.radius).toBe(3);

    s = filletStep(s, { kind: "grabEdge" }).state;
    expect(s.phase).toBe("dragging");

    const dragged = filletStep(s, { kind: "drag", radius: 4.5 });
    expect(dragged.effect).toBe("update");
    expect(dragged.state.radius).toBe(4.5);

    const committed = filletStep(dragged.state, { kind: "release" });
    expect(committed.effect).toBe("commit");
    expect(committed.state.phase).toBe("committing");
  });
});

describe("revolve FSM", () => {
  it("runs arm → axisPick → pickAxis → grab → drag → release → settle", () => {
    let s: RevolveFsm = revolveInit();
    expect(s.phase).toBe("idle");
    expect(s.angle).toBe(DEFAULT_REVOLVE_ANGLE);

    let step = revolveStep(s, { kind: "arm" });
    expect(step.effect).toBe("none"); // no axis yet → axis-pick, no preview
    expect(step.state.phase).toBe("axisPick");
    s = step.state;

    step = revolveStep(s, { kind: "pickAxis", lineId: "L1", valid: true });
    expect(step.effect).toBe("begin"); // axis chosen → L1 lathe begins
    expect(step.state.phase).toBe("armed");
    expect(step.state.axisLineId).toBe("L1");
    s = step.state;

    s = revolveStep(s, { kind: "grab" }).state;
    expect(s.phase).toBe("dragging");

    step = revolveStep(s, { kind: "drag", angle: 90 });
    expect(step.effect).toBe("update");
    expect(step.state.angle).toBe(90);
    s = step.state;

    step = revolveStep(s, { kind: "release" });
    expect(step.effect).toBe("commit");
    expect(step.state.phase).toBe("committing");
    s = step.state;

    expect(revolveStep(s, { kind: "settle" }).state.phase).toBe("idle");
  });

  it("rejects an invalid axis and stays in axis-pick", () => {
    const axisPick = revolveStep(revolveInit(), { kind: "arm" }).state;
    const step = revolveStep(axisPick, { kind: "pickAxis", lineId: "L9", valid: false });
    expect(step.effect).toBe("none");
    expect(step.state.phase).toBe("axisPick");
    expect(step.state.axisLineId).toBeNull();
  });

  it("plain click after axis-pick commits the default 360° (quickCommit)", () => {
    const armed = revolveStep(
      revolveStep(revolveInit(), { kind: "arm" }).state,
      { kind: "pickAxis", lineId: "L1", valid: true },
    ).state;
    expect(armed.angle).toBe(360);
    const step = revolveStep(armed, { kind: "quickCommit" });
    expect(step.effect).toBe("commit");
    expect(step.state.phase).toBe("committing");
    expect(step.state.angle).toBe(360);
    // quickCommit is only legal from armed (not from axis-pick).
    const axisPick = revolveStep(revolveInit(), { kind: "arm" }).state;
    expect(revolveStep(axisPick, { kind: "quickCommit" }).effect).toBe("none");
  });

  it("re-edit arms straight into armed with the seeded angle (skips axis-pick)", () => {
    const step = revolveStep(revolveInit(), { kind: "arm", angle: 120, hasAxis: true, axisLineId: "L2" });
    expect(step.effect).toBe("begin");
    expect(step.state.phase).toBe("armed");
    expect(step.state.angle).toBe(120);
    expect(step.state.axisLineId).toBe("L2");
  });

  it("resetAxis returns to axis-pick, clearing the axis; setAngle works while armed", () => {
    const armed = revolveStep(
      revolveStep(revolveInit(), { kind: "arm" }).state,
      { kind: "pickAxis", lineId: "L1", valid: true },
    ).state;
    const set = revolveStep(armed, { kind: "setAngle", angle: 45 });
    expect(set.effect).toBe("update");
    expect(set.state.angle).toBe(45);

    const reset = revolveStep(set.state, { kind: "resetAxis" });
    expect(reset.state.phase).toBe("axisPick");
    expect(reset.state.axisLineId).toBeNull();
    // drag/quickCommit are ignored back in axis-pick.
    expect(revolveStep(reset.state, { kind: "drag", angle: 10 }).effect).toBe("none");
  });

  it("cancel from any active phase resets; idle cancel is a no-op", () => {
    const armed = revolveStep(
      revolveStep(revolveInit(), { kind: "arm" }).state,
      { kind: "pickAxis", lineId: "L1", valid: true },
    ).state;
    const step = revolveStep(armed, { kind: "cancel" });
    expect(step.effect).toBe("cancel");
    expect(step.state.phase).toBe("idle");
    expect(revolveStep(revolveInit(), { kind: "cancel" }).effect).toBe("none");
  });
});

describe("boolean FSM", () => {
  it("runs start → pickTool → setOp → apply → settle", () => {
    let s: BooleanFsm = booleanStep(booleanInit(), { kind: "start", targetBodyId: "body1" }).state;
    expect(s.phase).toBe("pickTool");
    expect(s.targetBodyId).toBe("body1");

    const picked = booleanStep(s, { kind: "pickTool", toolBodyId: "body2" });
    expect(picked.effect).toBe("ghost");
    expect(picked.state.phase).toBe("armed");
    s = picked.state;

    s = booleanStep(s, { kind: "setOp", op: "Cut" }).state;
    expect(s.op).toBe("Cut");

    const applied = booleanStep(s, { kind: "apply" });
    expect(applied.effect).toBe("commit");
    expect(applied.state.phase).toBe("committing");

    expect(booleanStep(applied.state, { kind: "settle" }).state.phase).toBe("idle");
  });

  it("ignores picking the target body as the tool body", () => {
    const s = booleanStep(booleanInit(), { kind: "start", targetBodyId: "body1" }).state;
    const step = booleanStep(s, { kind: "pickTool", toolBodyId: "body1" });
    expect(step.effect).toBe("none");
    expect(step.state.phase).toBe("pickTool");
  });
});

describe("shell FSM", () => {
  it("arms only with ≥1 face, then drags thickness to commit", () => {
    expect(shellStep(shellInit(), { kind: "arm", faceCount: 0 }).state.phase).toBe("idle");

    let s: ShellFsm = shellStep(shellInit(), { kind: "arm", faceCount: 2, thickness: 3 }).state;
    expect(s.phase).toBe("armed");
    expect(s.faceCount).toBe(2);
    expect(s.thickness).toBe(3);

    s = shellStep(s, { kind: "grab" }).state;
    expect(s.phase).toBe("dragging");

    const dragged = shellStep(s, { kind: "drag", thickness: 4.5 });
    expect(dragged.effect).toBe("update");
    expect(dragged.state.thickness).toBe(4.5);

    const committed = shellStep(dragged.state, { kind: "release" });
    expect(committed.effect).toBe("commit");
    expect(committed.state.phase).toBe("committing");

    expect(shellStep(committed.state, { kind: "settle" }).state.phase).toBe("idle");
  });

  it("defaults the thickness + honors setThickness while armed", () => {
    const armed = shellStep(shellInit(), { kind: "arm", faceCount: 1 }).state;
    expect(armed.thickness).toBe(DEFAULT_SHELL_THICKNESS);
    // drag is ignored unless dragging.
    expect(shellStep(armed, { kind: "drag", thickness: 9 }).effect).toBe("none");
    const set = shellStep(armed, { kind: "setThickness", thickness: 5 });
    expect(set.effect).toBe("update");
    expect(set.state.thickness).toBe(5);
  });

  it("cancel from an active phase resets; idle cancel is a no-op", () => {
    const dragging = shellStep(shellStep(shellInit(), { kind: "arm", faceCount: 1 }).state, { kind: "grab" }).state;
    expect(shellStep(dragging, { kind: "cancel" }).effect).toBe("cancel");
    expect(shellStep(dragging, { kind: "cancel" }).state.phase).toBe("idle");
    expect(shellStep(shellInit(), { kind: "cancel" }).effect).toBe("none");
  });
});

describe("linear pattern FSM", () => {
  it("arms only with a body, defaults axis X, then configures + applies", () => {
    expect(linearPatternStep(linearPatternInit(), { kind: "arm" }).state.phase).toBe("idle");

    let s: LinearPatternFsm = linearPatternStep(linearPatternInit(), { kind: "arm", bodyId: "body1" }).state;
    const armed = linearPatternStep(linearPatternInit(), { kind: "arm", bodyId: "body1" });
    expect(armed.effect).toBe("ghost");
    expect(s.phase).toBe("armed");
    expect(s.axis).toBe("X");
    expect(s.bodyId).toBe("body1");

    s = linearPatternStep(s, { kind: "setAxis", axis: "Y" }).state;
    expect(s.axis).toBe("Y");
    s = linearPatternStep(s, { kind: "setSpacing", spacing: 30 }).state;
    expect(s.spacing).toBe(30);

    const apply = linearPatternStep(s, { kind: "apply" });
    expect(apply.effect).toBe("commit");
    expect(apply.state.phase).toBe("committing");
  });

  it("clamps count to [2, 12] (integer)", () => {
    const s = linearPatternStep(linearPatternInit(), { kind: "arm", bodyId: "b" }).state;
    expect(linearPatternStep(s, { kind: "setCount", count: 1 }).state.count).toBe(2);
    expect(linearPatternStep(s, { kind: "setCount", count: 99 }).state.count).toBe(12);
    expect(linearPatternStep(s, { kind: "setCount", count: 4.7 }).state.count).toBe(5);
  });

  it("cancel resets; idle cancel is a no-op", () => {
    const s = linearPatternStep(linearPatternInit(), { kind: "arm", bodyId: "b" }).state;
    expect(linearPatternStep(s, { kind: "cancel" }).effect).toBe("cancel");
    expect(linearPatternStep(s, { kind: "cancel" }).state.phase).toBe("idle");
    expect(linearPatternStep(linearPatternInit(), { kind: "cancel" }).effect).toBe("none");
  });
});

describe("circular pattern FSM", () => {
  it("arms with a body, defaults axis Z + 360°, configures + applies", () => {
    let s: CircularPatternFsm = circularPatternStep(circularPatternInit(), { kind: "arm", bodyId: "body1" }).state;
    expect(s.phase).toBe("armed");
    expect(s.axis).toBe("Z");
    expect(s.angle).toBe(DEFAULT_REVOLVE_ANGLE); // 360

    s = circularPatternStep(s, { kind: "setAngle", angle: 180 }).state;
    expect(s.angle).toBe(180);
    // angle clamps to [1, 360].
    expect(circularPatternStep(s, { kind: "setAngle", angle: 999 }).state.angle).toBe(360);
    expect(circularPatternStep(s, { kind: "setAngle", angle: 0 }).state.angle).toBe(1);

    const apply = circularPatternStep(s, { kind: "apply" });
    expect(apply.effect).toBe("commit");
    expect(apply.state.phase).toBe("committing");
  });

  it("arm without a body stays idle", () => {
    expect(circularPatternStep(circularPatternInit(), { kind: "arm" }).state.phase).toBe("idle");
  });
});

describe("mirror FSM", () => {
  it("arms with a body, defaults plane XY, sets plane + applies", () => {
    let s: MirrorFsm = mirrorInit();
    expect(mirrorStep(s, { kind: "arm" }).state.phase).toBe("idle");

    s = mirrorStep(mirrorInit(), { kind: "arm", bodyId: "body1" }).state;
    expect(s.phase).toBe("armed");
    expect(s.plane).toBe("XY");

    const setPlane = mirrorStep(s, { kind: "setPlane", plane: "YZ" });
    expect(setPlane.effect).toBe("ghost");
    expect(setPlane.state.plane).toBe("YZ");

    const apply = mirrorStep(setPlane.state, { kind: "apply" });
    expect(apply.effect).toBe("commit");
    expect(apply.state.phase).toBe("committing");
    expect(mirrorStep(apply.state, { kind: "settle" }).state.phase).toBe("idle");
  });

  it("cancel resets; idle cancel is a no-op", () => {
    const s = mirrorStep(mirrorInit(), { kind: "arm", bodyId: "b" }).state;
    expect(mirrorStep(s, { kind: "cancel" }).effect).toBe("cancel");
    expect(mirrorStep(mirrorInit(), { kind: "cancel" }).effect).toBe("none");
  });
});
