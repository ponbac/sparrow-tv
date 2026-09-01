import type {
  CatalogStatus,
  ClientResult,
  InstalledSparrowClient,
  RefreshReport,
  SparrowEvent,
} from "../../client/contracts";
import { InstalledSourceSettings } from "../configuration/installed-source-settings";
import { SourceStatusDesk } from "../status/source-status-desk";

export type FeedsDialogContentProps = {
  readonly status: CatalogStatus | null;
  readonly refreshing: boolean;
  readonly refreshResult: ClientResult<RefreshReport> | null;
  readonly latestEvent: SparrowEvent | null;
  readonly onRefresh: () => void;
} & (
  | {
      readonly runtime: "hosted";
      readonly client?: never;
      readonly onApplied?: never;
    }
  | {
      readonly runtime: "installed";
      readonly client: Pick<InstalledSparrowClient, "replaceSourceConfiguration">;
      readonly onApplied: (status: CatalogStatus) => void;
    }
);

/** Heavy source diagnostics loaded only when the Feeds sheet is requested. */
export function FeedsDialogContent(props: FeedsDialogContentProps) {
  return (
    <>
      {props.runtime === "installed" ? (
        <InstalledSourceSettings
          client={props.client}
          status={props.status}
          onApplied={props.onApplied}
        />
      ) : null}
      <SourceStatusDesk
        status={props.status}
        refreshing={props.refreshing}
        refreshResult={props.refreshResult}
        latestEvent={props.latestEvent}
        onRefresh={props.onRefresh}
        sourceScope={props.runtime === "installed" ? "device" : "deployment"}
      />
    </>
  );
}
