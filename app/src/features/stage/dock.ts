import type { StagePicture } from "./stage-chrome";

/** What decides the size of the picture's box in the pocket layout. */
export interface DockInput {
  /** The mode the shell shows. */
  readonly mode: "watch" | "guide";
  /** Whether the picture was docked before this render. */
  readonly previous: boolean;
  /** Whether the picture will follow its box to a new size; see `pictureFollows`. */
  readonly follows: boolean;
}

/**
 * Decides whether the picture is docked to the band. It docks in guide mode
 * and fills the width in watch mode, but only while it can follow its box:
 * otherwise it keeps the size it had.
 */
export function nextDock({ mode, previous, follows }: DockInput): boolean {
  return follows ? mode === "guide" : previous;
}

/**
 * Reports whether the picture follows its box when the box changes size.
 * A picture the page draws always does, and with no player there is nothing
 * to leave behind. Native Android video is only moved while a presentation
 * is live; in every other state it keeps its last rectangle in front of the
 * page.
 */
export function pictureFollows(
  pictureOverlay: boolean,
  picture: StagePicture | null,
): boolean {
  return pictureOverlay || picture === null || isLive(picture.state);
}

function isLive(state: StagePicture["state"]): boolean {
  switch (state) {
    case "starting":
    case "playing":
    case "autoplay-blocked":
      return true;
    case "suspending":
    case "paused":
    case "recovering":
    case "stopping":
    case "failed":
      return false;
  }
}
