import { createContext, useContext } from "react";
import type { PlayerState } from "../playback/playback-presentation";

/** What the player tells the shell about the picture it is showing. */
export interface StagePicture {
  /** Where playback stands; a picture is moving only while this is "playing". */
  readonly state: PlayerState["_tag"];
  /** The short name of that state as the player words it, such as "Paused". */
  readonly status: string;
  /** The picture is in a separate window (mpv), not in the page. */
  readonly external: boolean;
}

/** The shell's side of the player chrome: where controls go and how they look. */
export interface StageChrome {
  /** Element the player portals its controls group into; null → render controls inline. */
  readonly controlsSlot: HTMLElement | null;
  /** "compact": icon controls + "More" menu. "bar": every control inline with text. */
  readonly controls: "compact" | "bar";
  /** Element to make fullscreen; null → the player section (always on Android). */
  readonly fullscreenTarget: HTMLElement | null;
  /**
   * The side of its button the "More" menu opens on. "top" is for controls
   * that lie over the picture. "bottom" is for controls under a picture that
   * nothing may cover: the menu then keeps to the room under the picture
   * (`roomUnderPicture`), whichever way it has to turn to fit.
   */
  readonly menuSide: "top" | "bottom";
  /**
   * The player reports the state of its picture and whether it is in an
   * external window; null when the player goes away. Must be referentially
   * stable, because the player reports from an effect that depends on it.
   */
  readonly reportPicture: (picture: StagePicture | null) => void;
}

/** How the player presents its controls at one moment. */
export interface ControlPresentation {
  readonly variant: StageChrome["controls"];
  /** Element the controls group is portalled into; null → inline in the player section. */
  readonly slot: HTMLElement | null;
}

const STANDALONE_CHROME: StageChrome = {
  controlsSlot: null,
  controls: "bar",
  fullscreenTarget: null,
  menuSide: "top",
  reportPicture: () => undefined,
};

const StageChromeContext = createContext<StageChrome>(STANDALONE_CHROME);

/** Supplies the chrome to the player below it. Without one the player stands alone: inline bar controls, fullscreen on its own section. */
export const StageChromeProvider = StageChromeContext.Provider;

/** Reads the chrome the nearest shell provides, or the standalone default. */
export function useStageChrome(): StageChrome {
  return useContext(StageChromeContext);
}

/**
 * Chooses how the controls are presented. They follow the chrome, except
 * while the player section itself is the fullscreen element: only its own
 * subtree is drawn then, so the controls are the inline bar whatever the
 * chrome says. `sectionFullscreen` is what the document reports, not what
 * the chrome's target implies: the target can change under a section that is
 * still fullscreen.
 */
export function controlPresentation(
  chrome: Pick<StageChrome, "controls" | "controlsSlot">,
  sectionFullscreen: boolean,
): ControlPresentation {
  return sectionFullscreen
    ? { variant: "bar", slot: null }
    : { variant: chrome.controls, slot: chrome.controlsSlot };
}

/**
 * Chooses the element a press on Full screen acts on. While the player is
 * fullscreen that is the element the document has fullscreen, whatever the
 * chrome's target is by now, so the press leaves it. Otherwise it is the
 * chrome's target, or the player section without one.
 */
export function fullscreenToggleTarget({
  fullscreen,
  fullscreenElement,
  target,
  section,
}: {
  /** The player's picture is fullscreen. */
  readonly fullscreen: boolean;
  /** The document's fullscreen element. */
  readonly fullscreenElement: Element | null;
  /** The chrome's `fullscreenTarget`. */
  readonly target: HTMLElement | null;
  readonly section: HTMLElement | null;
}): HTMLElement | null {
  return fullscreen && fullscreenElement instanceof HTMLElement
    ? fullscreenElement
    : (target ?? section);
}

/** A box in the window, in CSS pixels from the window's top left corner. */
export interface WindowBox {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/**
 * The part of the window under the picture's bottom edge, across the
 * window's whole width. A menu held to it cannot cover the picture, wherever
 * the picture sits above its controls.
 */
export function roomUnderPicture(
  pictureBottom: number,
  viewport: { readonly width: number; readonly height: number },
): WindowBox {
  const top = Math.min(Math.max(pictureBottom, 0), viewport.height);
  return { x: 0, y: top, width: viewport.width, height: viewport.height - top };
}

/** Reports whether two picture reports say the same thing. */
export function samePicture(
  left: StagePicture | null,
  right: StagePicture | null,
): boolean {
  return left === null || right === null
    ? left === right
    : left.state === right.state &&
        left.status === right.status &&
        left.external === right.external;
}
