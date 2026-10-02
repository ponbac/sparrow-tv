import { Dialog } from "@base-ui/react/dialog";
import { RadioTower, X } from "lucide-react";
import { lazy, Suspense } from "react";
import type { FeedsDialogContentProps } from "./feeds-dialog-content";
import "./feeds-dialog.css";

const loadFeedsDialogContent = () => import("./feeds-dialog-content");
const FeedsDialogContent = lazy(async () => {
  const module = await loadFeedsDialogContent();
  return { default: module.FeedsDialogContent };
});

/** Opens source configuration and safe status without exposing private locations. */
export function FeedsDialog(props: FeedsDialogContentProps) {
  const configured = props.status?.configuration.configured === true;
  return (
    <Dialog.Root>
      <Dialog.Trigger
        className="feeds-trigger"
        data-attention={props.runtime === "installed" && !configured}
        onMouseEnter={loadFeedsDialogContent}
        onFocus={loadFeedsDialogContent}
      >
        <RadioTower aria-hidden="true" />
        Sources
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Backdrop className="feeds-dialog__backdrop" />
        <Dialog.Popup className="feeds-dialog__popup">
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
          <div className="feeds-dialog__content">
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
