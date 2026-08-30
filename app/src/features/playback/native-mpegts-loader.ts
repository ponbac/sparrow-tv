import mpegts from "mpegts.js";
import type {
  InstalledSparrowClient,
  NativePlaybackDescriptor,
} from "../../client/contracts";

/** Private sentinel consumed only by mpegts.js; it carries no provider data. */
export const NATIVE_PLAYBACK_SENTINEL = "sparrow://native-stream";

type MpegtsLoader = InstanceType<typeof mpegts.BaseLoader>;

interface LoaderDataSource {
  readonly url: string;
}

interface LoaderRange {
  readonly from: number;
  readonly to: number;
}

/** Narrow mpegts.js loader seam used by the native adapter and its tests. */
export interface NativeLoaderRuntime {
  readonly BaseLoader: typeof mpegts.BaseLoader;
  readonly LoaderStatus: typeof mpegts.LoaderStatus;
  readonly LoaderErrors: typeof mpegts.LoaderErrors;
}

/** Constructor shape accepted by mpegts.js for a custom transport loader. */
export interface NativeLoaderConstructor {
  new (seekHandler: unknown, config: unknown): MpegtsLoader;
}

/** The exact client surface owned by one native loader. */
export type NativePlaybackClient = Pick<
  InstalledSparrowClient,
  "readPlayback" | "stopPlayback"
>;

/**
 * Binds one opaque Playback Session to the pull-only mpegts.js loader surface.
 * Reads are sequential, bounded by the client contract, and never expose a URL.
 */
export function createNativeMpegtsLoader(
  client: NativePlaybackClient,
  descriptor: NativePlaybackDescriptor,
  runtime: NativeLoaderRuntime = mpegts,
  releaseSession: () => Promise<void> = () =>
    client
      .stopPlayback({ sessionId: descriptor.sessionId })
      .then(() => undefined)
      .catch(() => undefined),
): NativeLoaderConstructor {
  return class NativeMpegtsLoader extends runtime.BaseLoader {
    #active = false;
    #opened = false;
    #offset = 0;
    #readController: AbortController | null = null;
    #release: Promise<void> | null = null;

    constructor(seekHandler: unknown, config: unknown) {
      super("sparrow-native-stream");
      void seekHandler;
      void config;
    }

    open(dataSource: LoaderDataSource, range: LoaderRange): void {
      if (
        this.#opened ||
        dataSource.url !== NATIVE_PLAYBACK_SENTINEL ||
        range.from !== 0
      ) {
        this.#fail();
        return;
      }

      this.#opened = true;
      this.#active = true;
      this._status = runtime.LoaderStatus.kConnecting;
      void this.#pull();
    }

    abort(): void {
      if (!this.#active && this.#release !== null) {
        return;
      }
      this.#active = false;
      this._status = runtime.LoaderStatus.kIdle;
      this.#readController?.abort();
      this.#readController = null;
      void this.#releaseSession();
    }

    destroy(): void {
      this.abort();
      super.destroy();
    }

    async #pull(): Promise<void> {
      while (this.#active) {
        const controller = new AbortController();
        this.#readController = controller;
        const result = await client.readPlayback({
          sessionId: descriptor.sessionId,
          streamHandle: descriptor.streamHandle,
          signal: controller.signal,
        });
        if (this.#readController === controller) {
          this.#readController = null;
        }
        if (!this.#active) {
          return;
        }
        if (!result.ok) {
          this.#fail();
          return;
        }

        const chunk = result.value;
        if (chunk.byteLength === 0) {
          this.#active = false;
          this._status = runtime.LoaderStatus.kComplete;
          void this.#releaseSession();
          invokeIfFunction(this.onComplete, 0, Math.max(0, this.#offset - 1));
          return;
        }

        this._status = runtime.LoaderStatus.kBuffering;
        const byteStart = this.#offset;
        this.#offset += chunk.byteLength;
        invokeIfFunction(this.onDataArrival, chunk, byteStart, this.#offset);
      }
    }

    #fail(): void {
      if (!this.#active && this._status === runtime.LoaderStatus.kError) {
        return;
      }
      this.#active = false;
      this._status = runtime.LoaderStatus.kError;
      this.#readController?.abort();
      this.#readController = null;
      void this.#releaseSession();
      invokeIfFunction(
        this.onError,
        // mpegts.js declares callback values as the constants object itself.
        runtime.LoaderErrors.EXCEPTION as unknown as Parameters<
          MpegtsLoader["onError"]
        >[0],
        {
          code: 0,
          msg: "The native stream was interrupted.",
        },
      );
    }

    #releaseSession(): Promise<void> {
      if (this.#release !== null) {
        return this.#release;
      }
      this.#release = releaseSession().catch(() => undefined);
      return this.#release;
    }
  };
}

function invokeIfFunction<Arguments extends readonly unknown[]>(
  callback: ((...args: Arguments) => void) | null | undefined,
  ...args: Arguments
): void {
  if (typeof callback === "function") {
    callback(...args);
  }
}
