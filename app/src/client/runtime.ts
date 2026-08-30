import type { InstalledSparrowClient, SparrowClient } from "./contracts";
import { createHttpSparrowClient } from "./http";

/** The hosted browser composition and its authenticated same-origin client. */
export interface HostedRuntime {
  readonly _tag: "hosted";
  readonly client: SparrowClient;
}

/** The installed composition and its local Tauri IPC client. */
export interface InstalledRuntime {
  readonly _tag: "installed";
  readonly client: InstalledSparrowClient;
}

/** Exactly one transport composition selected before React mounts. */
export type SparrowRuntime = HostedRuntime | InstalledRuntime;

/** Injectable composition dependencies used to verify runtime selection. */
export interface SparrowRuntimeOptions {
  readonly platform?: unknown;
  readonly createHostedClient?: () => SparrowClient;
  readonly loadInstalledClient?: () => Promise<InstalledSparrowClient>;
}

/**
 * Selects one runtime transport. The native adapter is dynamically loaded only
 * inside a Tauri webview, keeping hosted startup behavior and requests unchanged.
 */
export async function createSparrowRuntime(
  options: SparrowRuntimeOptions = {},
): Promise<SparrowRuntime> {
  if (!isInstalledPlatform(options.platform ?? globalThis)) {
    return {
      _tag: "hosted",
      client: (options.createHostedClient ?? createHttpSparrowClient)(),
    };
  }

  const loadInstalledClient =
    options.loadInstalledClient ?? defaultInstalledClient;
  return {
    _tag: "installed",
    client: await loadInstalledClient(),
  };
}

/** Returns whether a platform exposes Tauri's own private IPC bootstrap marker. */
export function isInstalledPlatform(platform: unknown): boolean {
  return (
    typeof platform === "object" &&
    platform !== null &&
    "__TAURI_INTERNALS__" in platform
  );
}

async function defaultInstalledClient(): Promise<InstalledSparrowClient> {
  const native = await import("./native");
  return native.createNativeSparrowClient();
}
