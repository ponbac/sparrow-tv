import mpegts from "mpegts.js";
import { describe, expect, it } from "vitest";
import { clientSchemas, type ClientResult } from "../../client/contracts";
import {
  createNativeMpegtsLoader,
  NATIVE_PLAYBACK_SENTINEL,
  type NativePlaybackClient,
} from "./native-mpegts-loader";

const DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: `play1_${"a".repeat(32)}_1`,
  streamHandle: `stream1_${"b".repeat(16)}`,
  presentation: "webview-mse",
  tracks: [],
  selection: { _tag: "none" },
});
const SOURCE = { url: NATIVE_PLAYBACK_SENTINEL, duration: 0 };
const RANGE = { from: 0, to: -1 };

describe("native mpegts.js loader with the production BaseLoader", () => {
  it("pulls bounded chunks sequentially and preserves offsets through EOF", async () => {
    const chunks = [bytes(1, 2), bytes(3, 4, 5), new ArrayBuffer(0)];
    let activeReads = 0;
    let maximumReads = 0;
    const handles: string[] = [];
    const fixture = loaderFixture(async (input) => {
      handles.push(input.streamHandle);
      activeReads += 1;
      maximumReads = Math.max(maximumReads, activeReads);
      await Promise.resolve();
      activeReads -= 1;
      const chunk = chunks.shift();
      if (chunk === undefined) throw new Error("fixture exhausted");
      return success(chunk);
    });
    fixture.loader.open(SOURCE, RANGE);
    await until(() => fixture.completions.length === 1);
    expect(maximumReads).toBe(1);
    expect(handles).toEqual(Array<string>(3).fill(DESCRIPTOR.streamHandle));
    expect(fixture.arrivals).toEqual([
      [bytes(1, 2), 0, 2],
      [bytes(3, 4, 5), 2, 5],
    ]);
    expect(fixture.completions).toEqual([[0, 4]]);
    fixture.loader.destroy();
  });

  it.each([false, true])("parks an in-flight result across abort (early resume: %s)", async (earlyResume) => {
    const read = deferred<ClientResult<ArrayBuffer>>();
    let reads = 0;
    const fixture = loaderFixture(() => {
      reads += 1;
      return reads === 1 ? read.promise : Promise.resolve(success(new ArrayBuffer(0)));
    });
    fixture.loader.open(SOURCE, RANGE);
    await until(() => reads === 1);
    fixture.loader.abort();
    fixture.loader.abort();
    if (earlyResume) fixture.loader.open(SOURCE, RANGE);
    read.resolve(success(bytes(9, 8, 7)));
    await read.promise;
    await Promise.resolve();
    if (!earlyResume) {
      expect(fixture.arrivals).toEqual([]);
      fixture.loader.open(SOURCE, RANGE);
    }
    await until(() => fixture.completions.length === 1);
    expect(fixture.arrivals).toEqual([[bytes(9, 8, 7), 0, 3]]);
    expect(fixture.errors).toEqual([]);
    expect(reads).toBe(2);
    fixture.loader.destroy();
  });

  it.each(["eof", "failure"] as const)("retains %s received while paused instead of reading past it", async (outcome) => {
    const read = deferred<ClientResult<ArrayBuffer>>();
    let reads = 0;
    const fixture = loaderFixture(() => { reads += 1; return read.promise; });
    fixture.loader.open(SOURCE, RANGE);
    await until(() => reads === 1);
    fixture.loader.abort();
    read.resolve(outcome === "eof" ? success(new ArrayBuffer(0)) : {
      ok: false,
      error: { _tag: "transport", retryable: true, message: "private detail" },
    });
    await read.promise;
    await Promise.resolve();
    expect(fixture.errors).toEqual([]);
    expect(fixture.completions).toEqual([]);
    fixture.loader.open(SOURCE, RANGE);
    await until(() => fixture.errors.length + fixture.completions.length === 1);
    expect(reads).toBe(1);
    expect(fixture.arrivals).toEqual([]);
    expect(fixture.errors.length).toBe(outcome === "failure" ? 1 : 0);
    fixture.loader.destroy();
  });

  it("does not revive a destroyed loader when its final read resolves", async () => {
    const read = deferred<ClientResult<ArrayBuffer>>();
    let reads = 0;
    const fixture = loaderFixture(() => { reads += 1; return read.promise; });
    fixture.loader.open(SOURCE, RANGE);
    await until(() => reads === 1);
    fixture.loader.destroy();
    fixture.loader.open(SOURCE, RANGE);
    read.resolve(success(bytes(1, 2, 3)));
    await read.promise;
    await Promise.resolve();
    expect(fixture.arrivals).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(fixture.completions).toEqual([]);
    expect(reads).toBe(1);
  });

  it.each([false, true])("maps boundary failures to one safe loader error (rejection: %s)", async (rejects) => {
    const fixture = loaderFixture(async () => {
      const message = "synthetic private provider detail";
      if (rejects) throw new Error(message);
      return { ok: false, error: { _tag: "transport", retryable: false, message } };
    });
    fixture.loader.open(SOURCE, RANGE);
    await until(() => fixture.errors.length === 1);
    expect(fixture.errors).toEqual([[mpegts.LoaderErrors.EXCEPTION, {
      code: 0, msg: "The native stream was interrupted.",
    }]]);
    fixture.loader.destroy();
  });

  it.each([-1, 1, Number.NaN, Number.POSITIVE_INFINITY])("rejects unsupported byte range %s without reading", (from) => {
    let reads = 0;
    const fixture = loaderFixture(async () => { reads += 1; return success(new ArrayBuffer(0)); });
    fixture.loader.open(SOURCE, { from, to: -1 });
    expect(fixture.errors).toHaveLength(1);
    expect(reads).toBe(0);
    fixture.loader.destroy();
  });

  it("fails closed without reading for a forged URL", () => {
    let reads = 0;
    const fixture = loaderFixture(async () => { reads += 1; return success(new ArrayBuffer(0)); });
    fixture.loader.open({ url: "https://fixture.invalid/synthetic.ts", duration: 0 }, RANGE);
    expect(fixture.errors).toHaveLength(1);
    expect(reads).toBe(0);
    fixture.loader.destroy();
  });
});

function loaderFixture(read: NativePlaybackClient["read"]) {
  const Loader = createNativeMpegtsLoader({ read }, DESCRIPTOR);
  const loader = new Loader({}, {});
  const arrivals: unknown[][] = [];
  const errors: unknown[][] = [];
  const completions: unknown[][] = [];
  loader.onDataArrival = (...args) => { arrivals.push(args); };
  loader.onError = (...args) => { errors.push(args); };
  loader.onComplete = (...args) => { completions.push(args); };
  return { loader, arrivals, errors, completions };
}

function bytes(...values: number[]): ArrayBuffer {
  return Uint8Array.from(values).buffer;
}

function success<Value>(value: Value): { readonly ok: true; readonly value: Value } {
  return { ok: true, value };
}

async function until(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error("asynchronous fixture did not settle");
}

function deferred<Value>() {
  let resolve: ((value: Value) => void) | undefined;
  const promise = new Promise<Value>((next) => { resolve = next; });
  return {
    promise,
    resolve: (value: Value) => {
      if (resolve === undefined) throw new Error("deferred fixture was not initialized");
      resolve(value);
    },
  };
}
