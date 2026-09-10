import { describe, expect, test } from "bun:test";
import { createServer, type Socket } from "node:net";
import { mkdtemp, rm, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const cli = resolve(import.meta.dir, "cli.ts");
const token = "synthetic-test-token-only";

describe("Agent Control CLI real Unix sockets", () => {
  test.each(["", '{"ok":true'])(
    "rejects EOF without a framed response (%s)",
    async (body) => {
      const result = await withSocket((socket) => socket.end(body), ["ping"]);
      expect(result.code).toBe(1);
      expect(result.stderr).not.toContain(token);
    },
  );

  test.each([
    "null",
    '{"ok":true}',
    '{"ok":true,"result":{"_tag":"stopped"}}',
    '{"ok":false,"error":{"_tag":"secret-provider-url"}}',
  ])("rejects malformed or mismatched envelopes (%s)", async (body) => {
    const result = await withSocket(
      (socket) => socket.end(body + "\n"),
      ["ping"],
    );
    expect(result.code).toBe(1);
    expect(result.stderr).toBe('{"ok":false,"error":{"_tag":"cli"}}\n');
  });

  test("bounds unterminated responses", async () => {
    const result = await withSocket(
      (socket) => socket.write("x".repeat(300_000)),
      ["ping"],
    );
    expect(result.code).toBe(1);
  });

  test("validates split frames and strips arbitrary error fields", async () => {
    const result = await withSocket(
      (socket) => {
        socket.write('{"ok":false,');
        socket.end(
          JSON.stringify({
            error: { _tag: "unavailable", message: token },
          }).slice(1) + "\n",
        );
      },
      ["ping"],
    );
    expect(result.code).toBe(2);
    expect(JSON.parse(result.stdout)).toEqual({
      ok: false,
      error: { _tag: "unavailable" },
    });
    expect(result.stdout + result.stderr).not.toContain(token);
  });

  test("wait's absolute deadline includes an unresponsive RPC", async () => {
    const start = performance.now();
    const result = await withSocket(
      () => undefined,
      ["wait", "--phase", "playing", "--timeout-ms", "150"],
    );
    expect(result.code).toBe(3);
    expect(performance.now() - start).toBeLessThan(1500);
  });

  test("incoming bytes cannot extend wait's absolute deadline", async () => {
    const start = performance.now();
    const result = await withSocket(
      (socket) => {
        const timer = setInterval(() => socket.write(" "), 10);
        socket.once("close", () => clearInterval(timer));
      },
      ["wait", "--phase", "playing", "--timeout-ms", "150"],
    );
    expect(result.code).toBe(3);
    expect(performance.now() - start).toBeLessThan(1500);
  });

  test("static historical time-advance age never proves motion", async () => {
    const result = await withSocket(
      (socket) => socket.end(snapshot(100, 10)),
      waitArgs,
    );
    expect(result.code).toBe(3);
  });

  test.each(["paused", "frames-only", "time-only"])(
    "rejects %s motion evidence",
    async (kind) => {
      let sample = 0;
      const result = await withSocket((socket) => {
        sample += 1;
        socket.end(
          snapshot(
            kind === "frames-only" ? 100 : sample * 100,
            kind === "time-only" ? 10 : sample * 10,
            kind === "paused" ? "paused" : "playing",
          ),
        );
      }, waitArgs);
      expect(result.code).toBe(3);
    },
  );

  test("requires time and frames growing across fresh playing snapshots", async () => {
    let sample = 0;
    const result = await withSocket((socket) => {
      sample += 1;
      socket.end(snapshot(sample * 100, sample * 10));
    }, waitArgs);
    expect(result.code).toBe(0);
    expect(sample).toBe(2);
  });

  test("just passes metacharacters as literal argv, not shell code", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sparrow-argv-"));
    const marker = join(directory, "injected");
    const term = `News; touch ${marker}; $(touch ${marker}) 'quoted'`;
    let received: unknown;
    try {
      const result = await withSocket(
        (socket, request) => {
          received = request;
          socket.end(
            '{"ok":true,"result":{"_tag":"channels","channels":[]}}\n',
          );
        },
        ["search", term],
        true,
      );
      expect(result.code).toBe(0);
      expect(received).toEqual({ token, request: { _tag: "search", term } });
      await expect(access(marker)).rejects.toThrow();
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
});

const waitArgs = [
  "wait",
  "--media-advancing",
  "--timeout-ms",
  "250",
  "--interval-ms",
  "50",
];
function snapshot(time: number, frames: number, phase = "playing"): string {
  return (
    JSON.stringify({
      ok: true,
      result: {
        _tag: "snapshot",
        channelName: "Fixture News",
        diagnostics: {
          phase,
          media: {
            paused: false,
            currentTimeMs: time,
            presentedFrames: frames,
            msSinceTimeAdvance: 0,
          },
        },
      },
    }) + "\n"
  );
}

async function withSocket(
  respond: (socket: Socket, request: unknown) => void,
  args: string[],
  throughJust = false,
) {
  const directory = await mkdtemp(join(tmpdir(), "sparrow-cli-"));
  const path = join(directory, "agent.sock");
  const sockets = new Set<Socket>();
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.on("error", () => undefined);
    let input = "";
    socket.on("data", (chunk) => {
      input += chunk.toString();
      if (input.includes("\n")) respond(socket, JSON.parse(input.trim()));
    });
    socket.on("close", () => sockets.delete(socket));
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(path, resolve);
  });
  try {
    const child = Bun.spawn(
      throughJust
        ? ["just", "--", "agent", ...args]
        : [process.execPath, cli, ...args],
      {
        cwd: resolve(import.meta.dir, "../.."),
        env: {
          ...process.env,
          SPARROW_AGENT_SOCKET: path,
          SPARROW_AGENT_TOKEN: token,
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    const timeout = setTimeout(() => child.kill(), 3000);
    try {
      const [code, stdout, stderr] = await Promise.all([
        child.exited,
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
      ]);
      return { code, stdout, stderr };
    } finally {
      clearTimeout(timeout);
    }
  } finally {
    for (const socket of sockets) socket.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(directory, { recursive: true, force: true });
  }
}
