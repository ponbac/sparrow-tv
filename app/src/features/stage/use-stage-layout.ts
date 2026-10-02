import { skipToken, useQuery } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";
import type { SparrowClient } from "../../client/contracts";
import { successfulQueryResult } from "../../client/query-result";

/** A window this large has room to lay the chrome over the picture. */
const THEATER_VIEWPORT = "(min-width: 1051px) and (min-height: 601px)";

/**
 * How the shell arranges picture, info and guide. Theater lays the chrome over
 * a picture that fills the window; pocket keeps everything clear of the picture.
 */
export type StageLayout = "theater" | "pocket";

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
 * Chooses the layout for the current window. Theater needs the overlay
 * capability and either a large enough window or a fullscreen document root;
 * anything else, including a platform without media queries, is pocket.
 */
export function useStageLayout(pictureOverlay: boolean): StageLayout {
  const roomy = useSyncExternalStore(subscribeToViewport, hasTheaterViewport);
  return pictureOverlay && roomy ? "theater" : "pocket";
}

function subscribeToViewport(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") {
    return () => undefined;
  }
  const viewport = window.matchMedia(THEATER_VIEWPORT);
  viewport.addEventListener("change", onChange);
  document.addEventListener("fullscreenchange", onChange);
  return () => {
    viewport.removeEventListener("change", onChange);
    document.removeEventListener("fullscreenchange", onChange);
  };
}

function hasTheaterViewport(): boolean {
  if (typeof window.matchMedia !== "function") {
    return false;
  }
  return (
    window.matchMedia(THEATER_VIEWPORT).matches ||
    document.fullscreenElement === document.documentElement
  );
}
