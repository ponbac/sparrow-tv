const SAMPLE_INTERVAL_MS = 250;
const SETTLE_WINDOW_MS = 1_500;
const SEEK_COOLDOWN_MS = 30_000;
// Live samples arrive in multi-second batches; leave room for the next batch.
const MAX_FORWARD_BUFFER_SECONDS = 10;
const FORWARD_RESERVE_SECONDS = 5;

/** Numeric media surface observed by the native live-buffer controller. */
export type NativeLiveBufferMedia = Pick<
  HTMLMediaElement,
  "buffered" | "currentTime" | "paused" | "seeking"
>;

/**
 * Catches up only after the incoming timeline settles to approximately real
 * time. Startup bursts play normally instead of triggering a seek on every
 * buffer append. The returned release owns the timer and is idempotent.
 * `seek` must use the player's seek API so its internal seek state stays valid.
 */
export function watchNativeLiveBuffer(
  media: NativeLiveBufferMedia,
  seek: (seconds: number) => void,
): () => void {
  let anchor: { readonly at: number; readonly end: number } | null = null;
  let lastSeekAt = -Infinity;
  const timer = setInterval(() => {
    const ranges = media.buffered;
    if (media.paused || media.seeking || ranges.length === 0) {
      anchor = null;
      return;
    }
    const start = ranges.start(ranges.length - 1);
    const end = ranges.end(ranges.length - 1);
    const current = media.currentTime;
    const now = performance.now();
    if (![start, end, current].every(Number.isFinite)) return;
    if (anchor === null || end < anchor.end) {
      anchor = { at: now, end };
      return;
    }
    const elapsed = now - anchor.at;
    if (elapsed < SETTLE_WINDOW_MS) return;
    // Allow batching/jitter around real-time delivery, but not a fast initial
    // download containing tens of seconds of previously buffered live video.
    const settled = end - anchor.end <= (elapsed / 1_000) * 1.5;
    anchor = { at: now, end };
    if (
      !settled ||
      now - lastSeekAt < SEEK_COOLDOWN_MS ||
      end - current <= MAX_FORWARD_BUFFER_SECONDS ||
      end - start < FORWARD_RESERVE_SECONDS
    )
      return;
    lastSeekAt = now;
    seek(end - FORWARD_RESERVE_SECONDS);
  }, SAMPLE_INTERVAL_MS);
  return () => clearInterval(timer);
}
