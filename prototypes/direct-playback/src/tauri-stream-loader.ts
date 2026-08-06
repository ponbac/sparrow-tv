import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import mpegts from "mpegts.js";

type LoaderError = { code: number; msg: string };
type LoaderRange = { from: number; to: number };
type LoaderDataSource = { url: string; redirectedURL?: string };
type SeekHandler = {
  getConfig: (url: string, range: LoaderRange) => { url: string; headers: HeadersInit };
  removeURLParameters: (url: string) => string;
};
type LoaderConfig = {
  reuseRedirectedURL?: boolean;
  headers?: Record<string, string>;
};

let byteObserver: (bytes: number) => void = () => undefined;

export function observeNativeBytes(observer: (bytes: number) => void): void {
  byteObserver = observer;
}

export class TauriStreamLoader {
  _status: number = mpegts.LoaderStatus.kIdle;
  _needStash = true;
  onContentLengthKnown: ((length: number) => void) | null = null;
  onURLRedirect: ((url: string) => void) | null = null;
  onDataArrival: ((chunk: ArrayBuffer, start: number, received?: number) => void) | null = null;
  onError: ((type: string, error: LoaderError) => void) | null = null;
  onComplete: ((from: number, to: number) => void) | null = null;

  private readonly seekHandler: SeekHandler;
  private readonly config: LoaderConfig;
  private abortController: AbortController | null = null;
  private receivedLength = 0;
  private range: LoaderRange = { from: 0, to: -1 };

  constructor(seekHandler: SeekHandler, config: LoaderConfig) {
    this.seekHandler = seekHandler;
    this.config = config;
  }

  get type(): string {
    return "tauri-http-stream-loader";
  }

  get status(): number {
    return this._status;
  }

  get needStashBuffer(): boolean {
    return this._needStash;
  }

  isWorking(): boolean {
    return this._status === mpegts.LoaderStatus.kConnecting ||
      this._status === mpegts.LoaderStatus.kBuffering;
  }

  destroy(): void {
    this.abort();
    this._status = mpegts.LoaderStatus.kIdle;
  }

  abort(): void {
    this.abortController?.abort();
    this.abortController = null;
  }

  open(dataSource: LoaderDataSource, range: LoaderRange): void {
    this.range = range;
    this.receivedLength = 0;
    this.abortController = new AbortController();
    this._status = mpegts.LoaderStatus.kConnecting;

    const initialUrl = this.config.reuseRedirectedURL && dataSource.redirectedURL
      ? dataSource.redirectedURL
      : dataSource.url;
    const seek = this.seekHandler.getConfig(initialUrl, range);
    const headers = new Headers(seek.headers);
    for (const [key, value] of Object.entries(this.config.headers ?? {})) headers.set(key, value);

    void this.pump(seek.url, headers, this.abortController.signal);
  }

  private async pump(url: string, headers: Headers, signal: AbortSignal): Promise<void> {
    try {
      const response = await tauriFetch(url, { method: "GET", headers, signal });
      if (!response.ok) {
        this.fail(mpegts.LoaderErrors.HTTP_STATUS_CODE_INVALID, response.status, response.statusText);
        return;
      }

      if (response.url && response.url !== url) {
        this.onURLRedirect?.(this.seekHandler.removeURLParameters(response.url));
      }
      const length = Number(response.headers.get("content-length"));
      if (Number.isFinite(length) && length > 0) this.onContentLengthKnown?.(length);
      if (!response.body) {
        this.fail(mpegts.LoaderErrors.EXCEPTION, -1, "native HTTP response has no body");
        return;
      }

      const reader = response.body.getReader();
      while (!signal.aborted) {
        const result = await reader.read();
        if (result.done) break;
        const value = result.value;
        const chunk = value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength);
        const byteStart = this.range.from + this.receivedLength;
        this.receivedLength += value.byteLength;
        this._status = mpegts.LoaderStatus.kBuffering;
        byteObserver(value.byteLength);
        this.onDataArrival?.(chunk, byteStart, this.receivedLength);
      }

      if (!signal.aborted) {
        this._status = mpegts.LoaderStatus.kComplete;
        this.onComplete?.(this.range.from, this.range.from + this.receivedLength - 1);
      }
    } catch (error) {
      if (signal.aborted) return;
      const message = error instanceof Error ? error.message : String(error);
      this.fail(mpegts.LoaderErrors.EXCEPTION, -1, message);
    }
  }

  private fail(type: string, code: number, msg: string): void {
    this._status = mpegts.LoaderStatus.kError;
    this.onError?.(type, { code, msg });
  }
}
