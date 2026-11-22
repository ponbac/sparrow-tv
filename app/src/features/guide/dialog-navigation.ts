import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import {
  isVisibleControl,
  revealInScrollContainer,
} from "../stage/stage-dom";

const DIALOG_CONTROL_SELECTOR =
  'button, input, select, textarea, a[href], summary, [tabindex="0"], [role="button"]';

export { revealInScrollContainer };

/**
 * Keeps Android TV search inputs read-only while the viewer is browsing across
 * them so D-pad focus does not immediately summon the on-screen keyboard.
 * Pressing Enter or clicking starts editing; pressing Escape or blurring leaves edit mode.
 */
export function useTelevisionInputEdit(television: boolean): {
  readonly readOnly: boolean;
  readonly startEditing: () => void;
  readonly stopEditing: () => void;
  readonly handleKeyDown: (
    event: KeyboardEvent<HTMLInputElement>,
    options?: { readonly popupOpen?: boolean },
  ) => boolean;
} {
  const [editing, setEditing] = useState(false);
  const startEditing = useCallback(() => {
    if (television) {
      setEditing(true);
    }
  }, [television]);
  const stopEditing = useCallback(() => {
    setEditing(false);
  }, []);
  const handleKeyDown = useCallback(
    (
      event: KeyboardEvent<HTMLInputElement>,
      options?: { readonly popupOpen?: boolean },
    ): boolean => {
      if (!television || event.nativeEvent.isComposing) {
        return false;
      }
      if (
        !editing &&
        event.key === "Enter" &&
        !event.repeat &&
        options?.popupOpen !== true
      ) {
        event.preventDefault();
        event.stopPropagation();
        setEditing(true);
        return true;
      }
      if (editing && event.key === "Escape" && options?.popupOpen !== true) {
        event.preventDefault();
        event.stopPropagation();
        setEditing(false);
        return true;
      }
      return false;
    },
    [editing, television],
  );
  return {
    readOnly: television && !editing,
    startEditing,
    stopEditing,
    handleKeyDown,
  };
}

/** Reports whether a control inside a sheet is visible and focusable. */
export function isVisibleDialogControl(element: HTMLElement): boolean {
  return isVisibleControl(element, { checkDetails: true });
}

function findAdjacentDialogControl(
  root: HTMLElement,
  target: HTMLElement,
  step: 1 | -1,
): { readonly found: boolean; readonly next: HTMLElement | null } {
  const candidates = Array.from(
    root.querySelectorAll<HTMLElement>(DIALOG_CONTROL_SELECTOR),
  );
  const index = candidates.indexOf(target);
  if (index === -1) {
    return { found: false, next: null };
  }
  for (let i = index + step; i >= 0 && i < candidates.length; i += step) {
    if (isVisibleDialogControl(candidates[i])) {
      return { found: true, next: candidates[i] };
    }
  }
  return { found: true, next: null };
}

/**
 * Deterministic vertical and horizontal TV D-pad navigation for single-column
 * dialogs and sheets (Sources, full channel search), clamping at edges so
 * WebView spatial navigation stays inside the open sheet. No-op off TV.
 */
export function handleLinearDialogArrows(
  event: KeyboardEvent<HTMLElement>,
  {
    television,
    popup,
    scroller,
    horizontalGroups = [],
  }: {
    readonly television: boolean;
    readonly popup: HTMLElement;
    readonly scroller: HTMLElement | null;
    readonly horizontalGroups?: readonly string[];
  },
): void {
  if (
    !television ||
    event.defaultPrevented ||
    event.shiftKey ||
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    event.nativeEvent.isComposing ||
    !(event.target instanceof HTMLElement)
  ) {
    return;
  }
  const { key, target } = event;
  if (
    key !== "ArrowUp" &&
    key !== "ArrowDown" &&
    key !== "ArrowLeft" &&
    key !== "ArrowRight"
  ) {
    return;
  }
  const editingTextInput =
    target instanceof HTMLInputElement && !target.readOnly;
  if ((key === "ArrowLeft" || key === "ArrowRight") && editingTextInput) {
    return;
  }
  if (key === "ArrowLeft" || key === "ArrowRight") {
    for (const selector of horizontalGroups) {
      const group = target.closest<HTMLElement>(selector);
      if (group !== null && popup.contains(group)) {
        const result = findAdjacentDialogControl(
          group,
          target,
          key === "ArrowRight" ? 1 : -1,
        );
        if (result.found) {
          event.preventDefault();
          event.stopPropagation();
          if (result.next !== null) {
            result.next.focus({ preventScroll: true });
            revealInScrollContainer(scroller, result.next);
          }
          return;
        }
      }
    }
    event.preventDefault();
    event.stopPropagation();
    return;
  }

  // Let scrollable diagnostic regions scroll before stepping off them.
  if (target.tagName === "PRE" && target.scrollHeight > target.clientHeight + 1) {
    if (
      key === "ArrowDown" &&
      target.scrollTop + target.clientHeight < target.scrollHeight - 1
    ) {
      event.preventDefault();
      event.stopPropagation();
      target.scrollTop += Math.max(32, Math.round(target.clientHeight * 0.5));
      return;
    }
    if (key === "ArrowUp" && target.scrollTop > 0) {
      event.preventDefault();
      event.stopPropagation();
      target.scrollTop = Math.max(
        0,
        target.scrollTop - Math.max(32, Math.round(target.clientHeight * 0.5)),
      );
      return;
    }
  }

  const result = findAdjacentDialogControl(
    popup,
    target,
    key === "ArrowDown" ? 1 : -1,
  );
  if (!result.found) {
    return;
  }
  event.preventDefault();
  event.stopPropagation();
  if (result.next !== null) {
    result.next.focus({ preventScroll: true });
    revealInScrollContainer(scroller, result.next);
  }
}

