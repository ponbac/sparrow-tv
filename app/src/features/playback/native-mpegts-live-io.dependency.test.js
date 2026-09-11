// Exercise the pinned dependency source used by Vite, not a lookalike IO mock.
// JS keeps upstream's untyped/internal TS sources outside the app typecheck.
import { afterEach, describe, expect, it, vi } from "vitest";
import IOController from "../../../node_modules/mpegts.js/src/io/io-controller.js";
import FetchStreamLoader from "../../../node_modules/mpegts.js/src/io/fetch-stream-loader.js";
import TSDemuxer from "../../../node_modules/mpegts.js/src/demux/ts-demuxer.ts";
import TransmuxingController from "../../../node_modules/mpegts.js/src/core/transmuxing-controller.js";
import { createDefaultConfig } from "../../../node_modules/mpegts.js/src/config.js";
import { clientSchemas } from "../../client/contracts";
import { adoptLiveMpegtsIo } from "./native-mpegts-live-io";
import { createNativeMpegtsLoader, NATIVE_PLAYBACK_SENTINEL } from "./native-mpegts-loader";

const DESCRIPTOR = clientSchemas.nativePlaybackDescriptor.parse({
  _tag: "tauri-native-stream",
  sessionId: `play1_${"a".repeat(32)}_1`,
  streamHandle: `stream1_${"b".repeat(16)}`,
  presentation: "webview-mse",
  tracks: [],
  selection: { _tag: "none" },
});

afterEach(() => vi.unstubAllGlobals());

describe("native IOController / TS demuxer byte continuity", () => {
  it.each([false, true])("preserves split packets across repeated pauses (resume before read settles: %s)", async (resumeEarly) => {
    const input = transportPackets(1400);
    const pending = [];
    let activeReads = 0;
    let maximumReads = 0;
    const Loader = createNativeMpegtsLoader({
      read: () => {
        activeReads += 1;
        maximumReads = Math.max(maximumReads, activeReads);
        return new Promise((resolve) => pending.push((chunk) => {
          activeReads -= 1;
          resolve({ ok: true, value: chunk });
        }));
      },
    }, DESCRIPTOR);
    const config = { ...createDefaultConfig(), isLive: true, enableStashBuffer: false, customLoader: Loader };
    const io = new IOController({ url: NATIVE_PLAYBACK_SENTINEL }, config);
    const demux = new TSDemuxer(TSDemuxer.probe(input.buffer), config);
    const errors = [];
    const consumedChunks = [];
    let consumedBytes = 0;
    let discontinuities = 0;
    let completed = false;
    demux.onError = (...error) => errors.push(error);
    demux.onMediaInfo = () => {};
    demux.onTrackMetadata = () => {};
    demux.onDataAvailable = () => {};
    io.onError = (...error) => errors.push(error);
    io.onComplete = () => { completed = true; };
    io.onSeeked = () => { discontinuities += 1; };
    io.onDataArrival = (chunk, start) => {
      expect(start).toBe(consumedBytes);
      const count = demux.parseChunks(chunk, start);
      consumedChunks.push(new Uint8Array(chunk.slice(0, count)));
      consumedBytes += count;
      return count;
    };
    io.open();
    const adopted = adoptLiveMpegtsIo({ _transmuxer: { _controller: { _ioctl: io } } });
    try {
      await until(() => pending.length === 1);
      pending.shift()(input.slice(0, 65536).buffer);
      await until(() => pending.length === 1);
      // 64KiB is 348 TS packets plus 112 bytes of the next packet.
      expect(consumedBytes).toBe(65536 - 112);
      for (let offset = 65536; offset < input.length; offset += 65536) {
        io.pause();
        io.pause();
        if (resumeEarly) {
          io.resume();
          io.resume();
        }
        pending.shift()(input.slice(offset, offset + 65536).buffer);
        if (!resumeEarly) {
          await until(() => activeReads === 0);
          await Promise.resolve();
          io.resume();
          io.resume();
        }
        await until(() => pending.length === 1);
      }
      pending.shift()(new ArrayBuffer(0));
      await until(() => completed);
      const output = new Uint8Array(consumedBytes);
      let offset = 0;
      for (const chunk of consumedChunks) {
        output.set(chunk, offset);
        offset += chunk.length;
      }
      expect(output).toEqual(input);
      expect(maximumReads).toBe(1);
      expect(discontinuities).toBe(0);
      expect(errors).toEqual([]);
    } finally {
      adopted.stop();
      io.destroy();
      demux.destroy();
    }
  });
});

