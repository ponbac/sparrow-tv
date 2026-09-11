import { lazy, Suspense } from "react";
import { CatalogBrowser } from "./features/catalog/catalog-browser";
import type { SparrowRuntime } from "./client/runtime";

const InstalledAgentBridge = lazy(async () => {
  const module = await import("./features/agent-control/installed-agent-bridge");
  return { default: module.InstalledAgentBridge };
});

/** Dependencies owned by the selected React composition root. */
export interface AppProps {
  readonly runtime: SparrowRuntime;
}

/** Renders the capability-driven Sparrow catalog application. */
export default function App({ runtime }: AppProps) {
  return runtime._tag === "hosted" ? (
    <CatalogBrowser client={runtime.client} runtime="hosted" />
  ) : (
    <>
      <Suspense fallback={null}>
        <InstalledAgentBridge />
      </Suspense>
      <CatalogBrowser client={runtime.client} runtime="installed" />
    </>
  );
}
