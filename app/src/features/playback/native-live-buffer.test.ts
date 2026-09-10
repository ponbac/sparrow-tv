import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { watchNativeLiveBuffer } from "./native-live-buffer";

beforeEach(() =>
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "performance"] }),
);
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("native live buffer", () => {
  it("plays through an initial burst, then catches up once with a reserve", () => {
    const player = fixture();
    const release = watchNativeLiveBuffer(player.media, player.seek);
    player.advance(8_000, 5);
    expect(player.seeks).toEqual([]);
    player.advance(4_000, 1);
    expect(player.seeks).toHaveLength(1);
    expect(player.bufferAhead()).toBeCloseTo(5);
    player.advance(60_000, 1);
    expect(player.seeks).toHaveLength(1);
    release();
  });

  it("does not seek an ordinarily buffered live stream", () => {
    const player = fixture();
    const release = watchNativeLiveBuffer(player.media, player.seek);
    player.advance(60_000, 1);
    expect(player.seeks).toEqual([]);
    release();
  });

  it("prevents another catch-up during the cooldown after a delayed burst", () => {
    const player = fixture();
    const release = watchNativeLiveBuffer(player.media, player.seek);
    player.advance(8_000, 5);
    player.advance(4_000, 1);
    player.advance(4_000, 5);
    player.advance(20_000, 1);
    expect(player.seeks).toHaveLength(1);
    player.advance(12_000, 1);
    expect(player.seeks).toHaveLength(2);
    release();
  });

  it("waits for active playback and a fresh observation window", () => {
    const player = fixture();
    player.media.paused = true;
    const release = watchNativeLiveBuffer(player.media, player.seek);
    player.advance(10_000, 5);
    expect(player.seeks).toEqual([]);
    player.media.paused = false;
    player.advance(1_000, 1);
    expect(player.seeks).toEqual([]);
    player.advance(1_000, 1);
    expect(player.seeks).toHaveLength(1);
    release();
  });

  it("does not seek across a gap into a range smaller than the reserve", () => {
    const player = fixture();
    player.media.buffered.start = () => player.media.buffered.end() - 1;
    const release = watchNativeLiveBuffer(player.media, player.seek);
    player.advance(8_000, 5);
    player.advance(4_000, 1);
    expect(player.seeks).toEqual([]);
    release();
  });

  it("releases pending observations before transport replacement", () => {
    const player = fixture();
    const release = watchNativeLiveBuffer(player.media, player.seek);
    player.advance(8_000, 5);
    release();
    release();
    player.advance(60_000, 1);
    expect(player.seeks).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });
});

function fixture() {
  let end = 4;
  const seeks: number[] = [];
  const media = {
    currentTime: 0,
    paused: false,
    seeking: false,
    buffered: {
      length: 1,
      start: () => 0,
      end: () => end,
    },
  };
  return {
    media,
    seeks,
    seek: (seconds: number) => {
      seeks.push(seconds);
      media.currentTime = seconds;
    },
    bufferAhead: () => end - media.currentTime,
    advance: (milliseconds: number, incomingRate: number) => {
      for (let elapsed = 0; elapsed < milliseconds; elapsed += 250) {
        if (!media.paused) media.currentTime += 0.25;
        end += 0.25 * incomingRate;
        vi.advanceTimersByTime(250);
      }
    },
  };
}
