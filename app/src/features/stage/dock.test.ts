import { describe, expect, it } from "vitest";
import { nextDock, pictureFollows } from "./dock";
import type { StagePicture } from "./stage-chrome";

describe("nextDock", () => {
  it.each([
    { mode: "guide", previous: false, follows: true, docked: true },
    { mode: "guide", previous: true, follows: true, docked: true },
    { mode: "watch", previous: true, follows: true, docked: false },
    { mode: "watch", previous: false, follows: true, docked: false },
    // A picture that cannot follow keeps the box it has, whatever the mode.
    { mode: "guide", previous: false, follows: false, docked: false },
    { mode: "guide", previous: true, follows: false, docked: true },
    { mode: "watch", previous: true, follows: false, docked: true },
    { mode: "watch", previous: false, follows: false, docked: false },
  ] as const)(
    "in $mode mode, docked $previous, following $follows: docked $docked",
    ({ mode, previous, follows, docked }) => {
      expect(nextDock({ mode, previous, follows })).toBe(docked);
    },
  );
});

describe("pictureFollows", () => {
  it("always follows where the page draws the picture, and where there is no player", () => {
    expect(pictureFollows(true, picture("paused"))).toBe(true);
    expect(pictureFollows(true, picture("failed"))).toBe(true);
    expect(pictureFollows(true, null)).toBe(true);
    expect(pictureFollows(false, null)).toBe(true);
  });

  it.each([
    { state: "starting", follows: true },
    { state: "playing", follows: true },
    { state: "autoplay-blocked", follows: true },
    { state: "suspending", follows: false },
    { state: "paused", follows: false },
    { state: "recovering", follows: false },
    { state: "stopping", follows: false },
    { state: "failed", follows: false },
  ] as const)(
    "native video that is $state follows: $follows",
    ({ state, follows }) => {
      expect(pictureFollows(false, picture(state))).toBe(follows);
    },
  );
});

function picture(state: StagePicture["state"]): StagePicture {
  return { state, status: "", external: false, silent: false };
}
