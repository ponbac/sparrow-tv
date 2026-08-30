import { createHash } from "node:crypto";
import { open, type FileHandle } from "node:fs/promises";

/** Inputs for downloading one digest-pinned helper into a new private file. */
export interface AppImageToolDownloadRequest {
  readonly url: string;
  readonly temporaryPath: string;
  readonly expectedSha256: string;
  readonly maximumBytes: number;
  readonly timeoutMs: number;
}

/** Safe outcome of the bounded AppImage helper download adapter. */
export type AppImageToolDownloadResult =
  | { readonly ok: true; readonly byteLength: number; readonly sha256: string }
  | {
      readonly ok: false;
      readonly reason: "download-failed" | "invalid-size" | "digest-mismatch";
    };

/** Streams one helper with a deadline, byte cap, incremental digest, and exclusive private file. */
export async function downloadPinnedAppImageTool(
  request: AppImageToolDownloadRequest,
): Promise<AppImageToolDownloadResult> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), request.timeoutMs);
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  try {
    const response = await fetch(request.url, {
      redirect: "follow",
      signal: controller.signal,
    });
    if (!response.ok || response.body === null) return downloadFailed();
    reader = response.body.getReader();
    const file = await open(request.temporaryPath, "wx", 0o600);
    try {
      const hash = createHash("sha256");
      let byteLength = 0;
      while (true) {
        const chunk = await reader.read();
        if (chunk.done) break;
        byteLength += chunk.value.byteLength;
        if (byteLength > request.maximumBytes) {
          return { ok: false, reason: "invalid-size" };
        }
        hash.update(chunk.value);
        await writeAll(file, chunk.value);
      }
      if (byteLength === 0) return { ok: false, reason: "invalid-size" };
      const digest = hash.digest("hex");
      if (digest !== request.expectedSha256) {
        return { ok: false, reason: "digest-mismatch" };
      }
      await file.sync();
      return { ok: true, byteLength, sha256: digest };
    } finally {
      await file.close();
    }
  } catch {
    return downloadFailed();
  } finally {
    clearTimeout(timeout);
    if (reader !== undefined) {
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
  }
}

async function writeAll(file: FileHandle, bytes: Uint8Array): Promise<void> {
  let offset = 0;
  while (offset < bytes.byteLength) {
    const write = await file.write(bytes, offset, bytes.byteLength - offset, null);
    if (write.bytesWritten === 0) throw new Error("the helper file made no write progress");
    offset += write.bytesWritten;
  }
}

function downloadFailed(): AppImageToolDownloadResult {
  return { ok: false, reason: "download-failed" };
}
