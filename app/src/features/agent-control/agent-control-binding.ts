import {
  executeAgentControl,
  parseAgentControlRequest,
  type AgentControlCatalog,
  type AgentControlPlayback,
  type AgentControlResponse,
} from "./agent-control";

type AgentControlDispatch = (
  id: string,
  payload: unknown,
  expiresAt?: number,
) => void;
type AgentControlGlobal = {
  __sparrowAgentControl?: AgentControlDispatch;
  __sparrowAgentControlCancel?: (id: string) => void;
};

let catalog: AgentControlCatalog | null = null;
let playback: AgentControlPlayback | null = null;
const pending = new Set<AbortController>();
let mutation: {
  readonly tag: "tune" | "stop";
  readonly controller: AbortController;
} | null = null;
const REQUEST_TIMEOUT_MS = 12_000;
const MAX_PENDING = 8;

/** Binds catalog capabilities; releasing cancels work that captured this catalog. */
export function bindAgentControlCatalog(next: AgentControlCatalog): () => void {
  for (const controller of pending) controller.abort();
  catalog = next;
  return () => {
    if (catalog === next) {
      for (const controller of pending) controller.abort();
      catalog = null;
    }
  };
}

/** Binds diagnostics and confirmed stop for the mounted installed player. */
export function bindAgentControlPlayback(
  next: AgentControlPlayback,
): () => void {
  playback = next;
  return () => {
    if (playback === next) playback = null;
  };
}

/** Executes bounded work; stop preempts pending tune, never queues behind search. */
export function dispatchAgentControl(
  payload: unknown,
  options: { readonly signal?: AbortSignal; readonly timeoutMs?: number } = {},
): Promise<AgentControlResponse> {
  const parsed = parseAgentControlRequest(payload);
  if (!parsed.ok) return Promise.resolve({ ok: false, error: parsed.error });
  const tag = parsed.request._tag;
  if (tag === "stop" && mutation?.tag === "tune") mutation.controller.abort();
  if (
    pending.size >= MAX_PENDING ||
    ((tag === "tune" || tag === "stop") &&
      mutation !== null &&
      (mutation.tag === "stop" || !mutation.controller.signal.aborted))
  ) {
    return Promise.resolve({ ok: false, error: { _tag: "unavailable" } });
  }
  const controller = new AbortController();
  const timeoutMs = Math.min(
    options.timeoutMs ?? REQUEST_TIMEOUT_MS,
    REQUEST_TIMEOUT_MS,
  );
  if (
    options.signal?.aborted ||
    !Number.isFinite(timeoutMs) ||
    timeoutMs <= 0
  ) {
    return Promise.resolve({ ok: false, error: { _tag: "timeout" } });
  }
  const deadline = performance.now() + timeoutMs;
  pending.add(controller);
  if (tag === "tune" || tag === "stop") mutation = { tag, controller };
  return new Promise((resolve) => {
    const abort = () => controller.abort();
    const timer = setTimeout(abort, timeoutMs);
    const finish = (body: AgentControlResponse, completed = true) => {
      clearTimeout(timer);
      controller.signal.removeEventListener("abort", onAbort);
      options.signal?.removeEventListener("abort", abort);
      if (completed) {
        pending.delete(controller);
        if (mutation?.controller === controller) mutation = null;
      }
      resolve(body);
    };
    const onAbort = () =>
      finish({ ok: false, error: { _tag: "timeout" } }, false);
    controller.signal.addEventListener("abort", onAbort, { once: true });
    options.signal?.addEventListener("abort", abort, { once: true });
    // Both settlement paths are owned. A non-cooperative catalog may finish
    // late, but execute checks cancellation before committing a tune intent.
    void executeAgentControl(
      parsed.request,
      catalog,
      playback,
      controller.signal,
      () => {
        // Timers may be delayed by a suspended webview. Check the absolute
        // deadline at the commit seam, not only when the timer gets CPU time.
        if (performance.now() >= deadline) controller.abort();
        return controller.signal.aborted;
      },
    ).then(finish, () =>
      finish({
        ok: false,
        error: { _tag: tag === "stop" ? "cleanup-failed" : "unavailable" },
      }),
    );
  });
}

/** Installs Rust's dispatch/cancel bridge; uninstall aborts every owned request. */
export function installAgentControlDispatch(
  reply: (id: string, body: AgentControlResponse) => Promise<void>,
): () => void {
  // SAFETY: only installed composition owns these private webview globals.
  const target = globalThis as AgentControlGlobal;
  const requests = new Map<string, AbortController>();
  const cancel = (id: string) => requests.get(id)?.abort();
  const dispatch: AgentControlDispatch = (id, payload, expiresAt) => {
    if (requests.has(id) || requests.size >= MAX_PENDING) return;
    const controller = new AbortController();
    requests.set(id, controller);
    void dispatchAgentControl(payload, {
      signal: controller.signal,
      timeoutMs:
        expiresAt === undefined ? REQUEST_TIMEOUT_MS : expiresAt - Date.now(),
    })
      .then(async (body) => {
        if (!controller.signal.aborted) await reply(id, body);
        requests.delete(id);
      })
      .catch(() => {
        requests.delete(id);
      });
  };
  target.__sparrowAgentControl = dispatch;
  target.__sparrowAgentControlCancel = cancel;
  return () => {
    for (const controller of requests.values()) controller.abort();
    requests.clear();
    if (target.__sparrowAgentControl === dispatch) {
      delete target.__sparrowAgentControl;
      delete target.__sparrowAgentControlCancel;
    }
  };
}
