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
  /** Keeps busy buttons focusable and tailors copy for Android TV remotes. */
  readonly television?: boolean;
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

/** Heavy source diagnostics loaded only when the Sources sheet is requested. */
export function FeedsDialogContent(props: FeedsDialogContentProps) {
  const settings =
    props.runtime === "installed" ? (
      <InstalledSourceSettings
        television={props.television ?? false}
        client={props.client}
        status={props.status}
        onApplied={props.onApplied}
      />
    ) : null;
  const desk = (
    <SourceStatusDesk
      television={props.television ?? false}
      status={props.status}
      refreshing={props.refreshing}
      refreshResult={props.refreshResult}
      latestEvent={props.latestEvent}
      onRefresh={props.onRefresh}
      sourceScope={props.runtime === "installed" ? "device" : "deployment"}
    />
  );
  // On a TV the sheet opens on how the sources are doing; changing them is
  // the rarer errand, further down.
  return props.television === true ? (
    <>
      {desk}
      {settings}
    </>
  ) : (
    <>
      {settings}
      {desk}
    </>
  );
}
