import mpegts from "mpegts.js";
import type {
  ClientResult,
  InstalledPlaybackSession,
  NativeStreamPlaybackTransport,
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
export type NativePlaybackClient = Pick<InstalledPlaybackSession, "read">;

/**
 * Binds one opaque Playback Session to the pull-only mpegts.js loader surface.
 * Reads are sequential, bounded by the client contract, and never expose a URL.
 *
 * mpegts.js treats BUFFER_FULL by aborting the loader and later reopening it
 * with a byte range, as if the source were HTTP. This adapter is a live pipe:
 * pause must not cancel the in-flight native read, and resume continues from
 * the parked result at the exact delivered byte offset. The native engine's
 * IO adapter retains the demuxer's unconsumed stash and replaces pause/resume
 * without an HTTP seek or MSE discontinuity. Other ranges fail closed.
 */
export function createNativeMpegtsLoader(
  client: NativePlaybackClient,
  descriptor: NativeStreamPlaybackTransport,
  runtime: NativeLoaderRuntime = mpegts,
): NativeLoaderConstructor {
  return class NativeMpegtsLoader extends runtime.BaseLoader {
    #active = false;
    #destroyed = false;
    #generation = 0;
    #offset = 0;
    #drain: Promise<void> = Promise.resolve();
    #parked: ClientResult<ArrayBuffer> | null = null;

    constructor(seekHandler: unknown, config: unknown) {
      super("sparrow-native-stream");
      void seekHandler;
      void config;
    }

    open(dataSource: LoaderDataSource, range: LoaderRange): void {
      if (this.#destroyed) return;
      if (
        dataSource.url !== NATIVE_PLAYBACK_SENTINEL ||
        range.from !== this.#offset ||
        range.to !== -1
      ) {
        this.#fail();
        return;
      }
      if (this.#active) {
        return;
      }

      this.#active = true;
      this._status = runtime.LoaderStatus.kConnecting;
      void this.#pull(++this.#generation);
    }

    abort(): void {
      if (!this.#active) {
        return;
      }
      this.#active = false;
      this.#generation += 1;
      this._status = runtime.LoaderStatus.kIdle;
    }

    destroy(): void {
      this.abort();
      this.#destroyed = true;
      this.#parked = null;
      super.destroy();
    }

    async #pull(generation: number): Promise<void> {
      const previous = this.#drain;
      let release: () => void = () => undefined;
      this.#drain = new Promise<void>((resolve) => {
        release = () => {
          resolve();
        };
      });
      try {
        await previous;
        while (this.#active && generation === this.#generation) {
          const result = await this.#nextChunk();
          if (!this.#active || generation !== this.#generation) {
            if (!this.#destroyed) this.#parked = result;
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
            invokeIfFunction(this.onComplete, 0, Math.max(0, this.#offset - 1));
            return;
          }

          this._status = runtime.LoaderStatus.kBuffering;
          const byteStart = this.#offset;
          this.#offset += chunk.byteLength;
          invokeIfFunction(this.onDataArrival, chunk, byteStart, this.#offset);
        }
      } finally {
        release();
      }
    }

    async #nextChunk(): Promise<ClientResult<ArrayBuffer>> {
      if (this.#parked !== null) {
        const result = this.#parked;
        this.#parked = null;
        return result;
      }
      try {
        return await client.read({ streamHandle: descriptor.streamHandle });
      } catch {
        // The client is an async boundary. Never leak a rejected provider error
        // from detached pull work, including when the result must be parked.
        return {
          ok: false,
          error: {
            _tag: "transport",
            retryable: true,
            message: "The native stream was interrupted.",
          },
        };
      }
    }

    #fail(): void {
      if (!this.#active && this._status === runtime.LoaderStatus.kError) {
        return;
      }
      this.#active = false;
      this._status = runtime.LoaderStatus.kError;
      invokeIfFunction(
        this.onError,
        // SAFETY: mpegts.js declares the callback argument as the constants
        // object; its runtime actually passes one of that object's strings.
        runtime.LoaderErrors.EXCEPTION as unknown as Parameters<
          MpegtsLoader["onError"]
        >[0],
        {
          code: 0,
          msg: "The native stream was interrupted.",
        },
      );
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
