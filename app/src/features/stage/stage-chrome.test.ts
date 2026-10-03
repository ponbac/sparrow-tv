import { describe, expect, it } from "vitest";
import {
  controlPresentation,
  fullscreenToggleTarget,
  roomUnderPicture,
  samePicture,
  type StagePicture,
} from "./stage-chrome";

describe("controlPresentation", () => {
  const slot = document.createElement("div");

  it("follows the chrome while the player section is not the fullscreen element", () => {
    expect(
      controlPresentation({ controls: "compact", controlsSlot: slot }, false),
    ).toEqual({ variant: "compact", slot });
    expect(
      controlPresentation({ controls: "bar", controlsSlot: null }, false),
    ).toEqual({ variant: "bar", slot: null });
  });

  it("is the inline bar while the player section is the fullscreen element", () => {
    expect(
      controlPresentation({ controls: "compact", controlsSlot: slot }, true),
    ).toEqual({ variant: "bar", slot: null });
  });
});

describe("fullscreenToggleTarget", () => {
  const root = document.createElement("div");
  const section = document.createElement("section");

  it("enters fullscreen on the chrome's target, or on the player section without one", () => {
    expect(
      fullscreenToggleTarget({
        fullscreen: false,
        fullscreenElement: null,
        target: root,
        section,
      }),
    ).toBe(root);
    expect(
      fullscreenToggleTarget({
        fullscreen: false,
        fullscreenElement: null,
        target: null,
        section,
      }),
    ).toBe(section);
  });

  it("leaves the element that is fullscreen, whatever the target is by now", () => {
    // The section went fullscreen before the shell had a target of its own.
    expect(
      fullscreenToggleTarget({
        fullscreen: true,
        fullscreenElement: section,
        target: root,
        section,
      }),
    ).toBe(section);
    expect(
      fullscreenToggleTarget({
        fullscreen: true,
        fullscreenElement: root,
        target: null,
        section,
      }),
    ).toBe(root);
  });

  it("keeps to the target while the picture is fullscreen outside the document", () => {
    // mpv's own window is fullscreen: the document has nothing to leave.
    expect(
      fullscreenToggleTarget({
        fullscreen: true,
        fullscreenElement: null,
        target: root,
        section,
      }),
    ).toBe(root);
  });

  it("has nothing to act on before the player section is in the document", () => {
    expect(
      fullscreenToggleTarget({
        fullscreen: false,
        fullscreenElement: null,
        target: null,
        section: null,
      }),
    ).toBeNull();
  });
});

describe("roomUnderPicture", () => {
  const phone = { width: 360, height: 740 };

  it("is the window from the picture's bottom edge down, across its width", () => {
    expect(roomUnderPicture(240.5, phone)).toEqual({
      x: 0,
      y: 240.5,
      width: 360,
      height: 499.5,
    });
  });

  it("stays inside the window whatever the picture's box says", () => {
    expect(roomUnderPicture(-20, phone)).toEqual({
      x: 0,
      y: 0,
      width: 360,
      height: 740,
    });
    expect(roomUnderPicture(900, phone)).toEqual({
      x: 0,
      y: 740,
      width: 360,
      height: 0,
    });
  });
});

describe("samePicture", () => {
  const playing: StagePicture = {
    state: "playing",
    status: "On air",
    external: false,
    silent: false,
  };

  it("matches reports that say the same thing", () => {
    expect(samePicture(playing, { ...playing })).toBe(true);
    expect(samePicture(null, null)).toBe(true);
  });

  it("tells apart a change of state, of its wording, or of where the picture is", () => {
    expect(
      samePicture(playing, { ...playing, state: "paused", status: "Paused" }),
    ).toBe(false);
    expect(samePicture(playing, { ...playing, status: "Signal lost" })).toBe(
      false,
    );
    expect(samePicture(playing, { ...playing, external: true })).toBe(false);
    expect(samePicture(playing, { ...playing, silent: true })).toBe(false);
    expect(samePicture(playing, null)).toBe(false);
    expect(samePicture(null, playing)).toBe(false);
  });
});
