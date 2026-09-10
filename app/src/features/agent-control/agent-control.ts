import { z } from "zod";
import type {
  ChannelSummary,
  ClientResult,
  Page,
} from "../../client/contracts";
import {
  canonicalSearchTerm,
  searchTermFits,
} from "../guide/board-search-term";

const AGENT_CHANNEL_LIMIT = 40;

/** One Channel name/group pair returned by Agent Control search. */
export interface AgentControlChannelHit {
  readonly name: string;
  readonly group: string;
}

/** Parsed Agent Control command after the Unix-socket envelope is stripped. */
export type AgentControlRequest =
  | { readonly _tag: "ping" }
  | { readonly _tag: "search"; readonly term: string }
  | { readonly _tag: "tune"; readonly term: string }
  | { readonly _tag: "snapshot" }
  | { readonly _tag: "stop" };

/** Successful Agent Control result visible to the local CLI. */
export type AgentControlSuccess =
  | { readonly _tag: "pong"; readonly ready: boolean }
  | {
      readonly _tag: "channels";
      readonly channels: readonly AgentControlChannelHit[];
    }
  | { readonly _tag: "tuned"; readonly name: string }
  | {
      readonly _tag: "snapshot";
      readonly channelName: string | null;
      readonly diagnostics: unknown;
    }
  | { readonly _tag: "stopped" };

/** Expected Agent Control failure. Never carries provider or token data. */
export type AgentControlFailure =
  | { readonly _tag: "invalid-request" }
  | { readonly _tag: "unavailable" }
  | { readonly _tag: "catalog-not-ready" }
  | { readonly _tag: "search-incomplete" }
  | { readonly _tag: "timeout" }
  | { readonly _tag: "cleanup-failed" }
  | { readonly _tag: "channel-not-found" }
  | { readonly _tag: "ambiguous-channel"; readonly names: readonly string[] };

/** Envelope returned to Rust after executing one Agent Control command. */
export type AgentControlResponse =
  | { readonly ok: true; readonly result: AgentControlSuccess }
  | { readonly ok: false; readonly error: AgentControlFailure };

/** Catalog operations Agent Control needs from the installed browser. */
export interface AgentControlCatalog {
  readonly searchChannels: (
    term: string,
    signal?: AbortSignal,
  ) => Promise<ClientResult<Page<ChannelSummary>>>;
  readonly tune: (channel: ChannelSummary) => void;
  /** Clears selection intent even when the lazy player has not mounted. */
  readonly cancelPendingTune: () => void;
}

/** Playback operations Agent Control needs from the mounted installed player. */
export interface AgentControlPlayback {
  readonly channelName: string;
  readonly diagnostics: () => string;
  readonly stop: (signal?: AbortSignal) => Promise<boolean>;
}

const requestSchema: z.ZodType<AgentControlRequest> = z.discriminatedUnion(
  "_tag",
  [
    z.strictObject({ _tag: z.literal("ping") }),
    z.strictObject({ _tag: z.literal("search"), term: z.string() }),
    z.strictObject({ _tag: z.literal("tune"), term: z.string() }),
    z.strictObject({ _tag: z.literal("snapshot") }),
    z.strictObject({ _tag: z.literal("stop") }),
  ],
);

/**
 * Refines unknown socket JSON into one Agent Control command. Oversized or
 * empty search terms fail as invalid-request rather than reaching the catalog.
 */
export function parseAgentControlRequest(
  input: unknown,
):
  | { readonly ok: true; readonly request: AgentControlRequest }
  | { readonly ok: false; readonly error: AgentControlFailure } {
  const parsed = requestSchema.safeParse(input);
  if (!parsed.success) {
    return { ok: false, error: { _tag: "invalid-request" } };
  }
  if (parsed.data._tag === "search" || parsed.data._tag === "tune") {
    if (
      !searchTermFits(parsed.data.term) ||
      canonicalSearchTerm(parsed.data.term).length === 0
    ) {
      return { ok: false, error: { _tag: "invalid-request" } };
    }
  }
  return { ok: true, request: parsed.data };
}

/**
 * Executes one Agent Control command against the currently bound catalog and
 * player. The owner checks cancellation/deadlines again before committing a
 * tune; missing bindings become unavailable or an idle snapshot.
 */
export async function executeAgentControl(
  request: AgentControlRequest,
  catalog: AgentControlCatalog | null,
  playback: AgentControlPlayback | null,
  signal?: AbortSignal,
  cancelled: () => boolean = () => signal?.aborted === true,
): Promise<AgentControlResponse> {
  if (cancelled()) return { ok: false, error: { _tag: "timeout" } };
  switch (request._tag) {
    case "ping":
      return {
        ok: true,
        result: { _tag: "pong", ready: catalog !== null },
      };
    case "search":
      return searchChannels(request.term, catalog, signal, cancelled);
    case "tune":
      return tuneChannel(request.term, catalog, signal, cancelled);
    case "snapshot":
      return snapshot(playback);
    case "stop":
      return stopPlayback(catalog, playback, signal, cancelled);
  }
}

