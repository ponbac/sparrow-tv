import { z } from "zod";

export const REQUIRED_SUSTAINED_MS = 120_000;
export const MAX_DROPPED_FRAMES = 12;
export const MIN_DECODED_FRAMES_PER_SECOND = 20;
export const MAX_MODERN_JANK_PERCENT = 2;
export const MIN_MODERN_UI_FRAMES = 100;

type ParseResult<Value> =
  | { readonly ok: true; readonly value: Value }
  | { readonly ok: false; readonly reason: string };

export interface PlaybackMarker {
  readonly engine: "android-media3";
  readonly state: "starting" | "playing" | "paused" | "failed" | "stopped";
  readonly droppedFrames: number;
  readonly bufferedDurationMs: number;
  readonly decodedFrames: number | null;
  readonly processSilent: boolean;
  readonly muted: boolean;
  readonly volume: number;
  readonly routineUrlCount: number;
}

export interface SustainedPlaybackSummary {
  readonly durationMs: number;
  readonly sampleCount: number;
  readonly allPlaying: boolean;
  readonly allSilent: boolean;
  readonly routineUrlCount: number;
  readonly droppedFrames: number;
  readonly decodedFrames: number | null;
  readonly droppedFramePercent: number | null;
  readonly minimumBufferedDurationMs: number;
  readonly maximumBufferedDurationMs: number;
  readonly zeroBufferSamples: number;
  readonly countersMonotonic: boolean;
}

export interface ModernFrameStats {
  readonly totalFrames: number;
  readonly jankyFrames: number;
  readonly jankyPercent: number;
}

export interface PlaybackUiFrameJourneys {
  readonly startup: ModernFrameStats;
  readonly sustainedPlayback: ModernFrameStats;
  readonly pauseResume: ModernFrameStats;
  readonly backgroundForeground: ModernFrameStats;
  readonly audioSelection: ModernFrameStats;
  readonly channelSwitch: ModernFrameStats;
  readonly stop: ModernFrameStats;
}

export interface PlaybackActions {
  readonly initialPlayback: boolean;
  readonly pauseReleasedPresentation: boolean;
  readonly resumeReturnedToPlaying: boolean;
  readonly backgroundReleasedPresentation: boolean;
  readonly foregroundReturnedToPlaying: boolean;
  readonly alternateAudioAvailable: boolean;
  readonly alternateAudioSelected: boolean;
  readonly audioSelectionReleasedPresentation: boolean;
  readonly audioSelectionReturnedToPlaying: boolean;
  readonly channelSwitched: boolean;
  readonly channelSwitchReturnedToPlaying: boolean;
  readonly stopRemovedPresentation: boolean;
  readonly stopRemovedPlayer: boolean;
}

export interface PlaybackGateInput {
  readonly sustained: SustainedPlaybackSummary;
  readonly actions: PlaybackActions;
  readonly warmReplacementUiFrames: ModernFrameStats;
}

const safeCounter = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);

const playbackMarkerSchema = z
  .object({
    engine: z.literal("android-media3"),
    state: z.enum(["starting", "playing", "paused", "failed", "stopped"]),
    droppedFrames: safeCounter,
    bufferedDurationMs: safeCounter,
    decodedFrames: safeCounter.nullable(),
    processSilent: z.boolean(),
    muted: z.boolean(),
    volume: z.number().finite().min(0).max(1),
    routineUrlCount: safeCounter.max(100),
  })
  .strict();

/** Parses the deliberately aggregate-only WebView-to-harness marker. */
export function parsePlaybackMarker(input: unknown): ParseResult<PlaybackMarker> {
  const parsed = playbackMarkerSchema.safeParse(input);
  return parsed.success
    ? accept(parsed.data)
    : reject("the native playback marker was unavailable or contained non-aggregate fields");
}

/** Compresses periodic native status samples without retaining private media data. */
export function summarizePlaybackSamples(
  samples: readonly PlaybackMarker[],
  durationMs: number,
): ParseResult<SustainedPlaybackSummary> {
  if (
    samples.length < 2 ||
    !Number.isSafeInteger(durationMs) ||
    durationMs < 0
  ) {
    return reject("the sustained playback sample was incomplete");
  }
  const first = samples[0];
  const last = samples.at(-1);
  if (first === undefined || last === undefined) {
    return reject("the sustained playback sample was incomplete");
  }

  let countersMonotonic = true;
  for (let index = 1; index < samples.length; index += 1) {
    const previous = samples[index - 1];
    const current = samples[index];
    if (
      previous === undefined ||
      current === undefined ||
      current.droppedFrames < previous.droppedFrames ||
      (previous.decodedFrames !== null &&
        current.decodedFrames !== null &&
        current.decodedFrames < previous.decodedFrames)
    ) {
      countersMonotonic = false;
      break;
    }
  }

  const droppedFrames = Math.max(0, last.droppedFrames - first.droppedFrames);
  const decodedAvailable = samples.every((sample) => sample.decodedFrames !== null);
  const decodedFrames = decodedAvailable
    ? Math.max(0, (last.decodedFrames ?? 0) - (first.decodedFrames ?? 0))
    : null;
  const frameTotal = decodedFrames === null ? null : decodedFrames + droppedFrames;

  return accept({
    durationMs,
    sampleCount: samples.length,
    allPlaying: samples.every((sample) => sample.state === "playing"),
    allSilent: samples.every(
      (sample) => sample.processSilent && sample.muted && sample.volume === 0,
    ),
    routineUrlCount: Math.max(...samples.map((sample) => sample.routineUrlCount)),
    droppedFrames,
    decodedFrames,
    droppedFramePercent:
      frameTotal === null || frameTotal === 0
        ? null
        : roundPercent((droppedFrames / frameTotal) * 100),
    minimumBufferedDurationMs: Math.min(
      ...samples.map((sample) => sample.bufferedDurationMs),
    ),
    maximumBufferedDurationMs: Math.max(
      ...samples.map((sample) => sample.bufferedDurationMs),
    ),
    zeroBufferSamples: samples.filter((sample) => sample.bufferedDurationMs === 0)
      .length,
    countersMonotonic,
  });
}

