import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  watchNativeLiveBuffer,
  type NativeLiveBufferMedia,
} from "./native-live-buffer";

beforeEach(() =>
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "performance"] }),
);
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("native live buffer", () => {
  it("does not interrupt while a live timeline grows", () => {
    const player = fixture();
    const watch = watchNativeLiveBuffer(player.media);
    player.advance(8_000);
    player.advance(60_000);
    expect(watch.snapshot().standstills).toBe(0);
    expect(watch.snapshot().waiting).toBe(0);
    watch.stop();
  });

  it("records a standstill after media time stops with a frame present", () => {
    const player = fixture();
    const watch = watchNativeLiveBuffer(player.media);
    player.advance(1_000);
    expect(watch.snapshot().standstills).toBe(0);
    player.freeze(2_000);
    expect(watch.snapshot().standstills).toBe(1);
    expect(watch.snapshot().msSinceTimeAdvance).toBeGreaterThanOrEqual(2_000);
    player.freeze(2_000);
    expect(watch.snapshot().standstills).toBe(1);
    watch.stop();
  });

  it("does not treat startup without a frame as a standstill", () => {
    const player = fixture();
    player.media.readyState = 1;
    const watch = watchNativeLiveBuffer(player.media);
    player.freeze(5_000);
    expect(watch.snapshot().standstills).toBe(0);
    expect(watch.snapshot().readyState).toBe(1);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(5_000);
    watch.stop();
  });

  it.each([0, 1])("keeps progress age and counts a stall when readyState falls to %s", (readyState) => {
    const player = fixture();
    const watch = watchNativeLiveBuffer(player.media);
    player.advance(1_000);
    player.media.readyState = readyState;
    player.emit("waiting");
    player.freeze(5_000);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(5_000);
    expect(watch.snapshot().standstills).toBe(1);
    player.freeze(1_000);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(6_000);
    player.media.readyState = 2;
    player.freeze(250);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(6_250);
    player.advance(250);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(0);
    watch.stop();
  });

  it.each(["paused", "seeking"] as const)("does not count %s timeline movement as fresh playback", (field) => {
    const player = fixture();
    const watch = watchNativeLiveBuffer(player.media);
    player.advance(1_000);
    player.media[field] = true;
    player.media.currentTime += 20;
    player.freeze(3_000);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(3_000);
    player.media[field] = false;
    player.freeze(250);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(3_250);
    player.advance(250);
    expect(watch.snapshot().msSinceTimeAdvance).toBe(0);
    watch.stop();
  });

  it("keeps NVIDIA-visible frame callbacks independent of unsupported quality counters", () => {
    const player = fixture();
    const pending = new Map<number, (now: number) => void>();
    let next = 0;
    player.media.requestVideoFrameCallback = (callback) => {
      const id = ++next;
      pending.set(id, callback);
      return id;
    };
    player.media.cancelVideoFrameCallback = (id) => { pending.delete(id); };
    const watch = watchNativeLiveBuffer(player.media);
    for (let index = 0; index < 3; index += 1) {
      const callback = pending.get(next);
      pending.delete(next);
      callback?.(performance.now());
    }
    expect(watch.snapshot().presentedFrames).toBe(3);
    expect(watch.snapshot().totalVideoFrames).toBe(0);
    const lateCallback = pending.get(next);
    watch.stop();
    lateCallback?.(performance.now());
    expect(pending.size).toBe(0);
    expect(watch.snapshot().presentedFrames).toBe(3);
  });

  it("counts waiting events without changing playback", () => {
    const player = fixture();
    const watch = watchNativeLiveBuffer(player.media);
    player.emit("waiting");
    player.emit("waiting");
    player.emit("stalled");
    expect(watch.snapshot().waiting).toBe(2);
    expect(watch.snapshot().stalledEvents).toBe(1);
    watch.stop();
  });

  it("releases pending observations before transport replacement", () => {
    const player = fixture();
    const watch = watchNativeLiveBuffer(player.media);
    player.advance(1_000);
    watch.stop();
    watch.stop();
    player.freeze(5_000);
    expect(watch.snapshot().standstills).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});

function fixture() {
  const listeners = new Map<string, Set<() => void>>();
  let end = 4;
  const media: NativeLiveBufferMedia & { readyState: number } = {
    currentTime: 0,
    paused: false,
    seeking: false,
    readyState: 2,
    videoWidth: 1920,
    videoHeight: 1080,
    buffered: {
      length: 1,
      start: () => 0,
      end: () => end,
    },
    addEventListener: (type, listener) => {
      const set = listeners.get(type) ?? new Set();
      set.add(listener);
      listeners.set(type, set);
    },
    removeEventListener: (type, listener) => {
      listeners.get(type)?.delete(listener);
    },
  };
  return {
    media,
    emit: (type: string) => {
      for (const listener of listeners.get(type) ?? []) listener();
    },
    advance: (milliseconds: number) => {
      for (let elapsed = 0; elapsed < milliseconds; elapsed += 250) {
        if (!media.paused) media.currentTime += 0.25;
        end += 0.25;
        vi.advanceTimersByTime(250);
      }
    },
    freeze: (milliseconds: number) => {
      vi.advanceTimersByTime(milliseconds);
    },
  };
}
