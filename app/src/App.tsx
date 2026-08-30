import { CatalogBrowser } from "./features/catalog/catalog-browser";
import type { SparrowRuntime } from "./client/runtime";

/** Dependencies owned by the selected React composition root. */
export interface AppProps {
  readonly runtime: SparrowRuntime;
}

/** Renders the capability-driven Sparrow catalog application. */
export default function App({ runtime }: AppProps) {
  return runtime._tag === "hosted" ? (
    <CatalogBrowser client={runtime.client} runtime="hosted" />
  ) : (
    <CatalogBrowser client={runtime.client} runtime="installed" />
  );
}
