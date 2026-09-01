import { describe, expect, it, vi } from "vitest";
import { ALTERNATE_AUDIO_SELECTION_EXPRESSION } from "./android-playback-acceptance.ts";
import {
  combineModernFrameStats,
  evaluatePlaybackGates,
  parseModernFrameStats,
  parsePlaybackMarker,
  summarizePlaybackSamples,
  type PlaybackActions,
  type PlaybackMarker,
} from "./android-playback-acceptance-domain.ts";

const marker = (overrides: Partial<PlaybackMarker> = {}): PlaybackMarker => ({
  engine: "android-media3",
  state: "playing",
  droppedFrames: 4,
  bufferedDurationMs: 12_000,
  decodedFrames: null,
  processSilent: true,
  muted: true,
  volume: 0,
  routineUrlCount: 0,
  ...overrides,
});

const passedActions: PlaybackActions = {
  initialPlayback: true,
  pauseReleasedPresentation: true,
  resumeReturnedToPlaying: true,
  backgroundReleasedPresentation: true,
  foregroundReturnedToPlaying: true,
  alternateAudioAvailable: true,
  alternateAudioSelected: true,
  audioSelectionReleasedPresentation: true,
  audioSelectionReturnedToPlaying: true,
  channelSwitched: true,
  channelSwitchReturnedToPlaying: true,
  stopRemovedPresentation: true,
  stopRemovedPlayer: true,
};

describe("Android playback acceptance boundaries", () => {
  it("accepts a dispatched controlled-select change before the async value commits", () => {
    const runtime = globalThis as unknown as {
      readonly document: {
        readonly body: { innerHTML: string };
        querySelector(selector: string): unknown;
      };
      readonly eval: (source: string) => unknown;
    };
    runtime.document.body.innerHTML = `
      <select aria-label="Audio track">
        <option value="first" selected>First</option>
        <option value="second">Second</option>
      </select>
    `;
    const select = runtime.document.querySelector('select[aria-label="Audio track"]') as
      | {
          selectedIndex: number;
          addEventListener(type: string, listener: () => void): void;
        }
      | null;
    if (select === null) {
      throw new Error("audio fixture did not mount");
    }
    const change = vi.fn(() => {
      select.selectedIndex = 0;
    });
    select.addEventListener("change", change);

    expect(runtime.eval(ALTERNATE_AUDIO_SELECTION_EXPRESSION)).toEqual({
      available: true,
      selected: true,
    });
    expect(change).toHaveBeenCalledTimes(1);
  });

  it("accepts only strict aggregate native status and rejects private additions", () => {
    expect(parsePlaybackMarker(marker())).toEqual({ ok: true, value: marker() });
    expect(
      parsePlaybackMarker({
        ...marker(),
        sourceUrl: "https://credential-canary.invalid/private.ts",
      }),
    ).toEqual({
      ok: false,
      reason: "the native playback marker was unavailable or contained non-aggregate fields",
    });
    expect(JSON.stringify(parsePlaybackMarker(marker()))).not.toContain("credential-canary");
  });

  it("summarizes dropped, decoded, buffer, silence, and privacy counters", () => {
    expect(
      summarizePlaybackSamples(
        [
          marker({ droppedFrames: 2, decodedFrames: 1_000 }),
          marker({ droppedFrames: 4, decodedFrames: 2_000, bufferedDurationMs: 8_000 }),
        ],
        120_000,
      ),
    ).toEqual({
      ok: true,
      value: {
        durationMs: 120_000,
        sampleCount: 2,
        allPlaying: true,
        allSilent: true,
        routineUrlCount: 0,
        droppedFrames: 2,
        decodedFrames: 1_000,
        droppedFramePercent: 0.2,
        minimumBufferedDurationMs: 8_000,
        maximumBufferedDurationMs: 12_000,
        zeroBufferSamples: 0,
        countersMonotonic: true,
      },
    });

    const unsilenced = summarizePlaybackSamples(
      [marker(), marker({ processSilent: false })],
      120_000,
    );
    expect(unsilenced.ok && unsilenced.value.allSilent).toBe(false);
  });

  it("uses the modern frame deadline line and ignores the legacy percentage", () => {
    expect(
      parseModernFrameStats(`
        Total frames rendered: 12355
        Janky frames: 52 (0.42%)
        Janky frames (legacy): 9211 (74.55%)
      `),
    ).toEqual({
      ok: true,
      value: { totalFrames: 12_355, jankyFrames: 52, jankyPercent: 0.42 },
    });
  });

  it("combines disjoint frame journeys from their counters, not their percentages", () => {
    expect(
      combineModernFrameStats([
        { totalFrames: 80, jankyFrames: 8, jankyPercent: 10 },
        { totalFrames: 920, jankyFrames: 2, jankyPercent: 0.22 },
      ]),
    ).toEqual({
      ok: true,
      value: { totalFrames: 1_000, jankyFrames: 10, jankyPercent: 1 },
    });
  });

  it("fails closed on continuity, action, silence, and modern deadline regressions", () => {
    const sustained = summarizePlaybackSamples(
      [marker({ droppedFrames: 0 }), marker({ droppedFrames: 3 })],
      120_000,
    );
    expect(sustained.ok).toBe(true);
    if (!sustained.ok) return;

    expect(
      evaluatePlaybackGates({
        sustained: sustained.value,
        actions: passedActions,
        warmReplacementUiFrames: {
          totalFrames: 1_000,
          jankyFrames: 4,
          jankyPercent: 0.4,
        },
      }),
    ).toEqual([]);

    expect(
      evaluatePlaybackGates({
        sustained: { ...sustained.value, decodedFrames: 1_000 },
        actions: passedActions,
        warmReplacementUiFrames: {
          totalFrames: 1_000,
          jankyFrames: 4,
          jankyPercent: 0.4,
        },
      }),
    ).toEqual(["decoded-frame-rate"]);

    expect(
      evaluatePlaybackGates({
        sustained: {
          ...sustained.value,
          allSilent: false,
          zeroBufferSamples: 1,
          droppedFrames: 13,
        },
        actions: { ...passedActions, channelSwitched: false },
        warmReplacementUiFrames: {
          totalFrames: 1_000,
          jankyFrames: 30,
          jankyPercent: 3,
        },
      }),
    ).toEqual([
      "per-process-silence",
      "buffer-starvation",
      "dropped-frames",
      "channelSwitched",
      "modern-frame-deadlines",
    ]);
  });
});
