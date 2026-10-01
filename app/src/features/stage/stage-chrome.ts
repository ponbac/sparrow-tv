import { createContext, useContext } from "react";

/** What the player tells the shell about the picture it is showing. */
export interface StagePicture {
  readonly playing: boolean;
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
   * The player reports whether a picture is playing and whether it is in an
   * external window; null when the player goes away. Must be referentially
   * stable, because the player reports from an effect that depends on it.
   */
  readonly reportPicture: (picture: StagePicture | null) => void;
}

const STANDALONE_CHROME: StageChrome = {
  controlsSlot: null,
  controls: "bar",
  fullscreenTarget: null,
  reportPicture: () => undefined,
};

const StageChromeContext = createContext<StageChrome>(STANDALONE_CHROME);

/** Supplies the chrome to the player below it. Without one the player stands alone: inline bar controls, fullscreen on its own section. */
export const StageChromeProvider = StageChromeContext.Provider;

/** Reads the chrome the nearest shell provides, or the standalone default. */
export function useStageChrome(): StageChrome {
  return useContext(StageChromeContext);
}
