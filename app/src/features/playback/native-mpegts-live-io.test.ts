import { afterEach, describe, expect, it, vi } from "vitest";
import { adoptLiveMpegtsIo } from "./native-mpegts-live-io";

afterEach(() => {
  vi.useRealTimers();
});

describe("live mpegts.js IO resume", () => {
  it("ignores players that are not mpegts.js MSE instances", () => {
    expect(() => adoptLiveMpegtsIo({})).not.toThrow();
  });
});

describe("live MSE duration", () => {
  it("sets MediaSource duration to Infinity once the source is open", () => {
    const mediaSource = liveMediaSource("closed");
    const player = pacedPlayer({ mediaSource });

    const adopted = adoptLiveMpegtsIo(player, bufferedMedia(0, 0.5));
    expect(mediaSource.duration).toBeNaN();

    mediaSource.readyState = "open";
    mediaSource.emit("sourceopen");

    expect(mediaSource.duration).toBe(Number.POSITIVE_INFINITY);
    adopted.stop();
  });

  it("keeps MediaSource duration unbounded through later appends", () => {
    vi.useFakeTimers();
    const mediaSource = liveMediaSource("open");
    const player = pacedPlayer({ mediaSource });
    const adopted = adoptLiveMpegtsIo(player, bufferedMedia(0, 0.5));
    expect(mediaSource.duration).toBe(Number.POSITIVE_INFINITY);
    mediaSource.duration = 0.5;
    player._msectl.appendMediaSegment({});
    expect(mediaSource.duration).toBe(Number.POSITIVE_INFINITY);
    mediaSource.duration = 1;
    vi.advanceTimersByTime(250);
    expect(mediaSource.duration).toBe(Number.POSITIVE_INFINITY);
    adopted.stop();
    adopted.stop();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not pause transmuxing when the forward buffer is long", () => {
    const pause = vi.fn();
    const resume = vi.fn();
    const player = pacedPlayer({ pause, resume });
    const adopted = adoptLiveMpegtsIo(player, bufferedMedia(0, 3));

    player._msectl.appendMediaSegment({});

    expect(pause).not.toHaveBeenCalled();
    expect(resume).not.toHaveBeenCalled();
    adopted.stop();
  });
});

function liveMediaSource(readyState: "open" | "closed") {
  const listeners = new Map<string, () => void>();
  return {
    readyState,
    duration: Number.NaN,
    addEventListener(type: string, listener: () => void) {
      listeners.set(type, listener);
    },
    removeEventListener(type: string) {
      listeners.delete(type);
    },
    emit(type: string) {
      listeners.get(type)?.();
    },
  };
}

function pacedPlayer(options: {
  readonly mediaSource?: ReturnType<typeof liveMediaSource>;
  readonly pause?: ReturnType<typeof vi.fn>;
  readonly resume?: ReturnType<typeof vi.fn>;
}) {
  return {
    _transmuxer: {
      pause: options.pause ?? vi.fn(),
      resume: options.resume ?? vi.fn(),
    },
    _msectl: {
      appendMediaSegment: vi.fn(),
      _mediaSource: options.mediaSource ?? liveMediaSource("open"),
    },
  };
}

function bufferedMedia(currentTime: number, end: number) {
  return {
    currentTime,
    buffered: {
      length: 1,
      start: () => 0,
      end: () => end,
    },
  };
}
