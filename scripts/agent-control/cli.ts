#!/usr/bin/env bun
/**
 * Local CLI for Sparrow Agent Control. Talks JSON-over-Unix-socket to a running
 * installed app that was started with SPARROW_AGENT_SOCKET and SPARROW_AGENT_TOKEN.
 *
 * Usage:
 *   just agent ping
 *   just agent search "Eurosport"
 *   just agent tune "Eurosport 1 FHD SE"
 *   just agent snapshot
 *   just agent wait --phase playing --timeout-ms 45000
 *   just agent wait --media-advancing --timeout-ms 45000
 *   just agent stop
 */
import { connect } from "node:net";
import { env } from "node:process";
import {
  agentMediaAdvanced,
  parseAgentWireResponse,
  type AgentSnapshot,
  type AgentWireResponse,
} from "../../app/src/features/agent-control/agent-control-response";

const USAGE = `Drive a running installed Sparrow over Agent Control.

Commands:
  ping
  search <term>
  tune <term>
  snapshot
  stop
  wait [--phase <tag>] [--media-advancing] [--timeout-ms <n>] [--interval-ms <n>]

Environment:
  SPARROW_AGENT_SOCKET   Absolute socket under $XDG_RUNTIME_DIR or /tmp
  SPARROW_AGENT_TOKEN    16–128 printable ASCII characters

Start the app with the same two variables, then run this CLI.`;

type AgentRequest =
  | { readonly _tag: "ping" }
  | { readonly _tag: "search"; readonly term: string }
  | { readonly _tag: "tune"; readonly term: string }
  | { readonly _tag: "snapshot" }
  | { readonly _tag: "stop" };

interface WaitOptions {
  readonly phase: string | null;
  readonly mediaAdvancing: boolean;
  readonly timeoutMs: number;
  readonly intervalMs: number;
}

async function main(argv: readonly string[]): Promise<number> {
  if (argv.length === 0 || argv[0] === "--help" || argv[0] === "-h") {
    console.error(USAGE);
    return argv.length === 0 ? 1 : 0;
  }
  const command = argv[0];
  if (command === "wait") {
    return waitCommand(parseWait(argv.slice(1)));
  }
  const request = parseCommand(argv);
  if (request === null) {
    console.error(USAGE);
    return 1;
  }
  const response = await rpc(request);
  printJson(response);
  return response.ok === true ? 0 : 2;
}

function parseCommand(argv: readonly string[]): AgentRequest | null {
  const command = argv[0];
  if (command === "ping" || command === "snapshot" || command === "stop") {
    if (argv.length !== 1) {
      return null;
    }
    return { _tag: command };
  }
  if (command === "search" || command === "tune") {
    const term = argv.slice(1).join(" ").trim();
    if (term.length === 0) {
      return null;
    }
    return { _tag: command, term };
  }
  return null;
}

function parseWait(argv: readonly string[]): WaitOptions {
  let phase: string | null = null;
  let mediaAdvancing = false;
  let timeoutMs = 30_000;
  let intervalMs = 500;
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const next = argv[index + 1];
    if (flag === "--phase" && next !== undefined) {
      phase = next;
      index += 1;
      continue;
    }
    if (flag === "--timeout-ms" && next !== undefined) {
      timeoutMs = Number(next);
      index += 1;
      continue;
    }
    if (flag === "--interval-ms" && next !== undefined) {
      intervalMs = Number(next);
      index += 1;
      continue;
    }
    if (flag === "--media-advancing") {
      mediaAdvancing = true;
      continue;
    }
    throw new Error("invalid wait options");
  }
  if (phase === null && !mediaAdvancing) {
    throw new Error("wait requires --phase and/or --media-advancing");
  }
  if (!Number.isFinite(timeoutMs) || timeoutMs < 1 || timeoutMs > 900_000) {
    throw new Error("timeout-ms must be between 1 and 900000");
  }
  if (!Number.isFinite(intervalMs) || intervalMs < 50 || intervalMs > 10_000) {
    throw new Error("interval-ms must be between 50 and 10000");
  }
  return { phase, mediaAdvancing, timeoutMs, intervalMs };
}

