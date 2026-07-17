import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { projectToScreen, HtmlOverlayDriver } from "./HtmlOverlayDriver";

function viewProjFor(camera: THREE.Camera): THREE.Matrix4 {
  camera.updateMatrixWorld();
  return new THREE.Matrix4().multiplyMatrices(
    camera.projectionMatrix,
    camera.matrixWorldInverse,
  );
}

describe("projectToScreen (pure)", () => {
  const cam = new THREE.PerspectiveCamera(60, 800 / 600, 0.1, 1000);
  cam.position.set(0, 0, 10);
  cam.up.set(0, 1, 0);
  cam.lookAt(0, 0, 0);
  cam.updateProjectionMatrix();
  const vp = viewProjFor(cam);

  it("projects the look-at point to screen center", () => {
    const p = projectToScreen({ x: 0, y: 0, z: 0 }, vp, 800, 600);
    expect(p.visible).toBe(true);
    expect(p.x).toBeCloseTo(400, 0);
    expect(p.y).toBeCloseTo(300, 0);
  });

  it("hides points behind the camera", () => {
    const p = projectToScreen({ x: 0, y: 0, z: 20 }, vp, 800, 600);
    expect(p.visible).toBe(false);
  });

  it("hides points outside the frustum", () => {
    const p = projectToScreen({ x: 1000, y: 0, z: 0 }, vp, 800, 600);
    expect(p.visible).toBe(false);
  });
});

describe("HtmlOverlayDriver", () => {
  it("writes a transform for a visible item and hides behind-camera ones", () => {
    const cam = new THREE.PerspectiveCamera(60, 1, 0.1, 100);
    cam.position.set(0, 0, 10);
    cam.up.set(0, 1, 0);
    cam.lookAt(0, 0, 0);
    cam.updateProjectionMatrix();
    cam.updateMatrixWorld();

    const driver = new HtmlOverlayDriver();
    const front = document.createElement("div");
    const back = document.createElement("div");
    driver.register("front", front, new THREE.Vector3(0, 0, 0));
    driver.register("back", back, new THREE.Vector3(0, 0, 30));
    expect(driver.size).toBe(2);

    driver.update(cam, 400, 400);
    expect(front.style.transform).toContain("translate");
    expect(front.style.display).toBe("");
    expect(back.style.display).toBe("none");

    driver.unregister("back");
    expect(driver.size).toBe(1);
  });
});
