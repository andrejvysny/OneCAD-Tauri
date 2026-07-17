/*
 * previewThrottle — the pacing gate for Level-2 exact previews (plan "Frontend
 * specifics" / NEW_SPEC §15). PURE + clock-injected so it unit-tests with fake
 * timers and explicit `now` values.
 *
 * Contract (from the plan):
 *   - LEADING EDGE: the first request of a burst sends immediately.
 *   - ≤1 IN FLIGHT: never more than one exact request outstanding at a time.
 *   - LATEST-PARAMS-WINS trailing: while one is in flight, further params
 *     coalesce; only the newest is sent next, and no sooner than `trailingMs`
 *     (≥80ms) after the previous send.
 *   - EPOCH COUNTER: every send mints a monotonically increasing epoch.
 *   - STALE-RESPONSE DISCARD: a response whose epoch is not the in-flight one is
 *     discarded (a superseded drag solve), leaving the in-flight slot untouched.
 *
 * The owner drives it: `request` on each pointer frame, `tick` on a timer to
 * release a trailing send, `onResponse` when an L2 result arrives, `flush` on
 * commit to force the final params out as a fresh epoch.
 */
export interface ThrottleSend<P> {
  epoch: number;
  params: P;
}

export const DEFAULT_TRAILING_MS = 80;

export class PreviewThrottle<P> {
  private readonly trailingMs: number;
  private epochCounter = 0;
  private inFlightEpoch: number | null = null;
  private lastSentAt = Number.NEGATIVE_INFINITY;
  private pendingParams: P | null = null;
  private lastParams: P | null = null;

  constructor(opts: { trailingMs?: number } = {}) {
    this.trailingMs = Math.max(0, opts.trailingMs ?? DEFAULT_TRAILING_MS);
  }

  /** Latest epoch minted (the newest params committed to the wire). */
  get epoch(): number {
    return this.epochCounter;
  }

  /** Epoch currently outstanding, or null when idle. */
  get inFlight(): number | null {
    return this.inFlightEpoch;
  }

  /** True when trailing params are waiting for a send window. */
  get pending(): boolean {
    return this.pendingParams !== null;
  }

  /** Register the latest drag params. Returns a send to dispatch now, or null. */
  request(params: P, now: number): ThrottleSend<P> | null {
    this.pendingParams = params;
    return this.maybeSend(now);
  }

  /** Timer poke: release a coalesced trailing send once its window opens. */
  tick(now: number): ThrottleSend<P> | null {
    return this.maybeSend(now);
  }

  /**
   * An L2 response for `epoch` arrived. Returns true when it is the in-flight
   * (fresh) one — the caller should then `tick` to pump any trailing send. A
   * stale epoch is discarded (returns false) and the in-flight slot is untouched.
   */
  onResponse(epoch: number, _now: number): boolean {
    if (epoch !== this.inFlightEpoch) return false; // stale — discard
    this.inFlightEpoch = null;
    return true;
  }

  /**
   * Commit flush: force the latest params out as a NEW epoch regardless of the
   * trailing window or a still-outstanding drag solve (its response is discarded
   * as stale). Returns the send, or null when nothing was ever requested.
   */
  flush(now: number): ThrottleSend<P> | null {
    const params = this.pendingParams ?? this.lastParams;
    if (params === null) return null;
    return this.dispatch(params, now);
  }

  /** Forget all state (drag ended / tool changed). */
  reset(): void {
    this.epochCounter = 0;
    this.inFlightEpoch = null;
    this.lastSentAt = Number.NEGATIVE_INFINITY;
    this.pendingParams = null;
    this.lastParams = null;
  }

  private maybeSend(now: number): ThrottleSend<P> | null {
    if (this.inFlightEpoch !== null) return null; // ≤1 in flight
    if (this.pendingParams === null) return null;
    if (now - this.lastSentAt < this.trailingMs) return null; // trailing floor
    return this.dispatch(this.pendingParams, now);
  }

  private dispatch(params: P, now: number): ThrottleSend<P> {
    this.epochCounter += 1;
    this.inFlightEpoch = this.epochCounter;
    this.lastSentAt = now;
    this.pendingParams = null;
    this.lastParams = params;
    return { epoch: this.epochCounter, params };
  }
}
