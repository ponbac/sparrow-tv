import { afterEach, describe, expect, it } from "vitest";
import { clientSchemas, type ChannelSummary } from "../../client/contracts";
import {
  executeAgentControl,
  parseAgentControlRequest,
  selectAgentControlChannel,
  type AgentControlCatalog,
  type AgentControlPlayback,
} from "./agent-control";

const EUROSPORT = clientSchemas.channel.parse({
  id: "eurosport-1-fhd-se",
  name: "Eurosport 1 FHD SE",
  group: "Sport",
});

const EUROSPORT_2 = clientSchemas.channel.parse({
  id: "eurosport-2-fhd-se",
  name: "Eurosport 2 FHD SE",
  group: "Sport",
});

describe("parseAgentControlRequest", () => {
  it("rejects unknown methods and empty search terms", () => {
    expect(parseAgentControlRequest({ _tag: "explode" })).toEqual({
      ok: false,
      error: { _tag: "invalid-request" },
    });
    expect(parseAgentControlRequest({ _tag: "tune", term: "   " })).toEqual({
      ok: false,
      error: { _tag: "invalid-request" },
    });
    expect(parseAgentControlRequest({ _tag: "ping" })).toEqual({
      ok: true,
      request: { _tag: "ping" },
    });
  });
});

describe("selectAgentControlChannel", () => {
  it("prefers an exact Channel name over a substring sibling", () => {
    expect(
      selectAgentControlChannel("Eurosport 1 FHD SE", [EUROSPORT, EUROSPORT_2]),
    ).toEqual({ _tag: "selected", channel: EUROSPORT });
  });

  it("reports ambiguous substring matches", () => {
    expect(
      selectAgentControlChannel("Eurosport", [EUROSPORT, EUROSPORT_2]),
    ).toEqual({
      _tag: "ambiguous",
      names: ["Eurosport 1 FHD SE", "Eurosport 2 FHD SE"],
    });
  });
});

describe("executeAgentControl", () => {
  afterEach(() => {
    recordedTunes.length = 0;
  });

  it("tunes the unique exact Channel and snapshots diagnostics", async () => {
    const catalog = catalogWith([EUROSPORT, EUROSPORT_2]);
    const playback = playbackWith("Eurosport 1 FHD SE", '{"phase":"playing"}');

    const tuned = await executeAgentControl(
      { _tag: "tune", term: "Eurosport 1 FHD SE" },
      catalog,
      null,
    );
    expect(tuned).toEqual({
      ok: true,
      result: { _tag: "tuned", name: "Eurosport 1 FHD SE" },
    });
    expect(recordedTunes).toEqual([EUROSPORT]);

    await expect(
      executeAgentControl({ _tag: "snapshot" }, catalog, playback),
    ).resolves.toEqual({
      ok: true,
      result: {
        _tag: "snapshot",
        channelName: "Eurosport 1 FHD SE",
        diagnostics: { phase: "playing" },
      },
    });
  });

  it("does not start playback when several Channels share the term", async () => {
    const catalog = catalogWith([EUROSPORT, EUROSPORT_2]);
    await expect(
      executeAgentControl({ _tag: "tune", term: "Eurosport" }, catalog, null),
    ).resolves.toEqual({
      ok: false,
      error: {
        _tag: "ambiguous-channel",
        names: ["Eurosport 1 FHD SE", "Eurosport 2 FHD SE"],
      },
    });
    expect(recordedTunes).toEqual([]);
  });

  it("fails closed when a later page could contain another exact name", async () => {
    const catalog: AgentControlCatalog = {
      ...catalogWith([EUROSPORT]),
      searchChannels: async () => ({
        ok: true,
        value: clientSchemas.channelsPage.parse({
          generation: 7,
          items: [EUROSPORT],
          next: "later-page",
        }),
      }),
    };
    await expect(
      executeAgentControl(
        { _tag: "tune", term: EUROSPORT.name },
        catalog,
        null,
      ),
    ).resolves.toEqual({ ok: false, error: { _tag: "search-incomplete" } });
    expect(recordedTunes).toEqual([]);
  });

  it("checks the owner's deadline again before committing tune, even before abort fires", async () => {
    let expired = false;
    const base = catalogWith([EUROSPORT]);
    const catalog: AgentControlCatalog = {
      ...base,
      searchChannels: async (term, signal) => {
        const result = await base.searchChannels(term, signal);
        expired = true;
        return result;
      },
    };
    const controller = new AbortController();
    await expect(
      executeAgentControl(
        { _tag: "tune", term: EUROSPORT.name },
        catalog,
        null,
        controller.signal,
        () => expired,
      ),
    ).resolves.toEqual({ ok: false, error: { _tag: "timeout" } });
    expect(controller.signal.aborted).toBe(false);
    expect(recordedTunes).toEqual([]);
  });

  it("reports cleanup failure instead of a successful stop", async () => {
    const playback = {
      ...playbackWith("Fixture", "{}"),
      stop: async () => false,
    };
    await expect(
      executeAgentControl({ _tag: "stop" }, null, playback),
    ).resolves.toEqual({ ok: false, error: { _tag: "cleanup-failed" } });
  });

  it("answers ping without a catalog and stop without a player", async () => {
    await expect(
      executeAgentControl({ _tag: "ping" }, null, null),
    ).resolves.toEqual({
      ok: true,
      result: { _tag: "pong", ready: false },
    });
    await expect(
      executeAgentControl({ _tag: "stop" }, null, null),
    ).resolves.toEqual({
      ok: true,
      result: { _tag: "stopped" },
    });
  });
});

const recordedTunes: ChannelSummary[] = [];

function catalogWith(items: readonly ChannelSummary[]): AgentControlCatalog {
  return {
    searchChannels: async () => ({
      ok: true,
      value: clientSchemas.channelsPage.parse({
        generation: 7,
        items: [...items],
        next: null,
      }),
    }),
    cancelPendingTune: () => undefined,
    tune: (channel) => {
      recordedTunes.push(channel);
    },
  };
}

function playbackWith(
  channelName: string,
  diagnostics: string,
): AgentControlPlayback {
  return {
    channelName,
    diagnostics: () => diagnostics,
    stop: async () => true,
  };
}
