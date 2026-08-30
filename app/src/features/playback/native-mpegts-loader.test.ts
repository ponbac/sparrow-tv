import mpegts from "mpegts.js";
import { describe, expect, it, vi } from "vitest";
import { clientSchemas, type ClientResult } from "../../client/contracts";
import {
  createNativeMpegtsLoader,
  NATIVE_PLAYBACK_SENTINEL,
  type NativeLoaderRuntime,
  type NativePlaybackClient,
} from "./native-mpegts-loader";

type RuntimeLoader = InstanceType<NativeLoaderRuntime["BaseLoader"]>;

class FixtureBaseLoader {
  _status: number = mpegts.LoaderStatus.kIdle;
  _needStash: boolean = false;
  readonly type: string;
  onContentLengthKnown: RuntimeLoader["onContentLengthKnown"] = vi.fn();
  onURLRedirect: RuntimeLoader["onURLRedirect"] = vi.fn();
  onDataArrival: RuntimeLoader["onDataArrival"] = vi.fn();
  onError: RuntimeLoader["onError"] = vi.fn();
  onComplete: RuntimeLoader["onComplete"] = vi.fn();

  constructor(typeName: string) {
    this.type = typeName;
  }

  get status(): number {
    return this._status;
  }

  get needStashBuffer(): boolean {
    return this._needStash;
  }

  isWorking(): boolean {
    return (
      this._status === mpegts.LoaderStatus.kConnecting ||
      this._status === mpegts.LoaderStatus.kBuffering
    );
  }

  destroy(): void {
    this._status = mpegts.LoaderStatus.kIdle;
  }

  open(
    dataSource: Parameters<RuntimeLoader["open"]>[0],
    range: Parameters<RuntimeLoader["open"]>[1],
  ): void {
    void dataSource;
    void range;
  }

  abort(): void {}
}

const RUNTIME: NativeLoaderRuntime = {
  BaseLoader: FixtureBaseLoader,
  LoaderStatus: mpegts.LoaderStatus,
  LoaderErrors: mpegts.LoaderErrors,
};

const DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: `play1_${"a".repeat(32)}_1`,
  streamHandle: `stream1_${"b".repeat(16)}`,
});

