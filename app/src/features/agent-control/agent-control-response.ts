import { z } from "zod";

const count = z.number().finite().nonnegative();
const label = z.string().regex(/^[a-z][a-z0-9-]{0,63}$/);
const phase = z.enum([
  "idle",
  "starting",
  "playing",
  "autoplay-blocked",
  "replacing-audio",
  "suspending",
  "paused",
  "recovering",
  "failed",
  "stopping",
]);
const media = z.object({
  paused: z.boolean(),
  currentTimeMs: count,
  presentedFrames: count,
  readyState: count.optional(),
  seeking: z.boolean().optional(),
  bufferAheadMs: count.optional(),
  bufferedRangeCount: count.optional(),
  waiting: count.optional(),
  stalledEvents: count.optional(),
  seekingEvents: count.optional(),
  seeked: count.optional(),
  standstills: count.optional(),
  msSinceTimeAdvance: count.optional(),
  width: count.optional(),
  height: count.optional(),
  totalVideoFrames: count.optional(),
  droppedVideoFrames: count.optional(),
});
const diagnostics = z.object({
  phase,
  version: z.literal(2).optional(),
  engine: label.optional(),
  intent: label.optional(),
  transport: label.optional(),
  failure: label.nullable().optional(),
  recoveryCount: count.optional(),
  playingDurationMs: count.optional(),
  controls: z
    .object({
      volumePercent: count,
      muted: z.boolean(),
      fullscreen: z.boolean(),
    })
    .optional(),
  audio: z
    .object({ trackCount: count, selection: label, preferenceStatus: label })
    .optional(),
  media: media.nullable().optional(),
  transitions: z
    .array(z.object({ from: phase, to: phase }))
    .max(20)
    .optional(),
});
const snapshot = z.object({
  _tag: z.literal("snapshot"),
  channelName: z.string().nullable(),
  diagnostics: diagnostics.nullable(),
});
const success = z.discriminatedUnion("_tag", [
  z.object({ _tag: z.literal("pong"), ready: z.boolean() }),
  z.object({
    _tag: z.literal("channels"),
    channels: z
      .array(z.object({ name: z.string(), group: z.string() }))
      .max(40),
  }),
  z.object({ _tag: z.literal("tuned"), name: z.string() }),
  snapshot,
  z.object({ _tag: z.literal("stopped") }),
]);
const response = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), result: success }),
  z.object({
    ok: z.literal(false),
    error: z.union([
      z.object({
        _tag: z.enum([
          "invalid-request",
          "unavailable",
          "unauthenticated",
          "catalog-not-ready",
          "channel-not-found",
          "search-incomplete",
          "timeout",
          "cleanup-failed",
        ]),
      }),
      z.object({
        _tag: z.literal("ambiguous-channel"),
        names: z.array(z.string()).max(40),
      }),
    ]),
  }),
]);

/** Parsed and allowlisted CLI response, without arbitrary error or source fields. */
export type AgentWireResponse = z.output<typeof response>;
/** A fresh socket snapshot usable as a media progression baseline. */
export type AgentSnapshot = z.output<typeof snapshot>;

/** Parses socket JSON and projects only protocol fields; malformed input returns null. */
export function parseAgentWireResponse(
  input: unknown,
): AgentWireResponse | null {
  const parsed = response.safeParse(input);
  return parsed.success ? parsed.data : null;
}

/** Requires two playing snapshots from the same Channel with time AND frame growth. */
export function agentMediaAdvanced(
  previous: AgentSnapshot | null,
  current: AgentSnapshot,
): boolean {
  const before = previous?.diagnostics;
  const after = current.diagnostics;
  return (
    previous !== null &&
    previous.channelName === current.channelName &&
    current.channelName !== null &&
    before?.phase === "playing" &&
    after?.phase === "playing" &&
    before.media != null &&
    after.media != null &&
    !before.media.paused &&
    !after.media.paused &&
    before.recoveryCount === after.recoveryCount &&
    after.media.currentTimeMs > before.media.currentTimeMs &&
    after.media.presentedFrames > before.media.presentedFrames
  );
}