/** Reads only Android's modern frame-deadline summary, never the legacy jank line. */
export function parseModernFrameStats(input: string): ParseResult<ModernFrameStats> {
  const totalMatches = Array.from(
    input.matchAll(/^\s*Total frames rendered:\s*(\d+)\s*$/gmu),
  );
  const jankyMatches = Array.from(
    input.matchAll(/^\s*Janky frames:\s*(\d+)\s*\(([0-9]+(?:\.[0-9]+)?)%\)\s*$/gmu),
  );
  if (totalMatches.length !== 1 || jankyMatches.length !== 1) {
    return reject("Android did not return one modern frame-deadline summary");
  }
  const totalFrames = Number(totalMatches[0]?.[1]);
  const jankyFrames = Number(jankyMatches[0]?.[1]);
  const reportedPercent = Number(jankyMatches[0]?.[2]);
  if (
    !Number.isSafeInteger(totalFrames) ||
    totalFrames < 0 ||
    !Number.isSafeInteger(jankyFrames) ||
    jankyFrames < 0 ||
    jankyFrames > totalFrames ||
    !Number.isFinite(reportedPercent) ||
    reportedPercent < 0 ||
    reportedPercent > 100
  ) {
    return reject("Android returned invalid modern frame-deadline counters");
  }
  const calculatedPercent =
    totalFrames === 0 ? 0 : roundPercent((jankyFrames / totalFrames) * 100);
  if (Math.abs(calculatedPercent - reportedPercent) > 0.02) {
    return reject("Android's modern frame-deadline percentage was inconsistent");
  }
  return accept({ totalFrames, jankyFrames, jankyPercent: calculatedPercent });
}

/** Combines disjoint journey counters without averaging their percentages. */
export function combineModernFrameStats(
  journeys: readonly ModernFrameStats[],
): ParseResult<ModernFrameStats> {
  if (journeys.length === 0) {
    return reject("no modern frame-deadline journeys were supplied");
  }
  const totalFrames = journeys.reduce(
    (total, journey) => total + journey.totalFrames,
    0,
  );
  const jankyFrames = journeys.reduce(
    (total, journey) => total + journey.jankyFrames,
    0,
  );
  if (
    !Number.isSafeInteger(totalFrames) ||
    !Number.isSafeInteger(jankyFrames) ||
    jankyFrames > totalFrames
  ) {
    return reject("the combined modern frame-deadline counters were invalid");
  }
  return accept({
    totalFrames,
    jankyFrames,
    jankyPercent:
      totalFrames === 0 ? 0 : roundPercent((jankyFrames / totalFrames) * 100),
  });
}

/** Applies the fixed physical-device gates, including the warm replacement UI sample. */
export function evaluatePlaybackGates(input: PlaybackGateInput): readonly string[] {
  const failures: string[] = [];
  const { sustained, actions, warmReplacementUiFrames } = input;

  if (sustained.durationMs < REQUIRED_SUSTAINED_MS) {
    failures.push("sustained-duration");
  }
  if (!sustained.allPlaying) {
    failures.push("sustained-playing");
  }
  if (!sustained.allSilent) {
    failures.push("per-process-silence");
  }
  if (sustained.routineUrlCount !== 0) {
    failures.push("routine-url-exposure");
  }
  if (!sustained.countersMonotonic) {
    failures.push("media-counter-reset");
  }
  if (
    sustained.decodedFrames !== null &&
    sustained.decodedFrames <
      Math.floor(
        (sustained.durationMs / 1_000) * MIN_DECODED_FRAMES_PER_SECOND,
      )
  ) {
    failures.push("decoded-frame-rate");
  }
  if (sustained.zeroBufferSamples !== 0) {
    failures.push("buffer-starvation");
  }
  if (
    sustained.droppedFramePercent === null
      ? sustained.droppedFrames > MAX_DROPPED_FRAMES
      : sustained.droppedFramePercent > 1
  ) {
    failures.push("dropped-frames");
  }

  for (const [gate, passed] of Object.entries(actions)) {
    if (!passed) {
      failures.push(gate);
    }
  }
  if (warmReplacementUiFrames.totalFrames < MIN_MODERN_UI_FRAMES) {
    failures.push("modern-frame-sample");
  }
  if (warmReplacementUiFrames.jankyPercent > MAX_MODERN_JANK_PERCENT) {
    failures.push("modern-frame-deadlines");
  }
  return failures;
}

function roundPercent(value: number): number {
  return Math.round(value * 100) / 100;
}

function accept<Value>(value: Value): ParseResult<Value> {
  return { ok: true, value };
}

function reject<Value = never>(reason: string): ParseResult<Value> {
  return { ok: false, reason };
}
