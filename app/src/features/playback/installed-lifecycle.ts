import { listen } from "@tauri-apps/api/event";
import { z } from "zod";

const installedLifecycleEventSchema = z.strictObject({
  revision: z.number().int().positive().max(Number.MAX_SAFE_INTEGER),
  state: z.enum(["suspended", "resumed"]),
});

/** Closed native application lifecycle signal consumed by installed playback. */
export type InstalledLifecycleSignal = "suspended" | "resumed";

/** Owned subscription seam for Tauri mobile lifecycle events. */
export interface InstalledLifecycleEvents {
  /** Registers one listener and returns an idempotent release for both events. */
  readonly subscribe: (
    listener: (signal: InstalledLifecycleSignal) => void,
  ) => Promise<() => void>;
}

/** Minimal native event listener used to verify lifecycle parsing and release. */
export type InstalledLifecycleListen = (
  event: string,
  listener: (event: { readonly payload: unknown }) => void,
) => Promise<() => void>;

/** Builds a lifecycle adapter that accepts only fresh, closed native events. */
export function createInstalledLifecycleEvents(
  listenNative: InstalledLifecycleListen,
): InstalledLifecycleEvents {
  return {
    subscribe: async (listener) => {
      let accepting = true;
      let lastRevision = 0;
      const releaseNative = await listenNative(
        "sparrow://playback-lifecycle",
        (event) => {
          if (!accepting) {
            return;
          }
          const parsed = installedLifecycleEventSchema.safeParse(event.payload);
          if (!parsed.success || parsed.data.revision <= lastRevision) {
            return;
          }
          lastRevision = parsed.data.revision;
          listener(parsed.data.state);
        },
      );
      let released = false;
      return () => {
        if (released) {
          return;
        }
        released = true;
        accepting = false;
        releaseNative();
      };
    },
  };
}

/** Tauri adapter for confirmed Android onPause/onResume ownership changes. */
export const tauriInstalledLifecycleEvents: InstalledLifecycleEvents =
  createInstalledLifecycleEvents((event, listener) => listen(event, listener));
