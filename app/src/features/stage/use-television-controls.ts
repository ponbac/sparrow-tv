import { useEffect, useLayoutEffect, useRef } from "react";
import { androidPlayerBridge, type AndroidPlayerBridge } from "../playback/android-player-bridge";

interface TelevisionControls {
  readonly enabled: boolean;
  readonly mode: "watch" | "guide";
  readonly onShowGuide: () => void;
  readonly onZap: (direction: -1 | 1) => void;
}

const GUIDE_ROW = ".programme-guide__row .programme-guide__channel";
const SOURCES = ".programme-guide .feeds-trigger";
const GROUPS = ".programme-guide__groups";
const SELECTED_GROUP = `${GROUPS} [role="radio"][aria-checked="true"]`;
const ACTION =
  "button, input, select, textarea, a[href], [role=radio], [role=menuitem]";

/**
 * Gives the Android TV remote a usable focus target and routes native Back and
 * Channel keys. WebView moves focus spatially between ordinary controls.
 */
export function useTelevisionControls(
  controls: TelevisionControls,
  bridge: AndroidPlayerBridge | null = androidPlayerBridge,
): void {
  const latest = useRef(controls);
  useLayoutEffect(() => {
    latest.current = controls;
  });
  const { enabled } = controls;

  useEffect(() => {
    if (!enabled || bridge === null) return;
    const releaseEvents = bridge.subscribeEvents((event) => {
      if (event._tag === "television-channel") {
        latest.current.onZap(event.direction);
      }
    });
    const releaseBack = bridge.subscribeTelevisionBack(() => {
      // Escape is Base UI's shared dismiss event for dialogs, menus and search.
      // A widget that handled it cancels the event.
      const target = document.activeElement ?? document.body;
      const unhandled = target.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
          cancelable: true,
        }),
      );
      if (!unhandled) return;
      if (document.fullscreenElement != null) {
        void document.exitFullscreen().then(
          () => latest.current.onShowGuide(),
          () => undefined,
        );
      } else if (latest.current.mode === "watch") {
        latest.current.onShowGuide();
      } else {
        bridge.defaultTelevisionBack();
      }
    });
    return () => {
      releaseBack();
      releaseEvents();
    };
  }, [bridge, enabled]);

  useEffect(() => {
    if (!enabled) return;
    let frame: number | null = null;
    const scheduleFocus = () => {
      if (frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        if (!usable(document.activeElement)) {
          fallbackFocus(latest.current.mode)?.focus();
        }
      });
    };
    const observer = new MutationObserver(scheduleFocus);
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["disabled", "data-mode"],
    });
    document.addEventListener("focusout", restoreAfterBlur);
    document.addEventListener("fullscreenchange", scheduleFocus);
    document.addEventListener("focusin", keepDialogFocus);
    document.addEventListener("keydown", moveInGuide, true);
    scheduleFocus();
    return () => {
      observer.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
      document.removeEventListener("focusout", restoreAfterBlur);
      document.removeEventListener("fullscreenchange", scheduleFocus);
      document.removeEventListener("focusin", keepDialogFocus);
      document.removeEventListener("keydown", moveInGuide, true);
    };

    function restoreAfterBlur(event: FocusEvent) {
      if (event.relatedTarget === null) scheduleFocus();
    }
  }, [enabled]);
}

function openDialog(): HTMLElement | null {
  const dialogs = document.querySelectorAll<HTMLElement>('[role="dialog"][data-open]');
  return dialogs[dialogs.length - 1] ?? null;
}

function usable(element: EventTarget | null): element is HTMLElement {
  return (
    element instanceof HTMLElement &&
    (element.matches(ACTION) || element === document.fullscreenElement) &&
    !element.matches(":disabled, [data-disabled]") &&
    (document.fullscreenElement?.contains(element) ?? true) &&
    element.getClientRects().length > 0
  );
}

function firstControl(scope: ParentNode, selector: string): HTMLElement | undefined {
  return Array.from(scope.querySelectorAll<HTMLElement>(selector)).find(usable);
}

function fallbackFocus(mode: TelevisionControls["mode"]): HTMLElement | undefined {
  const dialog = openDialog();
  if (dialog !== null) return firstControl(dialog, ACTION);
  if (document.fullscreenElement instanceof HTMLElement) return document.fullscreenElement;
  if (mode === "watch") return firstControl(document, ".channel-bar__guide");
  return (
    firstControl(document, '.programme-guide__row[data-playing="true"] .programme-guide__channel') ??
    firstControl(document, GUIDE_ROW) ??
    (document.querySelector('.programme-guide__board[aria-busy="true"]') === null
      ? firstControl(document, SOURCES)
      : undefined)
  );
}

function keepDialogFocus(event: FocusEvent): void {
  const dialog = openDialog();
  if (dialog === null || !(event.target instanceof Node) || dialog.contains(event.target)) {
    return;
  }
  const previous = event.relatedTarget;
  if (usable(previous) && dialog.contains(previous)) previous.focus();
  else firstControl(dialog, ACTION)?.focus();
}

function moveInGuide(event: KeyboardEvent): void {
  if (
    event.defaultPrevented ||
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    event.isComposing ||
    !(event.target instanceof HTMLElement) ||
    !["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(event.key)
  ) return;
  const target = event.target;
  const guide = target.closest(".programme-guide");
  if (guide === null) return;
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    // Base UI wraps the group radios. From either edge, the remote can
    // continue horizontally to the time or group chooser button.
    const radios = target.closest(GROUPS)?.querySelectorAll('[role="radio"]');
    const left = event.key === "ArrowLeft";
    const edge = left ? radios?.item(0) : radios?.item(radios.length - 1);
    const control = target === edge
      ? firstControl(guide, left ? ".programme-guide__when" : ".group-roster-trigger")
      : undefined;
    if (control !== undefined) {
      event.preventDefault();
      event.stopPropagation();
      control.focus();
    }
    return;
  }
  const down = event.key === "ArrowDown";
  const row = target.closest('.programme-guide__row[data-body="list"]');
  if (row !== null && row.parentElement !== null) {
    // WebView can move vertically onto a quality chip within the same Channel.
    // Advance one grid row in the current column and leave the chips to Left/Right.
    const grid = row.parentElement;
    const columns = getComputedStyle(grid).gridTemplateColumns.split(" ").length;
    const index = Array.from(grid.children).indexOf(row);
    const adjacentStart = (Math.floor(index / columns) + (down ? 1 : -1)) * columns;
    const control =
      adjacentStart < 0
        ? firstControl(guide, SELECTED_GROUP)
        : adjacentStart >= grid.children.length
          ? firstControl(guide, ".programme-guide__retained button, .programme-guide__more")
          : grid.children
              .item(Math.min(adjacentStart + index % columns, grid.children.length - 1))
              ?.querySelector<HTMLButtonElement>(".programme-guide__channel");
    if (control != null) {
      event.preventDefault();
      control.focus();
    }
    return;
  }
  // The search field and group lane need explicit vertical exits. On TV,
  // WebView may keep focus there even when there are Channel rows below.
  let destination: string;
  if (target.closest(`${GROUPS}, .group-roster-trigger`) !== null) {
    destination = down ? GUIDE_ROW : SOURCES;
  } else if (
    down &&
    target.matches('.board-search input[aria-expanded="false"]')
  ) {
    destination = SELECTED_GROUP;
  } else {
    return;
  }
  const control = firstControl(guide, destination);
  if (control === undefined) return;
  event.preventDefault();
  event.stopPropagation();
  control.focus();
}