async function waitCommand(options: WaitOptions): Promise<number> {
  const deadline = performance.now() + options.timeoutMs;
  let last: AgentWireResponse | null = null;
  let previous: AgentSnapshot | null = null;
  while (performance.now() < deadline) {
    let response: AgentWireResponse;
    try {
      response = await rpc({ _tag: "snapshot" }, deadline);
    } catch (error) {
      if (error instanceof RpcFailure && error.tag === "timeout") break;
      throw error;
    }
    last = response;
    if (performance.now() >= deadline) break;
    if (!response.ok) {
      printJson(response);
      return 2;
    }
    if (response.result._tag !== "snapshot")
      throw new RpcFailure("invalid-response");
    const current = response.result;
    if (
      (options.phase === null ||
        current.diagnostics?.phase === options.phase) &&
      (!options.mediaAdvancing || agentMediaAdvanced(previous, current))
    ) {
      printJson(response);
      return 0;
    }
    previous = current;
    await sleep(
      Math.min(options.intervalMs, Math.max(0, deadline - performance.now())),
    );
  }
  printJson({
    ok: false,
    error: { _tag: "timeout" },
    last,
  });
  return 3;
}

class RpcFailure extends Error {
  constructor(
    readonly tag: "timeout" | "closed" | "transport" | "invalid-response",
  ) {
    super("Agent Control request failed");
  }
}

function rpc(
  request: AgentRequest,
  deadline = performance.now() + 20_000,
): Promise<AgentWireResponse> {
  const socket = env.SPARROW_AGENT_SOCKET;
  const token = env.SPARROW_AGENT_TOKEN;
  if (socket === undefined || token === undefined) {
    throw new Error(
      "SPARROW_AGENT_SOCKET and SPARROW_AGENT_TOKEN are required",
    );
  }
  const payload = `${JSON.stringify({ token, request })}\n`;
  return new Promise((resolve, reject) => {
    const connection = connect(socket);
    const buffer = Buffer.alloc(256 * 1024);
    let length = 0;
    let settled = false;
    const finish = (result: AgentWireResponse | RpcFailure) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      connection.destroy();
      if (result instanceof RpcFailure) reject(result);
      else resolve(result);
    };
    const timer = setTimeout(
      () => finish(new RpcFailure("timeout")),
      Math.max(0, deadline - performance.now()),
    );
    connection.once("connect", () => {
      connection.write(payload, (error) => {
        if (error) finish(new RpcFailure("transport"));
      });
    });
    connection.on("data", (chunk: Buffer) => {
      if (settled) return;
      if (performance.now() >= deadline)
        return finish(new RpcFailure("timeout"));
      const newline = chunk.indexOf(10);
      const size = newline === -1 ? chunk.length : newline;
      if (length + size > buffer.length)
        return finish(new RpcFailure("invalid-response"));
      chunk.copy(buffer, length, 0, size);
      length += size;
      if (newline === -1) return;
      try {
        const decoded: unknown = JSON.parse(buffer.toString("utf8", 0, length));
        const parsed = parseAgentWireResponse(decoded);
        const expected = {
          ping: "pong",
          search: "channels",
          tune: "tuned",
          snapshot: "snapshot",
          stop: "stopped",
        };
        if (
          parsed === null ||
          (parsed.ok && parsed.result._tag !== expected[request._tag])
        ) {
          finish(new RpcFailure("invalid-response"));
        } else finish(parsed);
      } catch {
        finish(new RpcFailure("invalid-response"));
      }
    });
    connection.once("end", () => finish(new RpcFailure("closed")));
    connection.once("close", () => finish(new RpcFailure("closed")));
    connection.once("error", () => finish(new RpcFailure("transport")));
  });
}

function printJson(value: unknown): void {
  console.log(JSON.stringify(value, null, 2));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

try {
  const code = await main(process.argv.slice(2));
  process.exit(code);
} catch {
  // Socket/JSON/usage exceptions can contain paths, tokens or source data.
  console.error(JSON.stringify({ ok: false, error: { _tag: "cli" } }));
  process.exit(1);
}
