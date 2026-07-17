/*
 * Adaptive ground grid on the world XY plane (Z=0).
 *
 * v1: a finite grid re-centered on the camera target. Minor + major lines are
 * separate LineSegments; the minor lines fade toward the clear color at the
 * grid edge (a cheap distance fade with no transparency sorting). The step is
 * chosen adaptively from the camera distance on a 1/5/10 decade progression.
 */
import * as THREE from "three";

export interface GridStep {
  /** Minor line spacing in world units (mm). */
  minor: number;
  /** Major line spacing in world units (mm) = minor * 10. */
  major: number;
}

const DECADE = [1, 5, 10] as const;

/** Snap a positive value up to the nearest 1/5/10 decade step. */
export function snapToDecade(value: number): number {
  const v = Math.max(value, 1e-6);
  const pow = Math.pow(10, Math.floor(Math.log10(v)));
  const n = v / pow; // 1..<10
  const m = n < 2 ? DECADE[0] : n < 5 ? DECADE[1] : DECADE[2];
  return m * pow;
}

/**
 * Pure adaptive step selection. Targets ~one minor line per `distance/50` world
 * units, snapped to a 1/5/10 step; major lines are every 10 minors.
 */
export function chooseGridStep(cameraDistance: number): GridStep {
  const minor = snapToDecade(Math.max(cameraDistance, 1) / 50);
  return { minor, major: minor * 10 };
}

/** Half-extent of the finite grid, in minor cells, from center. */
const HALF_CELLS = 100;

interface GridColors {
  minor: THREE.Color;
  major: THREE.Color;
  clear: THREE.Color;
}

export class GridPlane {
  readonly object3D: THREE.Group;
  private readonly colors: GridColors;
  private minorSeg: THREE.LineSegments | null = null;
  private majorSeg: THREE.LineSegments | null = null;
  private currentStep = 0;

  constructor(colors: GridColors) {
    this.colors = colors;
    this.object3D = new THREE.Group();
    this.object3D.name = "gridPlane";
    // Grid lives on Z=0; render behind geometry.
    this.object3D.renderOrder = -1;
  }

  setVisible(visible: boolean): void {
    this.object3D.visible = visible;
  }

  /** Re-center on the target and re-step from the camera distance. */
  update(target: THREE.Vector3, cameraDistance: number): void {
    const { minor, major } = chooseGridStep(cameraDistance);
    if (minor !== this.currentStep) {
      this.rebuild(minor);
      this.currentStep = minor;
    }
    // Snap the grid origin to the nearest major line so lines stay put visually.
    const snap = major;
    this.object3D.position.set(
      Math.round(target.x / snap) * snap,
      Math.round(target.y / snap) * snap,
      0,
    );
  }

  private rebuild(minor: number): void {
    this.disposeSegments();
    const extent = HALF_CELLS * minor;

    const minorGeo = new THREE.BufferGeometry();
    const majorGeo = new THREE.BufferGeometry();
    const minorPos: number[] = [];
    const minorCol: number[] = [];
    const majorPos: number[] = [];

    const edge = new THREE.Color();
    for (let i = -HALF_CELLS; i <= HALF_CELLS; i++) {
      const c = i * minor;
      const isMajor = i % 10 === 0;
      // Fade minor lines toward the clear color near the grid edge.
      const fade = 1 - Math.abs(i) / HALF_CELLS;
      edge.copy(this.colors.clear).lerp(this.colors.minor, fade * fade);

      if (isMajor) {
        majorPos.push(-extent, c, 0, extent, c, 0);
        majorPos.push(c, -extent, 0, c, extent, 0);
      } else {
        minorPos.push(-extent, c, 0, extent, c, 0);
        minorPos.push(c, -extent, 0, c, extent, 0);
        for (let k = 0; k < 4; k++) minorCol.push(edge.r, edge.g, edge.b);
      }
    }

    minorGeo.setAttribute("position", new THREE.Float32BufferAttribute(minorPos, 3));
    minorGeo.setAttribute("color", new THREE.Float32BufferAttribute(minorCol, 3));
    majorGeo.setAttribute("position", new THREE.Float32BufferAttribute(majorPos, 3));

    this.minorSeg = new THREE.LineSegments(
      minorGeo,
      new THREE.LineBasicMaterial({ vertexColors: true }),
    );
    this.majorSeg = new THREE.LineSegments(
      majorGeo,
      new THREE.LineBasicMaterial({ color: this.colors.major }),
    );
    this.minorSeg.renderOrder = -1;
    this.majorSeg.renderOrder = -1;
    this.object3D.add(this.minorSeg, this.majorSeg);
  }

  private disposeSegments(): void {
    for (const seg of [this.minorSeg, this.majorSeg]) {
      if (!seg) continue;
      this.object3D.remove(seg);
      seg.geometry.dispose();
      (seg.material as THREE.Material).dispose();
    }
    this.minorSeg = null;
    this.majorSeg = null;
  }

  dispose(): void {
    this.disposeSegments();
  }
}
