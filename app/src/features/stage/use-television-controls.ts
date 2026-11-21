import { useEffect, useLayoutEffect, useRef } from "react";
import { findGuideFallback, scrollBoardToReveal } from "../guide/guide-navigation";
import { androidPlayerBridge, type AndroidPlayerBridge } from "../playback/android-player-bridge";
import { isVisibleControl } from "./stage-dom";

interface TelevisionControls {
  readonly enabled: boolean;
  readonly mode: "watch" | "guide";
  readonly onShowGuide: () => void;
  readonly onZap: (direction: -1 | 1) => void;
}

const ACTION =
  'button, input, select, textarea, a[href], summary, [tabindex="0"], [role="radio"], [role="menuitem"], [role="checkbox"], [role="switch"], [role="option"], [role="menuitemradio"], [role="menuitemcheckbox"], [role="tab"]';
const OVERLAY =
  '[role="dialog"][data-open], [role="alertdialog"][data-open], [role="menu"][data-open]';
const ZAP_BLOCKING_POPUP =
  '[role="listbox"][data-open], .board-search input[aria-expanded="true"]';

/**
 * Gives the Android TV remote a usable focus target, keeps open overlays
 * contained, and routes Escape, native Back, and Channel keys. Guide 2D
 * movement is owned by `guide-navigation.ts`.
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
        if (
          openOverlay() !== null ||
          document.querySelector(ZAP_BLOCKING_POPUP) !== null
        ) {
          return;
        }
        latest.current.onZap(event.direction);
      }
    });
    const releaseBack = bridge.subscribeTelevisionBack(() => {
      // Escape is Base UI's shared dismiss event for dialogs, menus, search,
      // visible fullscreen controls, and Watch/fullscreen exit below.
      const target = document.activeElement ?? document.body;
      const unhandled = target.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
          cancelable: true,
        }),
      );
      if (unhandled) {
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
    const settledOverlays = new WeakSet<HTMLElement>();
    const scheduleFocus = () => {
      if (frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        if (usable(document.activeElement)) return;
        // Let a newly opened Base UI dialog run its own initialFocus rAF before
        // applying generic overlay fallback focus.
        const overlay = openOverlay();
        if (overlay !== null && !settledOverlays.has(overlay)) {
          settledOverlays.add(overlay);
          scheduleFocus();
          return;
        }
        const target = fallbackFocus(latest.current.mode);
        if (target !== undefined) {
          target.focus({ preventScroll: true });
          const board = target.closest<HTMLElement>(".programme-guide__board");
          if (board !== null) {
            scrollBoardToReveal(board, target);
          }
        }
      });
    };
    const observer = new MutationObserver(scheduleFocus);
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: [
        "disabled",
        "data-disabled",
        "data-mode",
        "data-state",
        "aria-busy",
        "hidden",
        "data-controls-visible",
      ],
    });
    document.addEventListener("focusout", restoreAfterBlur);
    document.addEventListener("fullscreenchange", scheduleFocus);
    document.addEventListener("focusin", keepOverlayFocus);
    document.addEventListener("keydown", clampMenuHorizontalArrows, true);
    document.addEventListener("keydown", handleUnhandledEscape);
    scheduleFocus();
    return () => {
      observer.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
      document.removeEventListener("focusout", restoreAfterBlur);
      document.removeEventListener("fullscreenchange", scheduleFocus);
      document.removeEventListener("focusin", keepOverlayFocus);
      document.removeEventListener("keydown", clampMenuHorizontalArrows, true);
      document.removeEventListener("keydown", handleUnhandledEscape);
    };

    function restoreAfterBlur(event: FocusEvent) {
      if (event.relatedTarget === null) scheduleFocus();
    }

    function handleUnhandledEscape(event: KeyboardEvent) {
      if (event.defaultPrevented || event.isComposing || event.key !== "Escape") {
        return;
      }
      if (openOverlay() !== null) return;
      if (document.fullscreenElement != null) {
        event.preventDefault();
        void document.exitFullscreen().then(
          () => latest.current.onShowGuide(),
          () => undefined,
        );
      } else if (latest.current.mode === "watch") {
        event.preventDefault();
        latest.current.onShowGuide();
      }
    }
  }, [enabled]);
}

function openOverlay(): HTMLElement | null {
  const overlays = document.querySelectorAll<HTMLElement>(OVERLAY);
  return overlays[overlays.length - 1] ?? null;
}

function fullscreenFailureAction(surface: HTMLElement): HTMLElement | undefined {
  if (surface.querySelector('.hosted-player__state[data-state="failed"]') === null) {
    return undefined;
  }
  return (
    firstControl(surface, ".hosted-player__recovery") ??
    firstControl(surface, ".hosted-player__stop")
  );
}

function usable(element: EventTarget | null): element is HTMLElement {
  if (!(element instanceof HTMLElement)) {
    return false;
  }
  const isFullscreenSurface = element === document.fullscreenElement;
  if (isFullscreenSurface) {
    if (fullscreenFailureAction(element) !== undefined) {
      return false;
    }
    return isVisibleControl(element, {
      allowNegativeTabIndex: true,
      requireLayoutBox: true,
    });
  }
  if (!element.matches(ACTION)) {
    return false;
  }
  if (!(document.fullscreenElement?.contains(element) ?? true)) {
    return false;
  }
  return isVisibleControl(element, {
    allowNegativeTabIndex: element.matches(
      '[role="radio"], [role="menuitem"], [role="option"]',
    ),
    requireLayoutBox: true,
  });
}

function firstControl(scope: ParentNode, selector: string): HTMLElement | undefined {
  return Array.from(scope.querySelectorAll<HTMLElement>(selector)).find(usable);
}

function fallbackFocus(mode: TelevisionControls["mode"]): HTMLElement | undefined {
  const overlay = openOverlay();
  if (overlay !== null) return firstControl(overlay, ACTION);
  if (document.fullscreenElement instanceof HTMLElement) {
    return (
      fullscreenFailureAction(document.fullscreenElement) ??
      document.fullscreenElement
    );
  }
  if (mode === "watch") {
    return (
      firstControl(document, ".playback-load-notice button") ??
      firstControl(document, ".hosted-player__recovery") ??
      firstControl(document, ".channel-bar__guide") ??
      firstControl(document, ".channel-bar button") ??
      firstControl(document, ".now-playing__controls button")
    );
  }
  return (
    findGuideFallback(document.querySelector(".programme-guide"), {
      includeNotice: true,
      respectBusyBoard: true,
      requireLayoutBox: true,
    }) ?? undefined
  );
}

function keepOverlayFocus(event: FocusEvent): void {
  const overlay = openOverlay();
  if (overlay === null || !(event.target instanceof Node) || overlay.contains(event.target)) {
    return;
  }
  if (
    event.target instanceof Element &&
    event.target.closest('[role="listbox"][data-open], [role="menu"][data-open]') !== null
  ) {
    return;
  }
  const previous = event.relatedTarget;
  if (usable(previous) && overlay.contains(previous)) {
    previous.focus({ preventScroll: true });
  } else {
    firstControl(overlay, ACTION)?.focus({ preventScroll: true });
  }
}

function clampMenuHorizontalArrows(event: KeyboardEvent): void {
  if (
    event.defaultPrevented ||
    event.isComposing ||
    (event.key !== "ArrowLeft" && event.key !== "ArrowRight") ||
    !(event.target instanceof Element)
  ) {
    return;
  }
  if (event.target.closest('[role="menu"][data-open]') !== null) {
    event.preventDefault();
    event.stopPropagation();
  }
}