describe("native mpegts.js loader", () => {
  it("pulls bounded chunks sequentially, preserves offsets, and releases at EOF", async () => {
    const chunks = [bytes(1, 2), bytes(3, 4, 5), new ArrayBuffer(0)];
    let activeReads = 0;
    let maximumReads = 0;
    const readPlayback = vi.fn(async () => {
      activeReads += 1;
      maximumReads = Math.max(maximumReads, activeReads);
      await Promise.resolve();
      activeReads -= 1;
      return success(chunks.shift() ?? new ArrayBuffer(0));
    });
    const stopPlayback = vi.fn(async () => success(undefined));
    const Loader = createNativeMpegtsLoader(
      { readPlayback, stopPlayback },
      DESCRIPTOR,
      RUNTIME,
    );
    const loader = new Loader({}, {});
    loader.onDataArrival = vi.fn();
    loader.onComplete = vi.fn();

    loader.open(
      { url: NATIVE_PLAYBACK_SENTINEL, duration: 0 },
      { from: 0, to: -1 },
    );
    await until(() => vi.mocked(loader.onComplete).mock.calls.length === 1);

    expect(maximumReads).toBe(1);
    expect(readPlayback).toHaveBeenCalledTimes(3);
    expect(readPlayback).toHaveBeenNthCalledWith(1, {
      sessionId: DESCRIPTOR.sessionId,
      streamHandle: DESCRIPTOR.streamHandle,
      signal: expect.any(AbortSignal),
    });
    expect(vi.mocked(loader.onDataArrival).mock.calls).toEqual([
      [bytes(1, 2), 0, 2],
      [bytes(3, 4, 5), 2, 5],
    ]);
    expect(loader.onComplete).toHaveBeenCalledWith(0, 4);
    expect(stopPlayback).toHaveBeenCalledTimes(1);

    loader.abort();
    loader.destroy();
    expect(stopPlayback).toHaveBeenCalledTimes(1);
  });

  it("aborts an in-flight native read and stops its exact session once", async () => {
    const read = deferred<ClientResult<ArrayBuffer>>();
    const readPlayback = vi.fn(() => read.promise);
    const stopPlayback = vi.fn(async () => success(undefined));
    const Loader = createNativeMpegtsLoader(
      { readPlayback, stopPlayback },
      DESCRIPTOR,
      RUNTIME,
    );
    const loader = new Loader({}, {});
    loader.onDataArrival = vi.fn();
    loader.onComplete = vi.fn();
    loader.onError = vi.fn();
    loader.open(
      { url: NATIVE_PLAYBACK_SENTINEL, duration: 0 },
      { from: 0, to: -1 },
    );
    const signal = requireSignal(readPlayback);

    loader.abort();
    loader.abort();
    expect(signal.aborted).toBe(true);
    expect(stopPlayback).toHaveBeenCalledTimes(1);
    expect(stopPlayback).toHaveBeenCalledWith({
      sessionId: DESCRIPTOR.sessionId,
    });

    read.resolve({ ok: false, error: { _tag: "cancelled" } });
    await read.promise;
    await Promise.resolve();
    expect(loader.onDataArrival).not.toHaveBeenCalled();
    expect(loader.onComplete).not.toHaveBeenCalled();
    expect(loader.onError).not.toHaveBeenCalled();
  });

  it("maps transport failures to one fixed safe loader error", async () => {
    const privateMessage = "https://user:secret@provider.invalid/live";
    const readPlayback = vi.fn(async () => ({
      ok: false as const,
      error: {
        _tag: "transport" as const,
        retryable: false,
        message: privateMessage,
      },
    }));
    const stopPlayback = vi.fn(async () => success(undefined));
    const Loader = createNativeMpegtsLoader(
      { readPlayback, stopPlayback },
      DESCRIPTOR,
      RUNTIME,
    );
    const loader = new Loader({}, {});
    loader.onError = vi.fn();

    loader.open(
      { url: NATIVE_PLAYBACK_SENTINEL, duration: 0 },
      { from: 0, to: -1 },
    );
    await until(() => vi.mocked(loader.onError).mock.calls.length === 1);

    expect(JSON.stringify(vi.mocked(loader.onError).mock.calls)).not.toContain(
      "provider.invalid",
    );
    expect(loader.onError).toHaveBeenCalledWith(
      mpegts.LoaderErrors.EXCEPTION,
      { code: 0, msg: "The native stream was interrupted." },
    );
    expect(stopPlayback).toHaveBeenCalledTimes(1);
  });

  it("fails closed without reading for a forged URL or nonzero range", async () => {
    for (const [url, from] of [
      ["https://provider.invalid/live", 0],
      [NATIVE_PLAYBACK_SENTINEL, 1],
    ] as const) {
      const client = fixtureClient();
      const Loader = createNativeMpegtsLoader(client, DESCRIPTOR, RUNTIME);
      const loader = new Loader({}, {});
      loader.onError = vi.fn();
      loader.open({ url, duration: 0 }, { from, to: -1 });
      await until(() => vi.mocked(loader.onError).mock.calls.length === 1);
      expect(client.readPlayback).not.toHaveBeenCalled();
      expect(client.stopPlayback).toHaveBeenCalledTimes(1);
    }
  });
});

function fixtureClient(): NativePlaybackClient & {
  readonly readPlayback: ReturnType<typeof vi.fn>;
  readonly stopPlayback: ReturnType<typeof vi.fn>;
} {
  return {
    readPlayback: vi.fn(async () => success(new ArrayBuffer(0))),
    stopPlayback: vi.fn(async () => success(undefined)),
  };
}

function bytes(...values: number[]): ArrayBuffer {
  return Uint8Array.from(values).buffer;
}

function success<Value>(value: Value): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}

function requireSignal(readPlayback: ReturnType<typeof vi.fn>): AbortSignal {
  const first = readPlayback.mock.calls[0]?.[0];
  if (typeof first !== "object" || first === null || !("signal" in first)) {
    throw new Error("expected a native read signal");
  }
  const signal = first.signal;
  if (!(signal instanceof AbortSignal)) {
    throw new Error("expected an AbortSignal");
  }
  return signal;
}

async function until(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  throw new Error("asynchronous fixture did not settle");
}

function deferred<Value>(): {
  readonly promise: Promise<Value>;
  readonly resolve: (value: Value) => void;
} {
  let resolve: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((next) => {
    resolve = next;
  });
  return {
    promise,
    resolve: (value) => {
      if (resolve === undefined) {
        throw new Error("deferred fixture was not initialized");
      }
      resolve(value);
    },
  };
}
