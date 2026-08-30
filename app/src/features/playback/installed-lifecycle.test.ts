import { describe, expect, it, vi } from "vitest";
import {
  createInstalledLifecycleEvents,
  type InstalledLifecycleSignal,
} from "./installed-lifecycle";

describe("installed lifecycle events", () => {
  it("accepts only fresh closed events and releases the native listener once", async () => {
    let nativeListener:
      ((event: { readonly payload: unknown }) => void) | null = null;
    const releaseNative = vi.fn();
    const listenNative = vi.fn(
      async (
        event: string,
        listener: (event: { readonly payload: unknown }) => void,
      ) => {
        expect(event).toBe("sparrow://playback-lifecycle");
        nativeListener = listener;
        return releaseNative;
      },
    );
    const received: InstalledLifecycleSignal[] = [];
    const release = await createInstalledLifecycleEvents(
      listenNative,
    ).subscribe((signal) => received.push(signal));
    const emit = (payload: unknown) => nativeListener?.({ payload });

    emit({ revision: 1, state: "suspended", privateValue: "canary" });
    emit({ revision: 0, state: "suspended" });
    emit({ revision: 1, state: "suspended" });
    emit({ revision: 1, state: "resumed" });
    emit({ revision: 2, state: "resumed" });
    emit({ revision: 3, state: "unknown" });
    expect(received).toEqual(["suspended", "resumed"]);

    release();
    release();
    emit({ revision: 3, state: "suspended" });
    expect(received).toEqual(["suspended", "resumed"]);
    expect(releaseNative).toHaveBeenCalledTimes(1);
  });
});
