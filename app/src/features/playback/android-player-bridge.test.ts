import { describe, expect, it, vi } from "vitest";
import {
  androidPlayerPlayback,
  createAndroidPlayerBridge,
  type AndroidPlayerWindowEvent,
} from "./android-player-bridge";
import { androidPortFixture } from "../../test/android-port-fixture";

describe("Android player bridge", () => {
  it("exists only where Android injected its port", () => {
    expect(createAndroidPlayerBridge(undefined)).toBeNull();
    expect(createAndroidPlayerBridge({ postMessage: "nope" })).toBeNull();
    expect(createAndroidPlayerBridge(androidPortFixture().value)).not.toBeNull();
  });

  it("asks for the window's state, then follows only its closed messages", () => {
    const port = androidPortFixture();
    const bridge = createAndroidPlayerBridge(port.value);
    if (bridge === null) throw new Error("bridge expected");
    expect(port.sent).toEqual(["ready"]);
    const events: AndroidPlayerWindowEvent[] = [];
    bridge.subscribeEvents((event) => events.push(event));
    const changed = vi.fn();
    bridge.subscribe(changed);

    port.deliver("capabilities:picture-in-picture");
    port.deliver("picture-in-picture:on");
    port.deliver("picture-in-picture:sideways");
    port.deliver({ type: "command", command: "play" });
    port.deliver("command:pause");
    port.deliver("picture-in-picture:rejected");

    expect(bridge.getSnapshot()).toEqual({
      pictureInPictureSupported: true,
      pictureInPicture: true,
    });
    expect(changed).toHaveBeenCalledTimes(2);
    expect(events).toEqual([
      { _tag: "command", command: "pause" },
      { _tag: "picture-in-picture-rejected" },
    ]);
  });

  it("tells the window only what changed", () => {
    const port = androidPortFixture();
    const bridge = createAndroidPlayerBridge(port.value);
    bridge?.reportPlayback("playing");
    bridge?.reportPlayback("playing");
    bridge?.reportPlayback("paused");
    bridge?.enterPictureInPicture();
    expect(port.sent).toEqual([
      "ready",
      "playback:playing",
      "playback:paused",
      "enter-picture-in-picture",
    ]);
  });

  it("offers the window only what a native picture can do", () => {
    const native = { presentation: "android-media3" } as const;
    expect(
      androidPlayerPlayback({
        ...native,
        phase: { _tag: "recovering", attempt: 1, retryAt: 0, failure: "stream-interrupted" },
      }),
    ).toBe("playing");
    expect(
      androidPlayerPlayback({
        ...native,
        phase: { _tag: "paused", cause: "user", resumeWhenVisible: false },
      }),
    ).toBe("paused");
    expect(
      androidPlayerPlayback({
        ...native,
        phase: { _tag: "failed", failure: "stream-interrupted", attemptsUsed: 3, canRestart: true },
      }),
    ).toBe("paused");
    expect(
      androidPlayerPlayback({
        ...native,
        phase: {
          _tag: "failed",
          failure: "cleanup-unconfirmed",
          attemptsUsed: 0,
          canRestart: false,
        },
      }),
    ).toBe("none");
    expect(
      androidPlayerPlayback({
        presentation: "webview-mse",
        phase: { _tag: "playing", stableSince: 0 },
      }),
    ).toBe("none");
  });
});
