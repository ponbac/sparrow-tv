// Exercise the pinned dependency source used by Vite, with the repository's
// patch applied. JS keeps upstream's untyped TS sources outside the app
// typecheck.
import { describe, expect, it } from "vitest";
import { AC3Parser } from "../../../node_modules/mpegts.js/src/demux/ac3.ts";

// 48 kHz, frame size code 8: 128 words, so 256 bytes.
const FRAME_BYTES = 256;

describe("patched mpegts.js AC-3 parser", () => {
  it("reads the frames after bytes that only look like a syncframe header", () => {
    // Unpatched, a reserved rate code throws and a reserved size code never
    // returns: the parser runs on the WebView's main thread, so the second
    // froze the app. The throwing case comes first so a lost patch fails
    // this test instead of hanging it.
    for (const falseHeader of [
      [0x0b, 0x77, 0x00, 0x00, 0xc8, 0x40, 0x40, 0, 0, 0],
      [0x0b, 0x77, 0x00, 0x00, 0x3f, 0x40, 0x40, 0, 0, 0],
    ]) {
      expect(frameSizes(falseHeader)).toEqual([FRAME_BYTES, FRAME_BYTES]);
    }
    expect(frameSizes([])).toEqual([FRAME_BYTES, FRAME_BYTES]);
  });
});

/** Parses `prefix` followed by two whole syncframes; returns the frames read. */
function frameSizes(prefix) {
  // The parser stops searching seven bytes before the end of its data.
  const data = new Uint8Array(prefix.length + FRAME_BYTES * 2 + 8);
  data.set(prefix, 0);
  data.set(syncframe(), prefix.length);
  data.set(syncframe(), prefix.length + FRAME_BYTES);
  const parser = new AC3Parser(data);
  const sizes = [];
  for (
    let frame = parser.readNextAC3Frame();
    frame != null;
    frame = parser.readNextAC3Frame()
  ) {
    sizes.push(frame.data.byteLength);
  }
  return sizes;
}

function syncframe() {
  const bytes = new Uint8Array(FRAME_BYTES);
  // Syncword, crc1, rate code 0 with size code 8, bsid 8, 2/0 channels.
  bytes.set([0x0b, 0x77, 0x00, 0x00, 0x08, 0x40, 0x40]);
  return bytes;
}
