import { describe, it, expect } from "vitest";
import { PreviewThrottle } from "./previewThrottle";

describe("PreviewThrottle", () => {
  it("leading edge: the first request sends immediately", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    const send = t.request({ d: 1 }, 0);
    expect(send).toEqual({ epoch: 1, params: { d: 1 } });
    expect(t.inFlight).toBe(1);
  });

  it("keeps ≤1 in flight: further requests coalesce while one is outstanding", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    t.request({ d: 1 }, 0);
    expect(t.request({ d: 2 }, 10)).toBeNull();
    expect(t.request({ d: 3 }, 20)).toBeNull();
    expect(t.pending).toBe(true);
    expect(t.inFlight).toBe(1);
  });

  it("latest-params-wins trailing after a response, honoring the ≥80ms floor", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    t.request({ d: 1 }, 0);
    t.request({ d: 2 }, 10);
    t.request({ d: 3 }, 20); // newest pending
    expect(t.onResponse(1, 30)).toBe(true);
    expect(t.inFlight).toBeNull();
    // Before the trailing window opens (30 < 0 + 80): no send.
    expect(t.tick(30)).toBeNull();
    // At the floor: send the LATEST params as a new epoch.
    expect(t.tick(80)).toEqual({ epoch: 2, params: { d: 3 } });
    expect(t.inFlight).toBe(2);
  });

  it("discards a stale response (epoch ≠ in-flight) without freeing the slot", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    t.request({ d: 1 }, 0);
    t.request({ d: 2 }, 10); // pending, epoch still 1 in flight
    expect(t.onResponse(99, 20)).toBe(false); // wrong epoch
    expect(t.inFlight).toBe(1); // untouched
    expect(t.onResponse(1, 30)).toBe(true);
  });

  it("mints monotonically increasing epochs across sends", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 0 });
    const e1 = t.request({ d: 1 }, 0)!.epoch;
    t.onResponse(1, 1);
    const e2 = t.request({ d: 2 }, 2)!.epoch;
    t.onResponse(2, 3);
    const e3 = t.request({ d: 3 }, 4)!.epoch;
    expect([e1, e2, e3]).toEqual([1, 2, 3]);
  });

  it("flush forces the latest params out as a new epoch (commit)", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    t.request({ d: 5 }, 0); // epoch 1 in flight
    t.request({ d: 9 }, 10); // pending
    const f = t.flush(20);
    expect(f).toEqual({ epoch: 2, params: { d: 9 } });
    expect(t.inFlight).toBe(2);
    // The superseded epoch-1 drag response is now stale.
    expect(t.onResponse(1, 25)).toBe(false);
    expect(t.onResponse(2, 30)).toBe(true);
  });

  it("flush falls back to the last sent params when nothing is pending", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    t.request({ d: 7 }, 0);
    t.onResponse(1, 5);
    const f = t.flush(10);
    expect(f).toEqual({ epoch: 2, params: { d: 7 } });
  });

  it("flush before any request returns null", () => {
    const t = new PreviewThrottle<{ d: number }>({ trailingMs: 80 });
    expect(t.flush(0)).toBeNull();
  });
});
