import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { planeBasisMatrix, planePointToWorld, worldToPlanePoint, planeGeometry } from "./sketchBasis";
import { planeFor } from "@/ipc/mockSketch";

describe("sketchBasis — plane ↔ world mapping", () => {
  it("maps the NON-STANDARD XY basis (User X→World Y+, User Y→World X−)", () => {
    const xy = planeFor("XY");
    // plane (u,v) → world = u·(0,1,0) + v·(−1,0,0) = (−v, u, 0)
    const w = planePointToWorld(xy, { x: 3, y: 5 });
    expect(w.x).toBeCloseTo(-5);
    expect(w.y).toBeCloseTo(3);
    expect(w.z).toBeCloseTo(0);
  });

  it("round-trips world → plane → world on XY", () => {
    const xy = planeFor("XY");
    const p = worldToPlanePoint(xy, new THREE.Vector3(-5, 3, 0));
    expect(p.x).toBeCloseTo(3);
    expect(p.y).toBeCloseTo(5);
  });

  it("maps XZ and YZ planes to their exact bases", () => {
    // XZ: x=(0,1,0), y=(0,0,1) ⇒ (u,v) → (0, u, v)
    const w1 = planePointToWorld(planeFor("XZ"), { x: 2, y: 7 });
    expect([w1.x, w1.y, w1.z]).toEqual([0, 2, 7]);
    // YZ: x=(−1,0,0), y=(0,0,1) ⇒ (u,v) → (−u, 0, v)
    const w2 = planePointToWorld(planeFor("YZ"), { x: 4, y: 9 });
    expect([w2.x, w2.y, w2.z]).toEqual([-4, 0, 9]);
  });

  it("basis matrix transforms local (u,v,0) → world identically to planePointToWorld", () => {
    const xy = planeFor("XY");
    const m = planeBasisMatrix(xy);
    const local = new THREE.Vector3(3, 5, 0).applyMatrix4(m);
    const direct = planePointToWorld(xy, { x: 3, y: 5 });
    expect(local.distanceTo(direct)).toBeCloseTo(0);
  });

  it("respects a translated origin", () => {
    const xy = planeFor("XY");
    xy.origin = [10, 20, 30];
    const w = planePointToWorld(xy, { x: 0, y: 0 });
    expect([w.x, w.y, w.z]).toEqual([10, 20, 30]);
    const back = worldToPlanePoint(xy, new THREE.Vector3(10, 20, 30));
    expect(back.x).toBeCloseTo(0);
    expect(back.y).toBeCloseTo(0);
  });

  it("planeGeometry returns a THREE.Plane through the origin with the plane normal", () => {
    const plane = planeGeometry(planeFor("XY"));
    expect(plane.normal.z).toBeCloseTo(1);
    expect(plane.distanceToPoint(new THREE.Vector3(5, 5, 0))).toBeCloseTo(0);
    expect(plane.distanceToPoint(new THREE.Vector3(0, 0, 4))).toBeCloseTo(4);
  });
});
