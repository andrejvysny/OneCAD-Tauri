/*
 * Lathe (surface-of-revolution) preview geometry — PURE math, plane-local coords.
 * The revolve analogue of prismPreview: instead of lifting a profile along the
 * plane normal, it SWEEPS the profile boundary ring around an in-plane axis line.
 *
 * A revolve axis is a sketch LINE lying in the sketch plane. Rotating a planar
 * profile point around that in-plane axis carries it OUT of the plane (into the
 * normal direction w). For a profile point P and the unit axis direction D (both
 * in the z=0 plane), with foot F = projection of P onto the axis and perpendicular
 * r = P − F (also in-plane), Rodrigues' rotation by θ around D reduces to:
 *
 *   P(θ) = F + r·cosθ + (D × r)·sinθ ,   D × r = (0, 0, Dx·rv − Dy·ru)
 *
 * so the swept point is (Fu + ru·cosθ, Fv + rv·cosθ, mz·sinθ) with mz = Dx·rv −
 * Dy·ru. At θ=0 it is P (z=0); points ON the axis (r=0) stay fixed.
 *
 * Positions are authored in PLANE-LOCAL coords (x=u, y=v, z=w along the normal);
 * a scene group carrying the plane-basis matrix maps them to world (see
 * RevolvePreview / makeRevolveBodyMesh), exactly as prismPreview + PreviewMesh do.
 */

export type Vec2 = [number, number];

export interface LatheAxis {
  /** Axis endpoint 0 in plane (u, v). */
  a: Vec2;
  /** Axis endpoint 1 in plane (u, v). */
  b: Vec2;
}

export interface LatheLocal {
  /** Flat (x=u, y=v, z=w) plane-local vertex triples. */
  positions: number[];
  /** Triangle index triples into `positions`. */
  indices: number[];
  /** Angular segment count (there are `segments + 1` profile rings). */
  segments: number;
  /** Profile ring vertex count. */
  ringCount: number;
}

/** Coarse angular segment count for a sweep of `angleDeg` (~15°/segment, ≥2). */
export function latheSegmentsFor(angleDeg: number): number {
  const a = Math.max(0, Math.min(360, Math.abs(angleDeg)));
  return Math.max(2, Math.ceil(a / 15));
}

/**
 * Whether the (infinite) axis line through a→b passes through the profile
 * interior: true when the ring vertices STRADDLE the line (some strictly on each
 * side). A valid revolve axis keeps the whole profile on one side (touching the
 * line is allowed), so `false` ⇒ the axis is a legal axis of revolution.
 */
export function axisSplitsRegion(a: Vec2, b: Vec2, ring: Vec2[], eps = 1e-6): boolean {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  let pos = false;
  let neg = false;
  for (const [x, y] of ring) {
    const cross = dx * (y - a[1]) - dy * (x - a[0]);
    if (cross > eps) pos = true;
    else if (cross < -eps) neg = true;
    if (pos && neg) return true;
  }
  return false;
}

/**
 * Sweep a profile ring (plane u,v) around the in-plane `axis` line by `angleDeg`,
 * returning a surface-of-revolution mesh in plane-local coords. For a partial
 * sweep (<360°) the two end rings are capped (fan around each ring centroid) so
 * the coarse translucent L1 preview reads as a solid; a full sweep closes on
 * itself (the last ring coincides with the first) and needs no caps.
 */
export function latheLocal(
  ring: Vec2[],
  axis: LatheAxis,
  angleDeg: number,
  segments = latheSegmentsFor(angleDeg),
): LatheLocal {
  const ringN = ring.length;
  const ax = axis.a[0];
  const ay = axis.a[1];
  let dx = axis.b[0] - ax;
  let dy = axis.b[1] - ay;
  const dl = Math.hypot(dx, dy) || 1;
  dx /= dl;
  dy /= dl;

  const angleRad = (Math.max(0, Math.min(360, angleDeg)) * Math.PI) / 180;
  const full = angleRad >= Math.PI * 2 - 1e-6;
  const steps = Math.max(1, segments);

  // Per ring point: foot on the axis, in-plane perpendicular r, and mz = (D×r)_z.
  const footU: number[] = [];
  const footV: number[] = [];
  const perpU: number[] = [];
  const perpV: number[] = [];
  const mz: number[] = [];
  for (const [pu, pv] of ring) {
    const wx = pu - ax;
    const wy = pv - ay;
    const t = wx * dx + wy * dy;
    const fu = ax + t * dx;
    const fv = ay + t * dy;
    const ru = pu - fu;
    const rv = pv - fv;
    footU.push(fu);
    footV.push(fv);
    perpU.push(ru);
    perpV.push(rv);
    mz.push(dx * rv - dy * ru);
  }

  const positions: number[] = [];
  for (let j = 0; j <= steps; j++) {
    const theta = angleRad * (j / steps);
    const c = Math.cos(theta);
    const sn = Math.sin(theta);
    for (let i = 0; i < ringN; i++) {
      positions.push(footU[i] + perpU[i] * c, footV[i] + perpV[i] * c, mz[i] * sn);
    }
  }

  const indices: number[] = [];
  for (let j = 0; j < steps; j++) {
    const j0 = j * ringN;
    const j1 = (j + 1) * ringN;
    for (let i = 0; i < ringN; i++) {
      const iN = (i + 1) % ringN;
      indices.push(j0 + i, j0 + iN, j1 + iN);
      indices.push(j0 + i, j1 + iN, j1 + i);
    }
  }

  if (!full) {
    const cap = (base: number, reversed: boolean): void => {
      let cu = 0;
      let cv = 0;
      let cw = 0;
      for (let i = 0; i < ringN; i++) {
        cu += positions[(base + i) * 3];
        cv += positions[(base + i) * 3 + 1];
        cw += positions[(base + i) * 3 + 2];
      }
      const c = positions.length / 3;
      positions.push(cu / ringN, cv / ringN, cw / ringN);
      for (let i = 0; i < ringN; i++) {
        const iN = (i + 1) % ringN;
        if (reversed) indices.push(c, base + iN, base + i);
        else indices.push(c, base + i, base + iN);
      }
    };
    cap(0, true); // θ=0 cap, wound to face outward
    cap(steps * ringN, false); // θ=angle cap
  }

  return { positions, indices, segments: steps, ringCount: ringN };
}
