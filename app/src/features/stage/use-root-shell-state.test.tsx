import { render, renderHook } from "@testing-library/react";
import { useLayoutEffect } from "react";
import { describe, expect, it } from "vitest";
import { useRootShellState, type RootShellState } from "./use-root-shell-state";

const shellAttributes = () =>
  Object.fromEntries(
    Array.from(document.documentElement.attributes)
      .filter((attribute) => attribute.name.startsWith("data-shell"))
      .map((attribute) => [attribute.name, attribute.value]),
  );

describe("useRootShellState", () => {
  it("mirrors the shell onto the root, clears it while loading and on unmount", () => {
    const docked: RootShellState = { layout: "pocket", pocket: "television", playing: true, dock: true, retained: false };
    const view = renderHook<void, RootShellState | null>((state) => useRootShellState(state), { initialProps: docked });
    expect(shellAttributes()).toEqual({
      "data-shell": "",
      "data-shell-layout": "pocket",
      "data-shell-pocket": "television",
      "data-shell-playing": "true",
      "data-shell-dock": "true",
    });

    view.rerender({ layout: "theater", pocket: null, playing: false, dock: false, retained: true });
    expect(shellAttributes()).toEqual({
      "data-shell": "",
      "data-shell-layout": "theater",
      "data-shell-playing": "false",
      "data-shell-dock": "false",
      "data-shell-retained": "",
    });

    view.rerender(null);
    expect(shellAttributes()).toEqual({});

    view.rerender(docked);
    view.unmount();
    expect(shellAttributes()).toEqual({});
  });

  it("updates the root before child layout effects measure the new layout", () => {
    // The Guide measures its board in a layout effect, which runs before the
    // shell's own; a mirror set later would let it measure the old layout.
    const seen: (string | null)[] = [];
    function Measuring({ dock }: { readonly dock: boolean }) {
      useLayoutEffect(() => {
        seen.push(document.documentElement.getAttribute("data-shell-dock"));
      }, [dock]);
      return null;
    }
    function Shell({ dock }: { readonly dock: boolean }) {
      useRootShellState({ layout: "pocket", pocket: "stacked", playing: true, dock, retained: false });
      return <Measuring dock={dock} />;
    }
    const view = render(<Shell dock={false} />);
    view.rerender(<Shell dock />);

    expect(seen).toEqual(["false", "true"]);
  });
});
