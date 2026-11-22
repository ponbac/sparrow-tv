interface AndroidFullscreenPort {
  postMessage(message: "enter" | "exit"): void;
}

declare global {
  interface Window {
    readonly sparrowFullscreen?: AndroidFullscreenPort;
  }
}

const TV_FULLSCREEN_CHANGE = "sparrow-tv-fullscreenchange";
let televisionSurface: HTMLElement | null = null;

/** The surface filling the window, including an installed TV's top-layer player. */
export function fullscreenElement(): Element | null {
  return televisionSurface ?? document.fullscreenElement ?? null;
}

/** Observes player presentation changes, including browser-initiated fullscreen exits. */
export function subscribeToFullscreen(listener: () => void): () => void {
  document.addEventListener("fullscreenchange", listener);
  document.addEventListener(TV_FULLSCREEN_CHANGE, listener);
  return () => {
    document.removeEventListener("fullscreenchange", listener);
    document.removeEventListener(TV_FULLSCREEN_CHANGE, listener);
  };
}

/**
 * Fills the TV window without asking the WebView to recompute the Guide.
 * Other surfaces and older WebViews use ordinary document fullscreen.
 */
export async function enterPlaybackFullscreen(surface: HTMLElement): Promise<void> {
  if (surface === televisionSurface) return;
  if (
    window.sparrowFullscreen &&
    document.documentElement.hasAttribute("data-television") &&
    surface.matches(".hosted-player") &&
    typeof surface.showPopover === "function" &&
    fullscreenElement() == null
  ) {
    surface.setAttribute("popover", "manual");
    try {
      surface.showPopover();
      if (surface.matches(":popover-open")) {
        window.sparrowFullscreen.postMessage("enter");
        televisionSurface = surface;
        surface.addEventListener("toggle", followPopover);
        window.addEventListener("pagehide", leaveWindow);
        document.dispatchEvent(new Event(TV_FULLSCREEN_CHANGE));
        return;
      }
    } catch {
      // A WebView can expose the API while refusing a top-layer presentation.
    }
    surface.removeAttribute("popover");
  }
  await surface.requestFullscreen();
}

/** Leaves the current presentation and restores the native picture's dock coordinates. */
export async function exitPlaybackFullscreen(): Promise<void> {
  if (televisionSurface !== null) releaseTelevisionFullscreen(televisionSurface);
  else await document.exitFullscreen();
}

/** Clears the TV presentation when its surface leaves the tree or playback exits fullscreen. */
export function releaseTelevisionFullscreen(surface: HTMLElement | null): void {
  if (surface === null || surface !== televisionSurface) return;
  televisionSurface = null;
  surface.removeEventListener("toggle", followPopover);
  window.removeEventListener("pagehide", leaveWindow);
  // Removing the attribute also dismisses an open manual popover.
  surface.removeAttribute("popover");
  window.sparrowFullscreen?.postMessage(document.fullscreenElement ? "enter" : "exit");
  document.dispatchEvent(new Event(TV_FULLSCREEN_CHANGE));
}

function followPopover(event: Event): void {
  const surface = televisionSurface;
  if (surface !== null && event.target === surface && !surface.matches(":popover-open")) {
    releaseTelevisionFullscreen(surface);
  }
}

function leaveWindow(): void {
  releaseTelevisionFullscreen(televisionSurface);
}
