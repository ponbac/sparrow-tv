import { afterEach, describe, expect, it, vi } from "vitest";
import { clientSchemas } from "../../client/contracts";
import type { AgentControlCatalog } from "./agent-control";
import {
  bindAgentControlCatalog,
  bindAgentControlPlayback,
  dispatchAgentControl,
  installAgentControlDispatch,
} from "./agent-control-binding";

const CHANNEL = clientSchemas.channel.parse({
  id: "world-news",
  name: "World News",
  group: "News",
});

describe("agent-control-binding", () => {
  afterEach(() => {
    unbind?.();
    unbind = null;
    uninstall?.();
    uninstall = null;
    tuned = false;
    vi.useRealTimers();
  });

  it("dispatches through the current catalog binding", async () => {
    unbind = bindAgentControlCatalog({
      searchChannels: async () => ({
        ok: true,
        value: clientSchemas.channelsPage.parse({
          generation: 1,
          items: [CHANNEL],
          next: null,
        }),
      }),
      cancelPendingTune: () => undefined,
      tune: () => {
        tuned = true;
      },
    });

    await expect(
      dispatchAgentControl({ _tag: "tune", term: "World News" }),
    ).resolves.toEqual({
      ok: true,
      result: { _tag: "tuned", name: "World News" },
    });
    expect(tuned).toBe(true);
  });

  it.each(["stop", "unbind", "deadline"])(
    "prevents late tune after %s, even for non-cooperative search",
    async (exit) => {
      vi.useFakeTimers();
      const search = deferredSearch();
      let cancelled = false;
      unbind = bindAgentControlCatalog({
        searchChannels: () => search.promise,
        tune: () => {
          tuned = true;
        },
        cancelPendingTune: () => {
          cancelled = true;
        },
      });
      const response = dispatchAgentControl(
        { _tag: "tune", term: "World News" },
        { timeoutMs: 100 },
      );
      if (exit === "stop") {
        await expect(dispatchAgentControl({ _tag: "stop" })).resolves.toEqual({
          ok: true,
          result: { _tag: "stopped" },
        });
        expect(cancelled).toBe(true);
      } else if (exit === "unbind") unbind();
      else await vi.advanceTimersByTimeAsync(100);
      await expect(response).resolves.toEqual({
        ok: false,
        error: { _tag: "timeout" },
      });
      search.resolve();
      await vi.advanceTimersByTimeAsync(0);
      expect(tuned).toBe(false);
    },
  );

  it("bounds in-flight work without growing a serialized queue", async () => {
    const search = deferredSearch();
    let started = 0;
    unbind = bindAgentControlCatalog({
      searchChannels: () => {
        started += 1;
        return search.promise;
      },
      tune: () => undefined,
      cancelPendingTune: () => undefined,
    });
    const requests = Array.from({ length: 8 }, () =>
      dispatchAgentControl({ _tag: "search", term: CHANNEL.name }),
    );
    try {
      await expect(
        dispatchAgentControl({ _tag: "search", term: CHANNEL.name }),
      ).resolves.toEqual({ ok: false, error: { _tag: "unavailable" } });
      expect(started).toBe(8);
    } finally {
      search.resolve();
      await Promise.all(requests);
    }
  });

  it("clears pending selection when no player has mounted", async () => {
    let selected = false;
    unbind = bindAgentControlCatalog({
      searchChannels: async () => channelPage(),
      tune: () => {
        selected = true;
      },
      cancelPendingTune: () => {
        selected = false;
      },
    });
    await dispatchAgentControl({ _tag: "tune", term: "World News" });
    expect(selected).toBe(true);
    await dispatchAgentControl({ _tag: "stop" });
    expect(selected).toBe(false);
  });

  it("keeps unconfirmed stop exclusive after its reply deadline", async () => {
    vi.useFakeTimers();
    let complete: (confirmed: boolean) => void = () => undefined;
    const stop = new Promise<boolean>((resolve) => {
      complete = resolve;
    });
    const release = bindAgentControlPlayback({
      channelName: CHANNEL.name,
      diagnostics: () => "{}",
      stop: () => stop,
    });
    try {
      const response = dispatchAgentControl(
        { _tag: "stop" },
        { timeoutMs: 100 },
      );
      await vi.advanceTimersByTimeAsync(100);
      await expect(response).resolves.toEqual({
        ok: false,
        error: { _tag: "timeout" },
      });
      await expect(
        dispatchAgentControl({ _tag: "tune", term: CHANNEL.name }),
      ).resolves.toEqual({ ok: false, error: { _tag: "unavailable" } });
    } finally {
      complete(false);
      await vi.advanceTimersByTimeAsync(0);
      release();
    }
  });

  it("expires bridge work before it can search or tune", async () => {
    vi.useFakeTimers();
    const replies: unknown[] = [];
    unbind = bindAgentControlCatalog({
      searchChannels: async () => channelPage(),
      tune: () => {
        tuned = true;
      },
      cancelPendingTune: () => undefined,
    });
    uninstall = installAgentControlDispatch(async (_id, body) => {
      replies.push(body);
    });
    // SAFETY: this is the private bridge installed immediately above.
    const target = globalThis as {
      __sparrowAgentControl?: (
        id: string,
        payload: unknown,
        expiresAt: number,
      ) => void;
    };
    target.__sparrowAgentControl?.(
      "expired",
      { _tag: "tune", term: CHANNEL.name },
      Date.now() - 1,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(replies).toEqual([{ ok: false, error: { _tag: "timeout" } }]);
    expect(tuned).toBe(false);
  });

  it.each(["cancel", "uninstall"])(
    "owns late work after bridge %s",
    async (exit) => {
      vi.useFakeTimers();
      const search = deferredSearch();
      const replies: unknown[] = [];
      unbind = bindAgentControlCatalog({
        searchChannels: () => search.promise,
        tune: () => {
          tuned = true;
        },
        cancelPendingTune: () => undefined,
      });
      uninstall = installAgentControlDispatch(async (_id, body) => {
        replies.push(body);
      });
      // SAFETY: these globals are the installed production bridge under test.
      const target = globalThis as {
        __sparrowAgentControl?: (id: string, payload: unknown) => void;
        __sparrowAgentControlCancel?: (id: string) => void;
      };
      target.__sparrowAgentControl?.("cancelled", {
        _tag: "tune",
        term: CHANNEL.name,
      });
      if (exit === "cancel") target.__sparrowAgentControlCancel?.("cancelled");
      else uninstall();
      search.resolve();
      await vi.advanceTimersByTimeAsync(0);
      expect(replies).toEqual([]);
      expect(tuned).toBe(false);
    },
  );

  it("replies over the installed dispatch without throwing", async () => {
    const replies: { id: string; body: unknown }[] = [];
    uninstall = installAgentControlDispatch(async (id, body) => {
      replies.push({ id, body });
    });
    const dispatch = (
      globalThis as {
        __sparrowAgentControl?: (id: string, payload: unknown) => void;
      }
    ).__sparrowAgentControl;
    if (dispatch === undefined) {
      throw new Error("expected the Agent Control dispatch to be installed");
    }
    dispatch("agnt1_test", { _tag: "ping" });
    await vi.waitFor(() => {
      expect(replies).toEqual([
        {
          id: "agnt1_test",
          body: { ok: true, result: { _tag: "pong", ready: false } },
        },
      ]);
    });
  });
});

function channelPage(): Awaited<
  ReturnType<AgentControlCatalog["searchChannels"]>
> {
  return {
    ok: true,
    value: clientSchemas.channelsPage.parse({
      generation: 1,
      items: [CHANNEL],
      next: null,
    }),
  };
}

function deferredSearch() {
  let complete: () => void = () => {
    throw new Error("search not initialized");
  };
  const promise = new Promise<
    Awaited<ReturnType<AgentControlCatalog["searchChannels"]>>
  >((resolve) => {
    complete = () => resolve(channelPage());
  });
  return { promise, resolve: () => complete() };
}

let unbind: (() => void) | null = null;
let uninstall: (() => void) | null = null;
let tuned = false;
