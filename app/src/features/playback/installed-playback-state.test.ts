import { describe, expect, it } from "vitest";
import { clientSchemas } from "../../client/contracts";
import { installedPlaybackDiagnostics } from "./installed-playback-diagnostics";
import {
  createInstalledPlaybackState,
  reduceInstalledPlaybackState,
  type InstalledPlaybackEvent,
} from "./installed-playback-state";

const PRIVATE_CHANNEL = clientSchemas.channel.parse({
  id: "private-channel-canary",
  name: "Private Provider Canary",
  group: "Private",
});
const PRIVATE_AUDIO_ID = clientSchemas.audioTrackId.parse(
  `atrk1_${"e".repeat(32)}`,
);

describe("installed Playback Session state", () => {
  it("reduces every lifecycle phase and preserves controls across transport epochs", () => {
    let state = createInstalledPlaybackState();
    const events: readonly InstalledPlaybackEvent[] = [
      {
        _tag: "select",
        channel: PRIVATE_CHANNEL,
        sessionEpoch: 1,
        transportEpoch: 1,
      },
      {
        _tag: "transport-opened",
        tracks: [
          {
            id: PRIVATE_AUDIO_ID,
            language: "eng",
            label: "Private audio canary",
            codec: "aac-adts",
            selected: true,
          },
        ],
        selection: {
          _tag: "selected",
          trackId: PRIVATE_AUDIO_ID,
          reason: "saved-preference",
        },
      },
      { _tag: "volume", volume: 0.37 },
      { _tag: "muted", muted: true },
      { _tag: "fullscreen", fullscreen: true },
      { _tag: "playing", now: 100 },
      {
        _tag: "suspending",
        next: { _tag: "paused", cause: "user" },
        transportEpoch: 2,
      },
      { _tag: "paused", cause: "user", resumeWhenVisible: false },
      { _tag: "starting", reason: "resume", transportEpoch: 3 },
      { _tag: "autoplay-blocked" },
      {
        _tag: "suspending",
        next: { _tag: "recovering" },
        transportEpoch: 4,
      },
      {
        _tag: "recovering",
        attempt: 1,
        retryAt: 1_100,
        failure: "stream-interrupted",
      },
      { _tag: "starting", reason: "recovery", transportEpoch: 5 },
      { _tag: "playing", now: 1_200 },
      { _tag: "stable" },
      {
        _tag: "failed",
        failure: "source-invalid",
        attemptsUsed: 0,
        canRestart: true,
        canFailover: true,
      },
      { _tag: "fallback-starting" },
      { _tag: "fallback-playing" },
      {
        _tag: "fallback-stop-failed",
        failure: {
          _tag: "fallback-failed",
          reason: "control-unavailable",
          retryable: true,
        },
      },
      {
        _tag: "primary-stopped",
        fallbackFailure: null,
        canFailover: true,
      },
      {
        _tag: "stopping",
        nextChannel: null,
        sessionEpoch: 2,
        transportEpoch: 6,
      },
      { _tag: "stopped" },
    ];
    const phases: string[] = [];
    for (const event of events) {
      state = reduceInstalledPlaybackState(state, event);
      phases.push(state.phase._tag);
    }

    expect(phases).toEqual([
      "starting",
      "starting",
      "starting",
      "starting",
      "starting",
      "playing",
      "suspending",
      "paused",
      "starting",
      "autoplay-blocked",
      "suspending",
      "recovering",
      "starting",
      "playing",
      "playing",
      "failed",
      "fallback-starting",
      "fallback-playing",
      "fallback-stop-failed",
      "primary-stopped",
      "stopping",
      "idle",
    ]);
    expect(state.controls).toEqual({
      volume: 0.37,
      muted: true,
      fullscreen: false,
    });
    expect(state.audio.discovered).toBe(false);
  });

  it("clamps invalid control input and emits diagnostics from an allowlist only", () => {
    let state = reduceInstalledPlaybackState(createInstalledPlaybackState(), {
      _tag: "select",
      channel: PRIVATE_CHANNEL,
      sessionEpoch: 987_654,
      transportEpoch: 456_789,
    });
    state = reduceInstalledPlaybackState(state, {
      _tag: "transport-opened",
      tracks: [
        {
          id: PRIVATE_AUDIO_ID,
          label: "Private audio metadata canary",
          codec: "aac-adts",
          selected: true,
        },
      ],
      selection: {
        _tag: "fallback",
        trackId: PRIVATE_AUDIO_ID,
        missing: "saved-preference",
      },
      preferenceStatus: "not-saved",
    });
    state = reduceInstalledPlaybackState(state, {
      _tag: "volume",
      volume: Number.POSITIVE_INFINITY,
    });
    state = reduceInstalledPlaybackState(state, {
      _tag: "failed",
      failure: "source-unavailable",
      attemptsUsed: 3,
      canRestart: true,
      canFailover: true,
    });

    const diagnostics = installedPlaybackDiagnostics(
      state,
      Array.from({ length: 30 }, () => ({
        from: "starting" as const,
        to: "recovering" as const,
      })),
      999_999,
    );
    const decoded = JSON.parse(diagnostics) as {
      readonly transitions: readonly unknown[];
    };

    expect(state.controls.volume).toBe(0);
    expect(decoded.transitions).toHaveLength(20);
    for (const canary of [
      PRIVATE_CHANNEL.id,
      PRIVATE_CHANNEL.name,
      "play1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_1",
      "stream1_bbbbbbbbbbbbbbbb",
      "https://user:secret@provider.invalid/live",
      "authorization",
      "fingerprint-canary",
      "payload-canary",
      PRIVATE_AUDIO_ID,
      "Private audio metadata canary",
    ]) {
      expect(diagnostics).not.toContain(canary);
    }
    expect(diagnostics).toContain('"failure":"source-unavailable"');
    expect(diagnostics).toContain('"selection":"saved-preference-fallback"');
    expect(diagnostics).toContain('"preferenceStatus":"not-saved"');
  });
});
