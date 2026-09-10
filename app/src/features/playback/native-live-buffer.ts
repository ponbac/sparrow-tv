const SAMPLE_INTERVAL_MS = 250;
const STALL_MS = 2_000;
const TIME_EPSILON_SECONDS = 0.04;
const HAVE_CURRENT_DATA = 2;
const MEDIA_EVENTS = ["waiting", "stalled", "seeking", "seeked"] as const;

/** Numeric media surface observed by the native live-buffer controller. */
export interface NativeLiveBufferMedia {
  currentTime: number;
  paused: boolean;
  seeking: boolean;
  readyState: number;
  readonly buffered: {
    readonly length: number;
    start(index: number): number;
    end(index: number): number;
  };
  readonly videoWidth?: number;
  readonly videoHeight?: number;
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
  getVideoPlaybackQuality?: () => {
    readonly totalVideoFrames: number;
    readonly droppedVideoFrames: number;
  };
  requestVideoFrameCallback?: (callback: (now: number) => void) => number;
  cancelVideoFrameCallback?: (handle: number) => void;
}

/** Privacy-safe media counters for one native Playback Session generation. */
export interface NativeLiveMediaObservation {
  readonly readyState: number;
  readonly paused: boolean;
  readonly seeking: boolean;
  readonly currentTime: number;
  readonly bufferAhead: number;
  readonly bufferedRangeCount: number;
  readonly waiting: number;
  readonly stalledEvents: number;
  readonly seekingEvents: number;
  readonly seeked: number;
  readonly standstills: number;
  /** Elapsed since observed forward progress, or watch creation if none yet. */
  readonly msSinceTimeAdvance: number;
  readonly width: number;
  readonly height: number;
  readonly totalVideoFrames: number;
  readonly droppedVideoFrames: number;
  readonly presentedFrames: number;
}

/** Session-scoped sampling; stop releases timers, events, and frame callbacks. */
export interface NativeLiveBufferWatch {
  readonly snapshot: () => NativeLiveMediaObservation;
  readonly stop: () => void;
}

/**
 * Observes an in-app live surface without changing playback. It records media
 * counters and standstills for copyable diagnostics. Mid-stream MSE seeks are
 * not used: they stall WebKitGTK. The returned stop is idempotent.
 */
export function watchNativeLiveBuffer(
  media: NativeLiveBufferMedia,
): NativeLiveBufferWatch {
  let lastTime = media.currentTime;
  let lastAdvanceAt = performance.now();
  let hasAdvanced = false;
  let stopped = false;
  let standingStill = false;
  let standstills = 0;
  let presentedFrames = 0;
  let frameHandle: number | null = null;
  const events = {
    waiting: 0,
    stalled: 0,
    seeking: 0,
    seeked: 0,
  };
  const listeners = MEDIA_EVENTS.map((name) => {
    const listener = () => {
      events[name] += 1;
    };
    media.addEventListener(name, listener);
    return { name, listener };
  });
  const onFrame = () => {
    if (stopped) return;
    presentedFrames += 1;
    if (media.requestVideoFrameCallback !== undefined) {
      frameHandle = media.requestVideoFrameCallback(onFrame);
    }
  };
  if (media.requestVideoFrameCallback !== undefined) {
    frameHandle = media.requestVideoFrameCallback(onFrame);
  }
  const timer = setInterval(() => {
    const current = media.currentTime;
    if (!Number.isFinite(current)) return;
    const now = performance.now();
    if (media.paused || media.seeking) {
      // A seek is not playback progress; neither it nor a pause refreshes age.
      lastTime = current;
      standingStill = false;
      return;
    }
    if (current < lastTime) lastTime = current;
    if (
      media.readyState >= HAVE_CURRENT_DATA &&
      current > lastTime + TIME_EPSILON_SECONDS
    ) {
      lastTime = current;
      lastAdvanceAt = now;
      hasAdvanced = true;
      standingStill = false;
      return;
    }
    // Startup may wait for data without a standstill. After playback starts,
    // losing readyState must not reset age or hide a stalled timeline.
    if (!hasAdvanced || now - lastAdvanceAt < STALL_MS || standingStill) return;
    standingStill = true;
    standstills += 1;
  }, SAMPLE_INTERVAL_MS);
  return {
    snapshot: () => snapshot(media, events, standstills, lastAdvanceAt, presentedFrames),
    stop: () => {
      if (stopped) return;
      stopped = true;
      clearInterval(timer);
      if (
        frameHandle !== null &&
        media.cancelVideoFrameCallback !== undefined
      ) {
        media.cancelVideoFrameCallback(frameHandle);
        frameHandle = null;
      }
      for (const { name, listener } of listeners) {
        media.removeEventListener(name, listener);
      }
    },
  };
}

function snapshot(
  media: NativeLiveBufferMedia,
  events: {
    waiting: number;
    stalled: number;
    seeking: number;
    seeked: number;
  },
  standstills: number,
  lastAdvanceAt: number,
  presentedFrames: number,
): NativeLiveMediaObservation {
  const quality = media.getVideoPlaybackQuality?.();
  return {
    readyState: media.readyState,
    paused: media.paused,
    seeking: media.seeking,
    currentTime: media.currentTime,
    bufferAhead: bufferAhead(media.buffered, media.currentTime),
    bufferedRangeCount: media.buffered.length,
    waiting: events.waiting,
    stalledEvents: events.stalled,
    seekingEvents: events.seeking,
    seeked: events.seeked,
    standstills,
    msSinceTimeAdvance: Math.max(0, performance.now() - lastAdvanceAt),
    width: media.videoWidth ?? 0,
    height: media.videoHeight ?? 0,
    totalVideoFrames: quality?.totalVideoFrames ?? 0,
    droppedVideoFrames: quality?.droppedVideoFrames ?? 0,
    presentedFrames,
  };
}

function bufferAhead(
  ranges: NativeLiveBufferMedia["buffered"],
  currentTime: number,
): number {
  if (ranges.length === 0 || !Number.isFinite(currentTime)) return 0;
  const end = ranges.end(ranges.length - 1);
  if (!Number.isFinite(end)) return 0;
  return Math.max(0, end - currentTime);
}
