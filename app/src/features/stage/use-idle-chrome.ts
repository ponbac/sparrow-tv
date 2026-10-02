import { useEffect, useState } from "react";
import {
  focusStageMonitor,
  STAGE_CHROME,
  stageSearchInput,
} from "./stage-dom";

const IDLE_MS = 3000;
const OPEN_POPUP = '[role="menu"], [role="listbox"], [role="dialog"]';

/** Whether the chrome over the picture is in view. */
export type ChromeVisibility = "shown" | "hidden";

/**
 * Hides the chrome over the picture once the viewer has left it alone for
 * three seconds, and shows it again on any pointer or key input.
 *
 * `armed` says the chrome may hide at all: the Theater layout in watch mode
 * with a picture playing in the page. Otherwise the chrome is shown and the
 * hook attaches no listeners and holds no timer.
 *
 * The chrome stays while it is in use: the pointer rests on it, the search
 * field has focus, a menu, list or dialog is open, the volume is being
 * dragged, or a native picker is open. When it hides, and when the viewer
 * has finished with a picker or the volume, focus returns to the picture so
 * the stage keys apply.
 */
export function useIdleChrome(armed: boolean): ChromeVisibility {
  const [hidden, setHidden] = useState(false);

  useEffect(() => {
    if (!armed) {
      return;
    }
    let timer: ReturnType<typeof setTimeout> | null = null;
    // Arming follows a click as often as not (the Guide button, a rail item),
    // and a pointer resting where it clicked sends no further event.
    let pointerOverChrome = inChrome(
      Array.from(document.querySelectorAll(":hover")).at(-1) ?? null,
    );
    let pointerAt: { readonly x: number; readonly y: number } | null = null;

    const wake = () => {
      setHidden(false);
      if (timer !== null) {
        clearTimeout(timer);
      }
      timer = setTimeout(hideWhenIdle, IDLE_MS);
    };
    const hideWhenIdle = () => {
      if (chromeInUse(pointerOverChrome)) {
        timer = setTimeout(hideWhenIdle, IDLE_MS);
        return;
      }
      timer = null;
      // Focus moves before the chrome goes: moving it counts as input, and
      // the hiding that follows has the last word.
      if (inChrome(document.activeElement)) {
        focusStageMonitor();
      }
      setHidden(true);
    };

    const onPointerMove = (event: PointerEvent) => {
      // Only a new position is the viewer's doing. An engine may report a
      // move when what lies under a resting pointer changes, as it does when
      // the chrome hides.
      if (
        pointerAt !== null &&
        pointerAt.x === event.screenX &&
        pointerAt.y === event.screenY
      ) {
        return;
      }
      pointerAt = { x: event.screenX, y: event.screenY };
      wake();
    };
    const onPointerOver = (event: PointerEvent) => {
      pointerOverChrome = inChrome(event.target);
    };
    const onPointerOut = (event: PointerEvent) => {
      // No next element: the pointer left the window.
      if (event.relatedTarget === null) {
        pointerOverChrome = false;
      }
    };
    const onPointerUp = (event: PointerEvent) => {
      if (
        event.target instanceof HTMLInputElement &&
        event.target.type === "range" &&
        inChrome(event.target)
      ) {
        focusStageMonitor();
      }
    };
    const onChange = (event: Event) => {
      wake();
      if (event.target instanceof HTMLSelectElement && inChrome(event.target)) {
        focusStageMonitor();
      }
    };

    document.addEventListener("pointermove", onPointerMove);
    document.addEventListener("pointerdown", wake);
    document.addEventListener("keydown", wake);
    document.addEventListener("focusin", wake);
    document.addEventListener("change", onChange);
    document.addEventListener("pointerover", onPointerOver);
    document.addEventListener("pointerout", onPointerOut);
    document.addEventListener("pointerup", onPointerUp);
    wake();
    return () => {
      document.removeEventListener("pointermove", onPointerMove);
      document.removeEventListener("pointerdown", wake);
      document.removeEventListener("keydown", wake);
      document.removeEventListener("focusin", wake);
      document.removeEventListener("change", onChange);
      document.removeEventListener("pointerover", onPointerOver);
      document.removeEventListener("pointerout", onPointerOut);
      document.removeEventListener("pointerup", onPointerUp);
      if (timer !== null) {
        clearTimeout(timer);
      }
      setHidden(false);
    };
  }, [armed]);

  return armed && hidden ? "hidden" : "shown";
}

function inChrome(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(STAGE_CHROME) !== null;
}

/** Whether hiding the chrome now would take something away from the viewer. */
function chromeInUse(pointerOverChrome: boolean): boolean {
  const focused = document.activeElement;
  return (
    pointerOverChrome ||
    (focused !== null && focused === stageSearchInput()) ||
    document.querySelector(OPEN_POPUP) !== null ||
    document.querySelector('input[type="range"]:active') !== null ||
    (focused instanceof HTMLSelectElement &&
      inChrome(focused) &&
      pickerOpen(focused))
  );
}

/**
 * A native picker receives no page events while open, and hiding its select
 * would dismiss it. Where the platform cannot say whether it is open, a
 * focused select counts as open.
 */
function pickerOpen(picker: HTMLSelectElement): boolean {
  return (
    typeof CSS === "undefined" ||
    !CSS.supports("selector(:open)") ||
    picker.matches(":open")
  );
}
