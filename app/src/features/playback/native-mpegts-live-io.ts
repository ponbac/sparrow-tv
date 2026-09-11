/**
 * mpegts.js live IO is an HTTP range loader. BUFFER_FULL pauses it, then
 * resume reconnects with `_internalSeek`, which fires `insertDiscontinuity`
 * and WebKitGTK MSE drops the timeline after one GOP. Native playback is a
 * forward-only pipe: pause must retain unconsumed TS bytes, and resume must
 * reopen the same loader at its delivered offset without seeking. These hooks
 * apply only to Sparrow's native loader, never hosted FetchStreamLoader.
 *
 * Safari/WebKit live MSE also needs `MediaSource.duration = Infinity`, or
 * the first GOP is treated as a finite VOD and the pipeline ends. Chrome's
 * sequence append mode is not used: on WebKitGTK it split Eurosport into
 * two buffered ranges and the timeline still wiped.
 */

const DURATION_KEEPALIVE_MS = 250;

interface LiveIoLoader {
  readonly isWorking: () => boolean;
  readonly abort: () => void;
  readonly open: (source: unknown, range: { from: number; to: number }) => void;
}

interface LiveIoController {
  _paused: boolean;
  _resumeFrom: number;
  _currentRange: { from: number; to: number };
  _dataSource: unknown;
  _loader: LiveIoLoader;
  pause: () => void;
  resume: () => void;
}

/** HTMLMediaElement fields used when adopting live MSE duration. */
export interface LiveBufferedMedia {
  currentTime: number;
  readonly buffered: {
    readonly length: number;
    start(index: number): number;
    end(index: number): number;
  };
}

interface PaceableMseController {
  appendMediaSegment: (segment: unknown) => void;
}

interface LiveMediaSource {
  readyState: string;
  duration: number;
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

/** Releases live IO hooks. Stop is idempotent. */
export interface LiveMpegtsIoHandle {
  readonly stop: () => void;
}

/**
 * Replaces native IO pause/resume hooks and marks the MediaSource as unbounded
 * live media. `media` keeps duration applied after later appends.
 */
export function adoptLiveMpegtsIo(
  player: object,
  media?: LiveBufferedMedia,
): LiveMpegtsIoHandle {
  adoptLiveResume(player);
  adoptLiveDuration(player);
  if (media === undefined) {
    return idleHandle();
  }
  return keepLiveDuration(player);
}

function adoptLiveResume(player: object): void {
  const transmuxer = ownObject(player, "_transmuxer");
  const controller = transmuxer && ownObject(transmuxer, "_controller");
  const ioctl = controller && ownObject(controller, "_ioctl");
  if (
    controller === undefined ||
    ioctl === undefined ||
    !isLiveIoController(ioctl)
  ) {
    return;
  }
  // Keep the stash and its byteStart untouched. Upstream pause discards them
  // and rewinds the request range, which only a replayable HTTP source can do.
  ioctl.pause = () => {
    const loader = ioctl._loader;
    if (ioctl._paused || !loader.isWorking()) return;
    loader.abort();
    ioctl._resumeFrom = ioctl._currentRange.to + 1;
    ioctl._paused = true;
  };
  ioctl.resume = () => {
    if (ioctl._paused !== true) {
      return;
    }
    ioctl._paused = false;
    const from = ioctl._resumeFrom;
    ioctl._resumeFrom = 0;
    ioctl._currentRange = { from, to: -1 };
    ioctl._loader.open(ioctl._dataSource, { from, to: -1 });
  };
}

function adoptLiveDuration(player: object): void {
  const mediaSource = liveMediaSource(player);
  if (mediaSource === undefined) {
    return;
  }
  if (applyLiveDuration(mediaSource)) {
    return;
  }
  const onOpen = () => {
    mediaSource.removeEventListener("sourceopen", onOpen);
    applyLiveDuration(mediaSource);
  };
  mediaSource.addEventListener("sourceopen", onOpen);
}

function keepLiveDuration(player: object): LiveMpegtsIoHandle {
  const msectl = ownObject(player, "_msectl");
  if (msectl === undefined || !isPaceableMseController(msectl)) {
    return idleHandle();
  }

  const appendMediaSegment = msectl.appendMediaSegment.bind(msectl);
  msectl.appendMediaSegment = (segment: unknown) => {
    appendMediaSegment(segment);
    applyLiveDuration(liveMediaSource(player));
  };

  const timer = setInterval(() => {
    applyLiveDuration(liveMediaSource(player));
  }, DURATION_KEEPALIVE_MS);
  let stopped = false;
  return {
    stop: () => {
      if (stopped) {
        return;
      }
      stopped = true;
      clearInterval(timer);
    },
  };
}

function applyLiveDuration(mediaSource: LiveMediaSource | undefined): boolean {
  if (mediaSource === undefined || mediaSource.readyState !== "open") {
    return false;
  }
  if (mediaSource.duration === Number.POSITIVE_INFINITY) {
    return true;
  }
  try {
    mediaSource.duration = Number.POSITIVE_INFINITY;
    return true;
  } catch {
    return false;
  }
}

function liveMediaSource(player: object): LiveMediaSource | undefined {
  const msectl = ownObject(player, "_msectl");
  const mediaSource = msectl && ownObject(msectl, "_mediaSource");
  if (mediaSource === undefined || !isLiveMediaSource(mediaSource)) {
    return undefined;
  }
  return mediaSource;
}

function isLiveIoController(value: object): value is LiveIoController {
  const loader = ownObject(value, "_loader");
  return (
    loader !== undefined &&
    Reflect.get(loader, "type") === "sparrow-native-stream" &&
    typeof Reflect.get(loader, "isWorking") === "function" &&
    typeof Reflect.get(loader, "abort") === "function" &&
    typeof Reflect.get(loader, "open") === "function" &&
    typeof Reflect.get(value, "_paused") === "boolean" &&
    typeof Reflect.get(value, "_resumeFrom") === "number" &&
    ownObject(value, "_currentRange") !== undefined
  );
}

function isPaceableMseController(value: object): value is PaceableMseController {
  return typeof Reflect.get(value, "appendMediaSegment") === "function";
}

function isLiveMediaSource(value: object): value is LiveMediaSource {
  return (
    typeof Reflect.get(value, "addEventListener") === "function" &&
    typeof Reflect.get(value, "removeEventListener") === "function"
  );
}

function ownObject(value: object, key: string): object | undefined {
  const next = Reflect.get(value, key);
  return typeof next === "object" && next !== null ? next : undefined;
}

function idleHandle(): LiveMpegtsIoHandle {
  return {
    stop: () => undefined,
  };
}
