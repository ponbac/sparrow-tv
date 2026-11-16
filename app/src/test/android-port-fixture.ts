import type { AndroidPlayerPort } from "../features/playback/android-player-bridge";

/** Stands in for the port Android's WebView injects as `sparrowAndroidPlayer`. */
export function androidPortFixture(): {
  readonly value: AndroidPlayerPort;
  readonly sent: readonly string[];
  readonly deliver: (data: unknown) => void;
} {
  const sent: string[] = [];
  const listeners: ((event: { readonly data: unknown }) => void)[] = [];
  return {
    value: {
      postMessage: (message) => sent.push(message),
      addEventListener: (_type, listener) => listeners.push(listener),
    },
    sent,
    deliver: (data) => {
      for (const listener of listeners) listener({ data });
    },
  };
}
