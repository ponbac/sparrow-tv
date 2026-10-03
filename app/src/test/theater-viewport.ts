import { vi } from "vitest";

/** Changes the size the stubbed window reports to the shell. */
export interface ViewportStub {
  /** Reports a window that is, or is not, large enough for the Theater layout. */
  resize(theater: boolean): void;
}

/**
 * Gives jsdom a `matchMedia`, which it lacks: without one the shell is always
 * pocket. Every media query answers `theater`, and `resize` flips the answer
 * and tells the listeners. Call `vi.unstubAllGlobals()` after the test.
 */
export function stubViewport(theater: boolean): ViewportStub {
  let matches = theater;
  const listeners = new Set<EventListenerOrEventListenerObject>();
  vi.stubGlobal("matchMedia", (media: string): MediaQueryList => ({
    get matches() {
      return matches;
    },
    media,
    onchange: null,
    addListener: () => undefined,
    removeListener: () => undefined,
    addEventListener: (
      _type: string,
      listener: EventListenerOrEventListenerObject,
    ) => {
      listeners.add(listener);
    },
    removeEventListener: (
      _type: string,
      listener: EventListenerOrEventListenerObject,
    ) => {
      listeners.delete(listener);
    },
    dispatchEvent: () => false,
  }));
  return {
    resize(next) {
      matches = next;
      const change = new Event("change");
      for (const listener of listeners) {
        if (typeof listener === "function") {
          listener(change);
        } else {
          listener.handleEvent(change);
        }
      }
    },
  };
}
