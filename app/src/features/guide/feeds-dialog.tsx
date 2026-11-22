import { Dialog } from "@base-ui/react/dialog";
import { LoaderCircle, RadioTower, TriangleAlert, X } from "lucide-react";
import { lazy, Suspense, useRef, useState } from "react";
import type { CatalogStatus } from "../../client/contracts";
import { handleLinearDialogArrows } from "./dialog-navigation";
import type { FeedsDialogContentProps } from "./feeds-dialog-content";
import "./feeds-dialog.css";

const loadFeedsDialogContent = () => import("./feeds-dialog-content");
const FeedsDialogContent = lazy(async () => {
  const module = await loadFeedsDialogContent();
  return { default: module.FeedsDialogContent };
});

/** Opens source configuration and safe status without exposing private locations. */
export function FeedsDialog(props: FeedsDialogContentProps) {
  const popupRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const pill =
    props.television === true ? sourcePill(props.status, props.refreshing) : null;
  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      {pill === null ? null : (
        // The TV's one line about the sources, in place of the banner and the
        // readouts; it opens Sources, where the details are.
        <button
          className="feeds-status"
          type="button"
          data-tone={pill.tone}
          aria-label={`${pill.label}. Open Sources`}
          title={pill.detail}
          onFocus={loadFeedsDialogContent}
          onClick={() => setOpen(true)}
        >
          {pill.tone === "busy" ? (
            <LoaderCircle aria-hidden="true" />
          ) : (
            <TriangleAlert aria-hidden="true" />
          )}
          {pill.label}
        </button>
      )}
      <Dialog.Trigger
        className="feeds-trigger"
        data-attention={props.runtime === "installed" && props.status?.configuration.configured === false}
        onMouseEnter={loadFeedsDialogContent}
        onFocus={loadFeedsDialogContent}
      >
        <RadioTower aria-hidden="true" />
        Sources
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Backdrop className="feeds-dialog__backdrop" />
        <Dialog.Popup
          ref={popupRef}
          className="feeds-dialog__popup"
          onKeyDown={(event) => {
            if (popupRef.current !== null) {
              handleLinearDialogArrows(event, {
                television: props.television === true,
                popup: popupRef.current,
                scroller: contentRef.current,
              });
            }
          }}
        >
          <header className="feeds-dialog__header">
            <div>
              <Dialog.Title>Sources</Dialog.Title>
              <Dialog.Description>
                {props.runtime === "installed"
                  ? "Source locations stay on this device. Only their status is shown here."
                  : "The sources are set on the server. You can check and refresh them here."}
              </Dialog.Description>
            </div>
            <Dialog.Close
              className="feeds-dialog__close"
              aria-label="Close sources"
            >
              <X aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className="feeds-dialog__content" ref={contentRef}>
            <Suspense
              fallback={
                <p className="feeds-dialog__loading" role="status">
                  Opening sources…
                </p>
              }
            >
              <FeedsDialogContent {...props} />
            </Suspense>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** What the TV's status pill says about the sources, or null when all is well. */
function sourcePill(
  status: CatalogStatus | null,
  refreshing: boolean,
): {
  readonly tone: "warning" | "busy" | "attention";
  readonly label: string;
  readonly detail: string;
} | null {
  if (status === null) return null;
  if (!status.configuration.configured) {
    return {
      tone: "attention",
      label: "Set up sources",
      detail: "Open Sources to set up this device.",
    };
  }
  const states = [status.m3u._tag, status.epg?._tag];
  if (refreshing || states.includes("refreshing")) {
    return { tone: "busy", label: "Updating", detail: "Sparrow is checking the sources." };
  }
  if (status.generation !== null && (states.includes("failed") || states.includes("stale"))) {
    return {
      tone: "warning",
      label: states.includes("failed") ? "Saved catalog · update failed" : "Saved catalog",
      detail: "Showing the saved catalog. A fresh source check is pending.",
    };
  }
  return null;
}
