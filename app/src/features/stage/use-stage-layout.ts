import { skipToken, useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";
import type { SparrowClient } from "../../client/contracts";
import { successfulQueryResult } from "../../client/query-result";

/** A window this large has room to lay the chrome over the picture. */
const THEATER_VIEWPORT = "(min-width: 1051px) and (min-height: 601px)";

/**
 * How the shell arranges picture, info and guide. Theater lays the chrome over
 * a picture that fills the window; stacked keeps everything below the picture.
 */
export type StageLayout = "theater" | "stacked";

/**
 * Reads whether page content may be drawn over the picture. Hosted always
 * may and reads nothing. Installed asks its shell once; until that answers,
 * and when it fails, nothing may cover the picture.
 */
export function usePictureOverlay(
  runtime: "hosted" | "installed",
  client: Pick<SparrowClient, "capabilities">,
): boolean {
  const capabilities = useQuery({
    queryKey: ["capabilities"],
    queryFn:
      runtime === "installed"
        ? ({ signal }) => successfulQueryResult(client.capabilities({ signal }))
        : skipToken,
    retry: false,
    staleTime: Number.POSITIVE_INFINITY,
  });
  return (
    runtime === "hosted" || (capabilities.data?.value.pictureOverlay ?? false)
  );
}

/**
 * Chooses the layout for the current window. Theater always needs the overlay
 * capability and a roomy viewport, including in root fullscreen. Player-only
 * fullscreen stays stacked so its controls cannot be portalled outside it.
 * A platform without media queries is stacked.
 */
export function useStageLayout(pictureOverlay: boolean): StageLayout {
  const roomy = useSyncExternalStore(subscribeToViewport, hasTheaterViewport);
  const fullscreen = useSyncExternalStore(
    subscribeToFullscreen,
    fullscreenElement,
  );
  const shellVisible =
    fullscreen === null || fullscreen === document.documentElement;
  return pictureOverlay && roomy && shellVisible ? "theater" : "stacked";
}

/**
 * Theater enters root fullscreen; stacked enters player-only fullscreen (null).
 * Keep an already-fullscreen root as the target after shrinking, so the button
 * exits it. Once exited, the next request follows the current layout again.
 */
export function useStageFullscreenTarget(
  layout: StageLayout,
): HTMLElement | null {
  const fullscreen = useSyncExternalStore(
    subscribeToFullscreen,
    fullscreenElement,
  );
  return fullscreen === document.documentElement || layout === "theater"
    ? document.documentElement
    : null;
}

function subscribeToFullscreen(onChange: () => void): () => void {
  document.addEventListener("fullscreenchange", onChange);
  return () => document.removeEventListener("fullscreenchange", onChange);
}

function fullscreenElement(): Element | null {
  return document.fullscreenElement ?? null;
}

function subscribeToViewport(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") {
    return () => undefined;
  }
  const viewport = window.matchMedia(THEATER_VIEWPORT);
  viewport.addEventListener("change", onChange);
  return () => viewport.removeEventListener("change", onChange);
}

function hasTheaterViewport(): boolean {
  if (typeof window.matchMedia !== "function") {
    return false;
  }
  return window.matchMedia(THEATER_VIEWPORT).matches;
}
