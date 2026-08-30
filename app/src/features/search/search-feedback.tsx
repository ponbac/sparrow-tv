import type { ClientError } from "../../client/contracts";

/** Renders a compact, accessible pending state for one search or schedule lane. */
export function SearchLaneLoading({ label }: { readonly label: string }) {
  return (
    <div className="search-lane-loading" role="status">
      <span aria-hidden="true" />
      {label}…
    </div>
  );
}

/** Stops mixed-generation pagination and restarts it from one current snapshot. */
export function GenerationNotice({
  onRestart,
}: {
  readonly onRestart: () => void;
}) {
  return (
    <div className="generation-notice" role="status">
      <p>
        The catalog changed during pagination. Restart to use one generation.
      </p>
      <button type="button" onClick={onRestart}>
        Restart scan
      </button>
    </div>
  );
}

/** Renders one expected client failure with an explicit safe restart action. */
export function SearchLaneError({
  error,
  onRestart,
  retained,
}: {
  readonly error: ClientError;
  readonly onRestart: () => void;
  readonly retained: boolean;
}) {
  const copy = searchErrorCopy(error);
  return (
    <div className="lane-error" role="alert">
      <strong>{copy.title}</strong>
      <p>
        {retained ? "Earlier results remain visible. " : ""}
        {copy.detail}
      </p>
      <button type="button" onClick={onRestart}>
        {error._tag === "stale-cursor" ? "Restart on current catalog" : "Try again"}
      </button>
    </div>
  );
}

function searchErrorCopy(error: ClientError): {
  readonly title: string;
  readonly detail: string;
} {
  switch (error._tag) {
    case "authentication-required":
      return {
        title: "Access credential required",
        detail: "Authenticate with this Sparrow deployment, then restart the scan.",
      };
    case "service-unavailable":
      return {
        title: "The search desk is temporarily unavailable",
        detail: "No catalog state changed. Wait a moment, then restart the scan.",
      };
    case "invalid-input":
      return {
        title: "The search request was rejected",
        detail: "Adjust the search term or restart pagination from the first page.",
      };
    case "not-configured":
      return {
        title: "No Channel source is configured",
        detail: "Search becomes available after this deployment has a Channel source.",
      };
    case "catalog-unavailable":
      return {
        title: "The Channel Catalog is unavailable",
        detail: "No validated Channel snapshot is available yet.",
      };
    case "not-found":
      return {
        title: "That Channel left the catalog",
        detail: "Choose a Channel from the current result generation.",
      };
    case "stale-cursor":
      return {
        title: "A newer catalog is on air",
        detail: `Pagination moved to generation ${error.current}. Restart from its first page.`,
      };
    case "transport":
      return {
        title: "The hosted desk did not answer",
        detail: "The request can be retried without changing catalog state.",
      };
    case "cancelled":
      return {
        title: "The request was cancelled",
        detail: "No catalog state changed. Restart when ready.",
      };
  }
}
