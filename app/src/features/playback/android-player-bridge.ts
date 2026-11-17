import { useSyncExternalStore } from "react";
import { z } from "zod";
import type { InstalledPlaybackState } from "./installed-playback-state";

/** What the page tells the Android window about its Playback Session. */
export type AndroidPlayerPlayback = "playing" | "paused" | "none";

/** The Android window around the player, as the page sees it. */
export interface AndroidPlayerWindow {
  /** The device can shrink the player into a picture-in-picture window. */
  readonly pictureInPictureSupported: boolean;
  /** The app is showing as a picture-in-picture window now. */
  readonly pictureInPicture: boolean;
}

/** Something the Android window asks of the page's player. */
export type AndroidPlayerWindowEvent =
  | { readonly _tag: "command"; readonly command: "play" | "pause" | "toggle" }
  | { readonly _tag: "television-channel"; readonly direction: -1 | 1 }
  | { readonly _tag: "picture-in-picture-rejected" };

/**
 * The page's line to the Android window that holds the native player. It
 * exists only in the Android app, where that window, not the document's
 * visibility, says whether the picture can be seen.
 */
export interface AndroidPlayerBridge {
  readonly getSnapshot: () => AndroidPlayerWindow;
  readonly subscribe: (listener: () => void) => () => void;
  readonly subscribeEvents: (
    listener: (event: AndroidPlayerWindowEvent) => void,
  ) => () => void;
  /** Gives the TV shell first refusal of Android Back. */
  readonly subscribeTelevisionBack: (listener: () => void) => () => void;
  readonly reportPlayback: (playback: AndroidPlayerPlayback) => void;
  readonly enterPictureInPicture: () => void;
  /** Returns an unhandled TV Back press to Android. */
  readonly defaultTelevisionBack: () => void;
}

/** The object Android injects into the page (a WebMessageListener). */
export interface AndroidPlayerPort {
  readonly postMessage: (message: string) => void;
  readonly addEventListener: (
    type: "message",
    listener: (event: { readonly data: unknown }) => void,
  ) => void;
}

const windowMessageSchema = z.enum([
  "capabilities:picture-in-picture",
  "capabilities:none",
  "picture-in-picture:on",
  "picture-in-picture:off",
  "picture-in-picture:rejected",
  "command:play",
  "command:pause",
  "command:toggle",
  "tv:back",
  "tv:channel-up",
  "tv:channel-down",
]);

const NO_WINDOW: AndroidPlayerWindow = Object.freeze({
  pictureInPictureSupported: false,
  pictureInPicture: false,
});

/** Builds the bridge over an injected port, or null where there is none. */
export function createAndroidPlayerBridge(
  port: unknown,
): AndroidPlayerBridge | null {
  if (!isAndroidPlayerPort(port)) {
    return null;
  }
  let snapshot = NO_WINDOW;
  let reported: AndroidPlayerPlayback | null = null;
  let onTelevisionBack: (() => void) | null = null;
  const listeners = new Set<() => void>();
  const eventListeners = new Set<(event: AndroidPlayerWindowEvent) => void>();
  const update = (next: Partial<AndroidPlayerWindow>) => {
    snapshot = Object.freeze({ ...snapshot, ...next });
    for (const listener of listeners) listener();
  };
  const emit = (event: AndroidPlayerWindowEvent) => {
    for (const listener of eventListeners) listener(event);
  };
  port.addEventListener("message", (event) => {
    const parsed = windowMessageSchema.safeParse(event.data);
    if (!parsed.success) {
      return;
    }
    switch (parsed.data) {
      case "capabilities:picture-in-picture":
        return update({ pictureInPictureSupported: true });
      case "capabilities:none":
        return update({ pictureInPictureSupported: false });
      case "picture-in-picture:on":
        return update({ pictureInPicture: true });
      case "picture-in-picture:off":
        return update({ pictureInPicture: false });
      case "picture-in-picture:rejected":
        return emit({ _tag: "picture-in-picture-rejected" });
      case "command:play":
        return emit({ _tag: "command", command: "play" });
      case "command:pause":
        return emit({ _tag: "command", command: "pause" });
      case "command:toggle":
        return emit({ _tag: "command", command: "toggle" });
      case "tv:back":
        // The shell may still be loading when Android first sends Back.
        if (onTelevisionBack === null) port.postMessage("tv:default-back");
        else onTelevisionBack();
        return;
      case "tv:channel-up":
        return emit({ _tag: "television-channel", direction: 1 });
      case "tv:channel-down":
        return emit({ _tag: "television-channel", direction: -1 });
    }
  });
  port.postMessage("ready");
  return {
    getSnapshot: () => snapshot,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    subscribeEvents: (listener) => {
      eventListeners.add(listener);
      return () => eventListeners.delete(listener);
    },
    subscribeTelevisionBack: (listener) => {
      onTelevisionBack = listener;
      return () => {
        if (onTelevisionBack === listener) onTelevisionBack = null;
      };
    },
    reportPlayback: (playback) => {
      if (playback !== reported) {
        reported = playback;
        port.postMessage(`playback:${playback}`);
      }
    },
    enterPictureInPicture: () => port.postMessage("enter-picture-in-picture"),
    defaultTelevisionBack: () => port.postMessage("tv:default-back"),
  };
}

function isAndroidPlayerPort(value: unknown): value is AndroidPlayerPort {
  return (
    typeof value === "object" &&
    value !== null &&
    "postMessage" in value &&
    typeof value.postMessage === "function" &&
    "addEventListener" in value &&
    typeof value.addEventListener === "function"
  );
}

/** The Android app's bridge; null in every other target. */
export const androidPlayerBridge: AndroidPlayerBridge | null =
  createAndroidPlayerBridge(
    typeof window === "undefined"
      ? undefined
      : (window as { sparrowAndroidPlayer?: unknown }).sparrowAndroidPlayer,
  );

const subscribeNothing = () => () => undefined;
const noWindow = () => NO_WINDOW;

/** The Android window's state; never picture-in-picture without a bridge. */
export function useAndroidPlayerWindow(
  bridge: AndroidPlayerBridge | null = androidPlayerBridge,
): AndroidPlayerWindow {
  return useSyncExternalStore(
    bridge?.subscribe ?? subscribeNothing,
    bridge?.getSnapshot ?? noWindow,
    bridge?.getSnapshot ?? noWindow,
  );
}

/**
 * What a native Android picture can do in a picture-in-picture window:
 * keep playing, offer Play (Resume, or Restart after a failure that allows
 * it), or nothing. A picture the page draws itself never shrinks.
 */
export function androidPlayerPlayback(
  state: Pick<InstalledPlaybackState, "phase" | "presentation">,
): AndroidPlayerPlayback {
  if (state.presentation !== "android-media3") {
    return "none";
  }
  switch (state.phase._tag) {
    case "starting":
    case "playing":
    case "recovering":
    case "replacing-audio":
      return "playing";
    case "paused":
      return "paused";
    case "failed":
      return state.phase.canRestart ? "paused" : "none";
    case "idle":
    case "autoplay-blocked":
    case "suspending":
    case "stopping":
      return "none";
  }
}
