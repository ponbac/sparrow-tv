import { useInsertionEffect } from "react";

/** The shell's state that page-wide rules read from the root element. */
export interface RootShellState {
  readonly layout: "theater" | "pocket";
  readonly pocket: "television" | "split" | "stacked" | null;
  readonly playing: boolean;
  readonly dock: boolean;
  readonly retained: boolean;
}

/**
 * Mirrors the shell's state onto `<html>`, so rules that style the whole page
 * by it select the root directly. A `:root:has(.shell…)` rule makes Chrome
 * restyle the whole document on every focus move and row mount.
 *
 * An insertion effect runs before every layout effect, as the shell's own
 * attributes did, so the Guide measures its board against the new layout.
 */
export function useRootShellState(state: RootShellState | null): void {
  const { layout, pocket, playing, dock, retained } = state ?? {};
  useInsertionEffect(() => {
    if (layout === undefined) return;
    const root = document.documentElement;
    const attributes: readonly (readonly [string, string | null])[] = [
      ["data-shell", ""],
      ["data-shell-layout", layout],
      ["data-shell-pocket", pocket ?? null],
      ["data-shell-playing", String(playing)],
      ["data-shell-dock", String(dock)],
      ["data-shell-retained", retained ? "" : null],
    ];
    for (const [name, value] of attributes) {
      if (value !== null) root.setAttribute(name, value);
    }
    return () => {
      for (const [name] of attributes) root.removeAttribute(name);
    };
  }, [layout, pocket, playing, dock, retained]);
}
