import { QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { createSparrowQueryClient } from "../client/query-client";
import { CatalogBrowser } from "../features/catalog/catalog-browser";
import { prefetchTelevisionGuide } from "../features/catalog/use-guide-catalog";
import type { InstalledPlaybackEngine } from "../features/playback/installed-playback-engine";
import { BenchSparrowClient } from "./bench-client";
import "@fontsource-variable/saira/wdth.css";
import "../index.css";

// Stands in for Media3 or mpegts.js: a decoded clip loops in the real video element.
const engine: InstalledPlaybackEngine = {
  start: (request) => {
    const video = request.video;
    video.muted = true;
    video.loop = true;
    video.src = `${import.meta.env.BASE_URL}bench-clip.webm`;
    const playing = () => request.onPlaying();
    video.addEventListener("playing", playing, { once: true });
    void video.play().catch(() => request.onAutoplayBlocked());
    return {
      stop: () => {
        video.removeEventListener("playing", playing);
        video.removeAttribute("src");
        video.load();
      },
    };
  },
};

// `?tv=0` shows the desktop and phone layouts instead of the TV's.
const television = new URLSearchParams(location.search).get("tv") !== "0";
if (!television) document.documentElement.removeAttribute("data-television");
const client = new BenchSparrowClient({ pictureOverlay: !television });
const queryClient = createSparrowQueryClient("installed");
void prefetchTelevisionGuide(queryClient, client);
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}>
    <CatalogBrowser client={client} runtime="installed" playbackEngine={engine} />
  </QueryClientProvider>,
);
