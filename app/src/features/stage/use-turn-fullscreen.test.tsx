import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  useTurnFullscreen,
  type TurnFullscreenPicture,
} from "./use-turn-fullscreen";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(screen, "orientation");
  Reflect.deleteProperty(document, "fullscreenElement");
  Reflect.deleteProperty(document, "exitFullscreen");
});

describe("useTurnFullscreen", () => {
  it("takes a watched picture fullscreen on a turn to the side and back out on the turn upright", () => {
    const phone = renderPhone({ watching: true });

    phone.turn("landscape-primary");
    expect(phone.press).toHaveBeenCalledTimes(1);
    phone.documentFullscreen(true);

    // From one side to the other is still on its side.
    phone.turn("landscape-secondary");
    expect(phone.press).toHaveBeenCalledTimes(1);
    expect(phone.exit).not.toHaveBeenCalled();

    phone.turn("portrait-primary");
    expect(phone.exit).toHaveBeenCalledTimes(1);
  });

  it("only turns the layout while the viewer is not watching a live picture", () => {
    const phone = renderPhone({ watching: false });

    phone.turn("landscape-primary");
    expect(phone.press).not.toHaveBeenCalled();

    // What counts is the moment of the turn.
    phone.turn("portrait-primary");
    phone.picture({ watching: true, follows: true });
    phone.turn("landscape-primary");
    expect(phone.press).toHaveBeenCalledTimes(1);
  });

  it("keeps the turn's fullscreen while the picture cannot follow, and ends it once it can", () => {
    const phone = renderPhone({ watching: true });
    phone.turn("landscape-primary");
    phone.documentFullscreen(true);

    // Paused: the native picture stays where fullscreen put it.
    phone.picture({ watching: false, follows: false });
    phone.turn("portrait-primary");
    expect(phone.exit).not.toHaveBeenCalled();

    phone.picture({ watching: false, follows: true });
    expect(phone.exit).toHaveBeenCalledTimes(1);
  });

  it("leaves a window alone on a screen that is not held in the hand", () => {
    const desk = renderPhone({ watching: true, handheld: false });

    // A window moved to a monitor that stands the other way.
    desk.turn("landscape-primary");
    expect(desk.press).not.toHaveBeenCalled();
  });

  it("leaves fullscreen the viewer asked for themselves alone", () => {
    const phone = renderPhone({ watching: true });
    phone.documentFullscreen(true);

    phone.turn("landscape-primary");
    expect(phone.press).not.toHaveBeenCalled();
    phone.turn("portrait-primary");
    expect(phone.exit).not.toHaveBeenCalled();
  });

  it("does not return to fullscreen the viewer left while the phone stays on its side", () => {
    const phone = renderPhone({ watching: true });
    phone.turn("landscape-primary");
    phone.documentFullscreen(true);
    phone.documentFullscreen(false);

    phone.turn("landscape-secondary");
    expect(phone.press).toHaveBeenCalledTimes(1);

    // Upright, the turn has nothing of its own left to end.
    phone.documentFullscreen(true);
    phone.turn("portrait-primary");
    expect(phone.exit).not.toHaveBeenCalled();
  });

  it("forgets a request the document refused", () => {
    const phone = renderPhone({ watching: true });
    phone.turn("landscape-primary");
    document.dispatchEvent(new Event("fullscreenerror"));

    // Fullscreen the viewer then starts is not the turn's to end.
    phone.documentFullscreen(true);
    phone.turn("portrait-primary");
    expect(phone.exit).not.toHaveBeenCalled();
  });

  it("does nothing where the screen reports no orientation", () => {
    const press = vi.fn();
    render(<Phone watching follows press={press} />);
    expect(press).not.toHaveBeenCalled();
  });
});

function Phone({
  watching,
  follows,
  press,
}: TurnFullscreenPicture & { readonly press: () => void }) {
  useTurnFullscreen({ watching, follows });
  return (
    <button type="button" data-stage-action="fullscreen" onClick={press}>
      Full screen
    </button>
  );
}

function renderPhone({
  watching,
  handheld = true,
}: {
  readonly watching: boolean;
  readonly handheld?: boolean;
}) {
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: media === "(pointer: coarse)" && handheld,
  }));
  const orientation = Object.assign(new EventTarget(), {
    type: "portrait-primary",
  });
  Object.defineProperty(screen, "orientation", {
    configurable: true,
    value: orientation,
  });
  let fullscreenElement: Element | null = null;
  Object.defineProperty(document, "fullscreenElement", {
    configurable: true,
    get: () => fullscreenElement,
  });
  const exit = vi.fn(async () => undefined);
  Object.defineProperty(document, "exitFullscreen", {
    configurable: true,
    value: exit,
  });
  const press = vi.fn();
  const view = render(<Phone watching={watching} follows press={press} />);
  return {
    press,
    exit,
    turn(type: OrientationType) {
      orientation.type = type;
      orientation.dispatchEvent(new Event("change"));
    },
    /** The shell's picture changes: what is watched, or whether it follows. */
    picture(next: TurnFullscreenPicture) {
      view.rerender(<Phone {...next} press={press} />);
    },
    /** The document enters or leaves fullscreen and says so. */
    documentFullscreen(fullscreen: boolean) {
      fullscreenElement = fullscreen ? document.documentElement : null;
      document.dispatchEvent(new Event("fullscreenchange"));
    },
  };
}
