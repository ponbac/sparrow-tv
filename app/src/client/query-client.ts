import { QueryClient } from "@tanstack/react-query";
import type { SparrowRuntime } from "./runtime";

/** Local IPC remains usable offline; hosted requests follow network availability. */
export function createSparrowQueryClient(runtime: SparrowRuntime["_tag"]): QueryClient {
  const networkMode = runtime === "installed" ? "always" : "online";
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false, networkMode },
      mutations: { networkMode },
    },
  });
}
