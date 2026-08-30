// @vitest-environment node

import { describe, expect, it, vi } from "vitest";
import { createHttpSparrowClient } from "./http";
import { createNativeSparrowClient, type NativeIpc } from "./native";
import { createSparrowRuntime, isInstalledPlatform } from "./runtime";

describe("Sparrow runtime composition", () => {
  it("selects the hosted HTTP client without loading native code", async () => {
    const hostedClient = createHttpSparrowClient({
      fetch: () => Promise.reject(new Error("not called")),
    });
    const createHostedClient = vi.fn(() => hostedClient);
    const loadInstalledClient = vi.fn(() =>
      Promise.resolve(createNativeSparrowClient({ ipc: unusedIpc() })),
    );

    const runtime = await createSparrowRuntime({
      platform: {},
      createHostedClient,
      loadInstalledClient,
    });

    expect(runtime).toEqual({ _tag: "hosted", client: hostedClient });
    expect(createHostedClient).toHaveBeenCalledOnce();
    expect(loadInstalledClient).not.toHaveBeenCalled();
  });

  it("selects only the native IPC client inside a Tauri webview", async () => {
    const installedClient = createNativeSparrowClient({ ipc: unusedIpc() });
    const createHostedClient = vi.fn(() =>
      createHttpSparrowClient({
        fetch: () => Promise.reject(new Error("not called")),
      }),
    );
    const loadInstalledClient = vi.fn(() => Promise.resolve(installedClient));

    const runtime = await createSparrowRuntime({
      platform: { __TAURI_INTERNALS__: Object.freeze({}) },
      createHostedClient,
      loadInstalledClient,
    });

    expect(runtime).toEqual({ _tag: "installed", client: installedClient });
    expect(loadInstalledClient).toHaveBeenCalledOnce();
    expect(createHostedClient).not.toHaveBeenCalled();
  });

  it("requires an object carrying Tauri's own runtime marker", () => {
    expect(isInstalledPlatform(null)).toBe(false);
    expect(isInstalledPlatform("__TAURI_INTERNALS__")).toBe(false);
    expect(isInstalledPlatform({})).toBe(false);
    expect(isInstalledPlatform({ __TAURI_INTERNALS__: undefined })).toBe(true);
  });
});

function unusedIpc(): NativeIpc {
  return {
    invoke: () => Promise.reject(new Error("not called")),
    createChannel: (onmessage) => ({ onmessage }),
  };
}
