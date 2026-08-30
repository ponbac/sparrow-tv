// @vitest-environment node

import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { createServer, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { downloadPinnedAppImageTool } from "./appimage-tool-download.ts";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

describe("AppImage helper download", () => {
  it("streams chunked bytes into one private digest-verified file", async () => {
    const first = new Uint8Array([1, 2, 3]);
    const second = new Uint8Array([4, 5]);
    const expected = new Uint8Array([...first, ...second]);
    await withServer(
      (response) => {
        response.write(first);
        response.end(second);
      },
      async (url) => {
        const destination = await temporaryPath();
        const result = await downloadPinnedAppImageTool({
          url,
          temporaryPath: destination,
          expectedSha256: digest(expected),
          maximumBytes: expected.byteLength,
          timeoutMs: 1_000,
        });

        expect(result).toEqual({
          ok: true,
          byteLength: expected.byteLength,
          sha256: digest(expected),
        });
        expect(await readFile(destination)).toEqual(Buffer.from(expected));
        expect((await stat(destination)).mode & 0o077).toBe(0);
      },
    );
  });

  it("stops an oversized body before buffering the response", async () => {
    await withServer(
      (response) => response.end(new Uint8Array([1, 2, 3, 4, 5])),
      async (url) => {
        const result = await downloadPinnedAppImageTool({
          url,
          temporaryPath: await temporaryPath(),
          expectedSha256: digest(new Uint8Array([1, 2, 3, 4, 5])),
          maximumBytes: 4,
          timeoutMs: 1_000,
        });

        expect(result).toEqual({ ok: false, reason: "invalid-size" });
      },
    );
  });

  it("aborts a stalled body and rejects a digest mismatch", async () => {
    await withServer(
      (response) => {
        response.write(new Uint8Array([1]));
      },
      async (url) => {
        const stalled = await downloadPinnedAppImageTool({
          url,
          temporaryPath: await temporaryPath(),
          expectedSha256: digest(new Uint8Array([1])),
          maximumBytes: 4,
          timeoutMs: 25,
        });
        expect(stalled).toEqual({ ok: false, reason: "download-failed" });
      },
    );

    await withServer(
      (response) => response.end(new Uint8Array([1, 2, 3])),
      async (url) => {
        const mismatched = await downloadPinnedAppImageTool({
          url,
          temporaryPath: await temporaryPath(),
          expectedSha256: "0".repeat(64),
          maximumBytes: 4,
          timeoutMs: 1_000,
        });
        expect(mismatched).toEqual({ ok: false, reason: "digest-mismatch" });
      },
    );
  });
});

async function temporaryPath(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "sparrow-appimage-download-test-"));
  temporaryDirectories.push(directory);
  return join(directory, "helper");
}

async function withServer(
  respond: (response: ServerResponse) => void,
  operation: (url: string) => Promise<void>,
): Promise<void> {
  const server = createServer((_request, response) => respond(response));
  await new Promise<void>((resolveListening) => server.listen(0, "127.0.0.1", resolveListening));
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("test server has no port");
  try {
    await operation(`http://127.0.0.1:${address.port}/helper`);
  } finally {
    server.closeAllConnections();
    await new Promise<void>((resolveClosed, reject) => {
      server.close((error) => (error === undefined ? resolveClosed() : reject(error)));
    });
  }
}

function digest(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}
