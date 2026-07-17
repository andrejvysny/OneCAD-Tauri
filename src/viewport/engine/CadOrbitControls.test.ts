import { describe, it, expect } from "vitest";
import * as THREE from "three";
import {
  clampPitch,
  sphericalToOffset,
  zoomToCursor,
  shortestAngleLerp,
  easeInOutCubic,
} from "./CadOrbitControls";

describe("clampPitch", () => {
  it("clamps to just inside ±90°", () => {
    expect(clampPitch(Math.PI)).toBeCloseTo(Math.PI / 2 - 1e-3, 6);
    expect(clampPitch(-Math.PI)).toBeCloseTo(-(Math.PI / 2 - 1e-3), 6);
    expect(clampPitch(0.3)).toBe(0.3);
    expect(clampPitch(Math.PI / 2)).toBeLessThan(Math.PI / 2);
  });
});

describe("sphericalToOffset (turntable, yaw about Z)", () => {
  it("yaw=0 pitch=0 points along +X", () => {
    const o = sphericalToOffset(0, 0, 5);
    expect(o.x).toBeCloseTo(5, 6);
    expect(o.y).toBeCloseTo(0, 6);
    expect(o.z).toBeCloseTo(0, 6);
  });
  it("pitch=+90° points along +Z (top)", () => {
    const o = sphericalToOffset(0, Math.PI / 2, 5);
    expect(o.z).toBeCloseTo(5, 6);
    expect(Math.hypot(o.x, o.y)).toBeCloseTo(0, 6);
  });
  it("radius is preserved", () => {
    const o = sphericalToOffset(1.1, 0.4, 7);
    expect(o.length()).toBeCloseTo(7, 6);
  });
});

describe("zoomToCursor", () => {
  const cam = new THREE.Vector3(0, 0, 10);
  const target = new THREE.Vector3(0, 0, 0);

  it("factor 1 is a no-op", () => {
    const r = zoomToCursor(cam, target, new THREE.Vector3(3, 4, 0), 1);
    expect(r.camPos.toArray()).toEqual([0, 0, 10]);
    expect(r.target.toArray()).toEqual([0, 0, 0]);
  });

  it("zooming toward the target just halves the eye distance", () => {
    const r = zoomToCursor(cam, target, target, 0.5);
    expect(r.target.toArray()).toEqual([0, 0, 0]);
    expect(r.camPos.z).toBeCloseTo(5, 6);
  });

  it("zooming toward an off-axis cursor pulls the target toward it", () => {
    const cursor = new THREE.Vector3(10, 0, 0);
    const r = zoomToCursor(cam, target, cursor, 0.5);
    expect(r.target.x).toBeCloseTo(5, 6); // halfway to cursor
  });
});

describe("shortestAngleLerp", () => {
  it("takes the short way around the circle", () => {
    expect(shortestAngleLerp(0, (3 * Math.PI) / 2, 1)).toBeCloseTo(-Math.PI / 2, 6);
    expect(shortestAngleLerp(0, Math.PI / 2, 0.5)).toBeCloseTo(Math.PI / 4, 6);
  });
});

describe("easeInOutCubic", () => {
  it("has fixed endpoints and midpoint", () => {
    expect(easeInOutCubic(0)).toBe(0);
    expect(easeInOutCubic(1)).toBe(1);
    expect(easeInOutCubic(0.5)).toBeCloseTo(0.5, 6);
  });
});
