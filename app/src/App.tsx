import { CatalogBrowser } from "./features/catalog/catalog-browser";
import type { SparrowClient } from "./client/contracts";

/** Dependencies owned by the hosted React composition root. */
export interface AppProps {
  readonly client: SparrowClient;
}

/** Renders the capability-driven Sparrow catalog application. */
export default function App({ client }: AppProps) {
  return <CatalogBrowser client={client} />;
}
