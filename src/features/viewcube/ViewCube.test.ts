import { describe, it, expect } from "vitest";
import * as THREE from "three";
import {
  cubeContainerMatrix,
  faceMatrix,
  cssMatrix3d,
  viewLabelForDirection,
  FACES,
} from "./ViewCube";

describe("cubeContainerMatrix", () => {
  it("identity camera → Y-flip (diag 1,-1,1)", () => {
    const m = cubeContainerMatrix({ x: 0, y: 0, z: 0, w: 1 });
    expect(m).toHaveLength(16);
    expect(m.slice(0, 4)).toEqual([1, 0, 0, 0]);
    expect(m.slice(4, 8)).toEqual([0, -1, 0, 0]);
    expect(m.slice(8, 12)).toEqual([0, 0, 1, 0]);
    expect(m.slice(12, 16)).toEqual([0, 0, 0, 1]);
  });

  it("is the inverse of the camera rotation (Y-flipped)", () => {
    // A 90° yaw about world Z.
    const q = new THREE.Quaternion().setFromAxisAngle(
      new THREE.Vector3(0, 0, 1),
      Math.PI / 2,
    );
    const m = new THREE.Matrix4().fromArray(
      cubeContainerMatrix({ x: q.x, y: q.y, z: q.z, w: q.w }),
    );
    // Undo the Y-flip on both sides → should be inverse(q) as a rotation.
    const flip = new THREE.Matrix4().makeScale(1, -1, 1);
    const rot = m.clone().premultiply(flip);
    const expected = new THREE.Matrix4().makeRotationFromQuaternion(q.clone().invert());
    rot.elements.forEach((v, i) => expect(v).toBeCloseTo(expected.elements[i], 6));
  });
});

describe("faceMatrix", () => {
  it("places FRONT with normal -Y at translation -Y*half", () => {
    const m = faceMatrix(new THREE.Vector3(0, -1, 0), new THREE.Vector3(0, 0, 1), 31);
    // Column 2 (elements 8..10) is the outward normal.
    expect(m.slice(8, 11).map((v) => Math.round(v))).toEqual([0, -1, 0]);
    // Translation (elements 12..14) = normal * half.
    expect(m.slice(12, 15).map((v) => Math.round(v))).toEqual([0, -31, 0]);
  });

  it("produces an orthonormal basis for every face", () => {
    for (const f of FACES) {
      const m = new THREE.Matrix4().fromArray(faceMatrix(f.normal, f.up, 31));
      const c0 = new THREE.Vector3().setFromMatrixColumn(m, 0);
      const c1 = new THREE.Vector3().setFromMatrixColumn(m, 1);
      const c2 = new THREE.Vector3().setFromMatrixColumn(m, 2);
      expect(c0.length()).toBeCloseTo(1, 6);
      expect(c0.dot(c1)).toBeCloseTo(0, 6);
      expect(c1.dot(c2)).toBeCloseTo(0, 6);
      expect(c2.dot(c0)).toBeCloseTo(0, 6);
    }
  });
});

describe("cssMatrix3d", () => {
  it("formats a matrix3d() string", () => {
    expect(cssMatrix3d([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])).toBe(
      "matrix3d(1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1)",
    );
  });
});

describe("viewLabelForDirection", () => {
  it("names canonical views", () => {
    expect(viewLabelForDirection({ x: 0, y: 0, z: 1 })).toBe("TOP");
    expect(viewLabelForDirection({ x: 0, y: 0, z: -1 })).toBe("BOTTOM");
    expect(viewLabelForDirection({ x: 0, y: -1, z: 0 })).toBe("FRONT");
    expect(viewLabelForDirection({ x: 1, y: 0, z: 0 })).toBe("RIGHT");
  });
  it("labels the iso view", () => {
    expect(viewLabelForDirection({ x: 1, y: -1, z: 1 })).toBe("ISO");
  });
  it("falls back to — for arbitrary views", () => {
    expect(viewLabelForDirection({ x: 0.2, y: -0.3, z: 0.93 })).toBe("—");
    expect(viewLabelForDirection({ x: 0, y: 0, z: 0 })).toBe("—");
  });
});