/** Channel page size used by Agent Control search and tune. */
export function agentControlChannelLimit(): number {
  return AGENT_CHANNEL_LIMIT;
}

/**
 * Picks a single Channel from search hits. Exact canonical name wins; otherwise
 * a unique substring match; otherwise none or ambiguous.
 */
export function selectAgentControlChannel(
  term: string,
  channels: readonly ChannelSummary[],
):
  | { readonly _tag: "selected"; readonly channel: ChannelSummary }
  | { readonly _tag: "none" }
  | { readonly _tag: "ambiguous"; readonly names: readonly string[] } {
  const needle = canonicalSearchTerm(term);
  const exact = channels.filter(
    (channel) => canonicalSearchTerm(channel.name) === needle,
  );
  const exactMatch = exact[0];
  if (exact.length === 1 && exactMatch !== undefined) {
    return { _tag: "selected", channel: exactMatch };
  }
  if (exact.length > 1) {
    return { _tag: "ambiguous", names: uniqueNames(exact) };
  }
  const partial = channels.filter((channel) =>
    canonicalSearchTerm(channel.name).includes(needle),
  );
  const partialMatch = partial[0];
  if (partial.length === 1 && partialMatch !== undefined) {
    return { _tag: "selected", channel: partialMatch };
  }
  if (partial.length > 1) {
    return { _tag: "ambiguous", names: uniqueNames(partial) };
  }
  return { _tag: "none" };
}

async function searchChannels(
  term: string,
  catalog: AgentControlCatalog | null,
  signal: AbortSignal | undefined,
  cancelled: () => boolean,
): Promise<AgentControlResponse> {
  if (catalog === null) {
    return { ok: false, error: { _tag: "unavailable" } };
  }
  const result = await catalog.searchChannels(term, signal);
  if (cancelled()) return { ok: false, error: { _tag: "timeout" } };
  if (!result.ok) {
    return { ok: false, error: { _tag: "catalog-not-ready" } };
  }
  return {
    ok: true,
    result: {
      _tag: "channels",
      channels: result.value.items.map((channel) => ({
        name: channel.name,
        group: channel.group,
      })),
    },
  };
}

async function tuneChannel(
  term: string,
  catalog: AgentControlCatalog | null,
  signal: AbortSignal | undefined,
  cancelled: () => boolean,
): Promise<AgentControlResponse> {
  if (catalog === null) {
    return { ok: false, error: { _tag: "unavailable" } };
  }
  const result = await catalog.searchChannels(term, signal);
  if (cancelled()) return { ok: false, error: { _tag: "timeout" } };
  if (!result.ok) {
    return { ok: false, error: { _tag: "catalog-not-ready" } };
  }
  // A later page may contain a duplicate exact name. Never infer uniqueness
  // from a truncated search, even when this page has just one exact match.
  if (result.value.next !== null) {
    return { ok: false, error: { _tag: "search-incomplete" } };
  }
  const selection = selectAgentControlChannel(term, result.value.items);
  if (selection._tag === "none") {
    return { ok: false, error: { _tag: "channel-not-found" } };
  }
  if (selection._tag === "ambiguous") {
    return {
      ok: false,
      error: { _tag: "ambiguous-channel", names: selection.names },
    };
  }
  catalog.tune(selection.channel);
  return {
    ok: true,
    result: { _tag: "tuned", name: selection.channel.name },
  };
}

function snapshot(playback: AgentControlPlayback | null): AgentControlResponse {
  if (playback === null) {
    return {
      ok: true,
      result: { _tag: "snapshot", channelName: null, diagnostics: null },
    };
  }
  return {
    ok: true,
    result: {
      _tag: "snapshot",
      channelName: playback.channelName,
      diagnostics: parseDiagnostics(playback.diagnostics()),
    },
  };
}

async function stopPlayback(
  catalog: AgentControlCatalog | null,
  playback: AgentControlPlayback | null,
  signal: AbortSignal | undefined,
  cancelled: () => boolean,
): Promise<AgentControlResponse> {
  if (playback === null) {
    catalog?.cancelPendingTune();
    return { ok: true, result: { _tag: "stopped" } };
  }
  const confirmed = await playback.stop(signal);
  if (!confirmed) return { ok: false, error: { _tag: "cleanup-failed" } };
  if (cancelled()) return { ok: false, error: { _tag: "timeout" } };
  catalog?.cancelPendingTune();
  return { ok: true, result: { _tag: "stopped" } };
}

function parseDiagnostics(raw: string): unknown {
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return null;
  }
}

function uniqueNames(channels: readonly ChannelSummary[]): readonly string[] {
  return [...new Set(channels.map((channel) => channel.name))];
}
