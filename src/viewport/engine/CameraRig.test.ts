import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { CameraRig, orthoHalfHeight } from "./CameraRig";

describe("orthoHalfHeight (pure)", () => {
  it("matches distance * tan(fov/2)", () => {
    expect(orthoHalfHeight(100, 90)).toBeCloseTo(100 * Math.tan(Math.PI / 4), 6);
    expect(orthoHalfHeight(0, 76)).toBe(0);
    // larger fov → larger half-height at fixed distance
    expect(orthoHalfHeight(100, 90)).toBeGreaterThan(orthoHalfHeight(100, 60));
  });
});

describe("CameraRig persp⇄ortho apparent size", () => {
  it("ortho frustum preserves apparent size at the pivot distance", () => {
    const rig = new CameraRig(76);
    rig.setAspect(2);
    const target = new THREE.Vector3(0, 0, 0);
    const distance = 300;
    const offset = new THREE.Vector3(0, -distance, 0);

    rig.setProjection("ortho");
    rig.apply(target, offset, distance);

    const halfH = orthoHalfHeight(distance, 76);
    expect(rig.ortho.top).toBeCloseTo(halfH, 4);
    expect(rig.ortho.bottom).toBeCloseTo(-halfH, 4);
    expect(rig.ortho.right).toBeCloseTo(halfH * 2, 4); // aspect = 2
    expect(rig.ortho.left).toBeCloseTo(-halfH * 2, 4);
  });

  it("keeps up = world Z on both cameras", () => {
    const rig = new CameraRig();
    expect(rig.persp.up.toArray()).toEqual([0, 0, 1]);
    expect(rig.ortho.up.toArray()).toEqual([0, 0, 1]);
  });

  it("places the active camera at target + offset", () => {
    const rig = new CameraRig();
    rig.setProjection("persp");
    rig.apply(new THREE.Vector3(1, 2, 3), new THREE.Vector3(0, 0, 10), 10);
    expect(rig.persp.position.toArray()).toEqual([1, 2, 13]);
  });
});