/**
 * Preserves TV D-pad focus across "More channels" page loads (including when a
 * tail prefetch was already in flight), moving focus to the first newly added
 * row, a recovery action on error, or the last row when the final page removes
 * the button—while staying on "More channels" if a non-final page adds no new
 * visible rows and leaving focus untouched if the viewer moved away.
 */
export function useMoreButtonFocusHandoff({
  television,
  active,
  busy,
  hasError,
  scopeKey,
  rowCount,
  canAutoLoad,
  onRequestAutoLoad,
  onManualLoadMore,
  resolveTarget,
  revealTarget,
}: {
  readonly television: boolean;
  readonly active: boolean;
  readonly busy: boolean;
  readonly hasError: boolean;
  readonly scopeKey: string;
  readonly rowCount: number;
  readonly canAutoLoad: boolean;
  readonly onRequestAutoLoad: () => void;
  readonly onManualLoadMore: () => void;
  readonly resolveTarget: (options: {
    readonly existingRows: number;
    readonly hasError: boolean;
    readonly buttonConnected: boolean;
  }) => HTMLElement | null | undefined;
  readonly revealTarget: (target: HTMLElement) => void;
}): {
  readonly onPointerDown: () => void;
  readonly onPointerCancel: () => void;
  readonly onBlur: () => void;
  readonly onFocus: (event: FocusEvent<HTMLButtonElement>) => void;
  readonly onClick: (event: MouseEvent<HTMLButtonElement>) => void;
} {
  const pointerDownRef = useRef(false);
  const requestRef = useRef<{
    readonly button: HTMLButtonElement;
    readonly existingRows: number;
    readonly scopeKey: string;
  } | null>(null);
  const resolveRef = useRef(resolveTarget);
  resolveRef.current = resolveTarget;
  const revealRef = useRef(revealTarget);
  revealRef.current = revealTarget;

  useLayoutEffect(() => {
    const request = requestRef.current;
    if (request === null || busy) return;
    requestRef.current = null;
    if (!active || scopeKey !== request.scopeKey) return;
    const focus = document.activeElement;
    if (
      focus !== request.button &&
      (request.button.isConnected || focus !== document.body)
    ) {
      return;
    }
    const target = resolveRef.current({
      existingRows: request.existingRows,
      hasError,
      buttonConnected: request.button.isConnected,
    });
    if (target != null) {
      target.focus({ preventScroll: true });
      revealRef.current(target);
    }
  }, [active, busy, hasError, rowCount, scopeKey]);

  return {
    onPointerDown: () => {
      pointerDownRef.current = true;
    },
    onPointerCancel: () => {
      pointerDownRef.current = false;
    },
    onBlur: () => {
      pointerDownRef.current = false;
    },
    onFocus: (event) => {
      if (pointerDownRef.current) return;
      if (television && (canAutoLoad || busy)) {
        if (
          requestRef.current === null ||
          requestRef.current.scopeKey !== scopeKey
        ) {
          requestRef.current = {
            button: event.currentTarget,
            existingRows: rowCount,
            scopeKey,
          };
        }
      }
      if (canAutoLoad) {
        onRequestAutoLoad();
      }
    },
    onClick: (event) => {
      pointerDownRef.current = false;
      if (busy) return;
      if (television && event.currentTarget === document.activeElement) {
        requestRef.current = {
          button: event.currentTarget,
          existingRows: rowCount,
          scopeKey,
        };
      }
      if (canAutoLoad) {
        onRequestAutoLoad();
      } else {
        onManualLoadMore();
      }
    },
  };
}