describe("hosted live IO remains replayable HTTP", () => {
  it("reconnects using a fresh FetchStreamLoader and replays the partial TS packet", async () => {
    const input = transportPackets(700);
    const requests = [];
    vi.stubGlobal("fetch", async (url, options) => {
      let stream;
      const body = new ReadableStream({ start(controller) { stream = controller; } });
      options.signal.addEventListener("abort", () => stream.error(new DOMException("Aborted", "AbortError")));
      requests.push({ range: options.headers.get("Range"), stream });
      return { ok: true, status: 200, url, headers: new Headers(), body };
    });
    const config = { ...createDefaultConfig(), isLive: true, enableStashBuffer: false, customLoader: FetchStreamLoader };
    const io = new IOController({ url: "https://fixture.invalid/synthetic.ts" }, config);
    const demux = new TSDemuxer(TSDemuxer.probe(input.buffer), config);
    const errors = [];
    let total = 0;
    let seeked = 0;
    demux.onError = (...error) => errors.push(error);
    demux.onMediaInfo = () => {};
    demux.onTrackMetadata = () => {};
    demux.onDataAvailable = () => {};
    io.onError = (...error) => errors.push(error);
    io.onSeeked = () => { seeked += 1; };
    io.onDataArrival = (chunk, start) => {
      expect(start).toBe(total);
      const consumed = demux.parseChunks(chunk, start);
      expect(new Uint8Array(chunk, 0, consumed)).toEqual(input.slice(start, start + consumed));
      total += consumed;
      return consumed;
    };
    // Even accidental adoption must leave non-native pause/resume untouched.
    adoptLiveMpegtsIo({ _transmuxer: { _controller: { _ioctl: io } } });
    try {
      io.open();
      await until(() => requests.length === 1);
      requests[0].stream.enqueue(input.slice(0, 65536));
      await until(() => total === 65536 - 112);
      io.pause();
      io.resume();
      await until(() => requests.length === 2);
      expect(requests[1].range).toBe(`bytes=${65536 - 112}-`);
      requests[1].stream.enqueue(input.slice(65536 - 112));
      await until(() => total === input.length);
      expect(seeked).toBe(1);
      expect(errors).toEqual([]);
    } finally {
      io.destroy();
      demux.destroy();
    }
  });

  it("does not suppress genuine live reconnect discontinuities in the transmuxer", async () => {
    vi.stubGlobal("fetch", async (url) => ({
      ok: true, status: 200, url, headers: new Headers(),
      body: new ReadableStream({ start(stream) { stream.close(); } }),
    }));
    const controller = new TransmuxingController(
      { type: "mpegts", url: "https://fixture.invalid/synthetic.ts" },
      { ...createDefaultConfig(), isLive: true, customLoader: FetchStreamLoader },
    );
    let discontinuities = 0;
    try {
      controller.start();
      // Record the real controller's downstream remuxer port. Exercise its
      // public lifecycle and real IO wiring, not the private seek callback.
      controller._remuxer = { insertDiscontinuity: () => { discontinuities += 1; }, destroy: () => {} };
      controller.pause();
      controller.resume();
      expect(discontinuities).toBe(1);
      await Promise.resolve();
    } finally {
      controller.destroy();
    }
  });
});

function transportPackets(count) {
  const data = new Uint8Array(count * 188);
  for (let i = 0; i < count; i += 1) {
    const packet = data.subarray(i * 188, (i + 1) * 188);
    // Valid null PID packets with distinct payloads; no provider/media fixture.
    packet.fill((i % 181) + 1);
    packet.set([0x47, 0x1f, 0xff, 0x10 | (i % 16)]);
  }
  return data;
}

async function until(predicate) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error("dependency fixture did not settle");
}
