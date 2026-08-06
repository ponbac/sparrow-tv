import { invoke } from "@tauri-apps/api/core";
import mpegts from "mpegts.js";
import "./styles.css";
import {
  exportableState,
  initialProbeState,
  redactSource,
  reduceProbe,
  type Candidate,
  type ProbeAction,
  type ProbeState,
} from "./player-state.ts";
import { observeNativeBytes, TauriStreamLoader } from "./tauri-stream-loader.ts";

interface MpvSnapshot {
  paused: boolean | null;
  timePosition: number | null;
  videoCodec: string | null;
  audioCodec: string | null;
  droppedFrames: number | null;
  estimatedFps: number | null;
}

type MpegtsPlayer = ReturnType<typeof mpegts.createPlayer>;
type MpegtsConfig = NonNullable<Parameters<typeof mpegts.createPlayer>[1]>;
type MpegtsMediaInfo = {
  videoCodec?: string;
  audioCodec?: string;
  width?: number;
  height?: number;
};
type MpegtsStatistics = {
  decodedFrames?: number;
  droppedFrames?: number;
  speed?: number;
};

const candidates: Array<{ id: Candidate; label: string }> = [
  { id: "direct-video", label: "W0 · direct video" },
  { id: "browser-mpegts", label: "W1 · browser mpegts.js" },
  { id: "tauri-mpegts", label: "N1 · Tauri HTTP mpegts.js" },
  { id: "mpv", label: "L1 · mpv Wayland" },
];

document.querySelector<HTMLDivElement>("#app")!.innerHTML = `
  <div class="shell">
    <div class="eyebrow">Throwaway prototype · Wayfinder playback proof</div>
    <h1>Direct live playback probe</h1>
    <p class="lede">Paste one direct channel URL. Nothing is persisted; exported evidence contains only the source label and redacted host.</p>
    <div class="grid">
      <section class="panel">
        <div class="controls">
          <div class="source-row">
            <label>Source label<input id="source-label" value="representative HD" autocomplete="off" /></label>
            <label>Direct stream URL<input id="source-url" type="password" placeholder="https://provider.example/live/…" autocomplete="off" /></label>
          </div>
          <div id="candidates" class="candidate-row"></div>
          <div class="action-row">
            <button id="start" class="primary">Start</button>
            <button id="pause">Pause / resume</button>
            <button id="restart">Restart</button>
            <button id="fullscreen">Fullscreen</button>
            <button id="sample">Sample now</button>
            <button id="stop">Stop</button>
          </div>
          <span id="runtime-status" class="status">Idle</span>
          <span class="hint">Use candidate-relative evidence. A WebView failure does not imply the feed is invalid; compare it with mpv and the later physical-device Media3 baseline.</span>
        </div>
        <div class="video-wrap">
          <video id="video" playsinline controls></video>
          <span class="watermark">PROTOTYPE</span>
        </div>
      </section>
      <aside class="panel">
        <div class="state-head"><strong>Complete probe state</strong><button id="export">Export redacted JSON</button></div>
        <pre id="state" class="state"></pre>
      </aside>
    </div>
  </div>
`;

const video = document.querySelector<HTMLVideoElement>("#video")!;
const sourceUrl = document.querySelector<HTMLInputElement>("#source-url")!;
const sourceLabel = document.querySelector<HTMLInputElement>("#source-label")!;
const stateView = document.querySelector<HTMLPreElement>("#state")!;
const runtimeStatus = document.querySelector<HTMLSpanElement>("#runtime-status")!;
const candidateRow = document.querySelector<HTMLDivElement>("#candidates")!;

let selectedCandidate: Candidate = "direct-video";
let state: ProbeState = initialProbeState;
let player: MpegtsPlayer | null = null;
let suppressVideoErrors = false;

function dispatch(action: ProbeAction): void {
  state = reduceProbe(state, action);
  stateView.textContent = JSON.stringify(exportableState(state), null, 2);
  runtimeStatus.textContent = `${state.status} · ${state.lastEvent ?? "no events"}`;
}

function selectCandidate(candidate: Candidate): void {
  selectedCandidate = candidate;
  for (const button of candidateRow.querySelectorAll<HTMLButtonElement>("button")) {
    button.dataset.active = String(button.dataset.candidate === candidate);
  }
}

for (const candidate of candidates) {
  const button = document.createElement("button");
  button.textContent = candidate.label;
  button.dataset.candidate = candidate.id;
  button.addEventListener("click", () => selectCandidate(candidate.id));
  candidateRow.append(button);
}
selectCandidate(selectedCandidate);
dispatch({ type: "status", status: "idle", detail: "ready" });

observeNativeBytes((bytes) => dispatch({ type: "bytes", bytes }));

video.addEventListener("playing", () => dispatch({ type: "first-frame" }));
video.addEventListener("pause", () => {
  if (!suppressVideoErrors && state.status === "playing") dispatch({ type: "status", status: "paused" });
});
video.addEventListener("waiting", () => {
  if (!suppressVideoErrors) dispatch({ type: "status", status: "stalled", detail: "video waiting" });
});
video.addEventListener("stalled", () => {
  if (!suppressVideoErrors) dispatch({ type: "status", status: "stalled", detail: "media stalled" });
});
video.addEventListener("error", () => {
  if (!suppressVideoErrors) {
    const code = video.error?.code ?? "unknown";
    dispatch({ type: "error", detail: `HTML media error ${code}` });
  }
});

async function cleanup(markStopped: boolean): Promise<void> {
  suppressVideoErrors = true;
  if (player) {
    player.destroy();
    player = null;
  }
  video.pause();
  video.removeAttribute("src");
  video.load();
  try {
    await invoke("mpv_stop");
  } catch {
    // Mobile and a stopped desktop player both have nothing to clean up.
  }
  queueMicrotask(() => { suppressVideoErrors = false; });
  if (markStopped) dispatch({ type: "stop" });
}

function mpegtsConfig(nativeTransport: boolean): MpegtsConfig {
  return {
    isLive: true,
    enableWorker: false,
    enableStashBuffer: true,
    autoCleanupSourceBuffer: true,
    autoCleanupMaxBackwardDuration: 60,
    autoCleanupMinBackwardDuration: 30,
    ...(nativeTransport
      ? { customLoader: TauriStreamLoader as unknown as NonNullable<MpegtsConfig["customLoader"]> }
      : {}),
  };
}

function wireMpegts(nextPlayer: MpegtsPlayer): void {
  nextPlayer.on(mpegts.Events.MEDIA_INFO, (info: MpegtsMediaInfo) => {
    dispatch({
      type: "media-info",
      videoCodec: info.videoCodec,
      audioCodec: info.audioCodec,
      resolution: info.width && info.height ? `${info.width}×${info.height}` : undefined,
    });
  });
  nextPlayer.on(mpegts.Events.STATISTICS_INFO, (statistics: MpegtsStatistics) => {
    dispatch({
      type: "sample",
      decodedFrames: statistics.decodedFrames,
      droppedFrames: statistics.droppedFrames,
      speedKbps: statistics.speed,
      ...videoSample(),
    });
  });
  nextPlayer.on(mpegts.Events.ERROR, (errorType: string, errorDetail: string, errorInfo: unknown) => {
    dispatch({ type: "error", detail: `${errorType} · ${errorDetail} · ${JSON.stringify(errorInfo)}` });
  });
}

async function startCandidate(isRestart = false): Promise<void> {
  const url = sourceUrl.value.trim();
  if (!/^https?:\/\//i.test(url)) {
    dispatch({ type: "error", detail: "enter a direct HTTP or HTTPS stream URL" });
    return;
  }

  if (isRestart) dispatch({ type: "restart", detail: "manual restart" });
  await cleanup(false);
  dispatch({
    type: "start",
    candidate: selectedCandidate,
    source: url,
    sourceLabel: `${sourceLabel.value.trim() || "unnamed feed"} · ${redactSource(url)}`,
  });

  try {
    if (selectedCandidate === "direct-video") {
      video.src = url;
      await video.play();
      return;
    }

    if (selectedCandidate === "mpv") {
      const snapshot = await invoke<MpvSnapshot>("mpv_start", { url });
      applyMpvSnapshot(snapshot);
      dispatch({ type: "status", status: "starting", detail: "mpv loaded; waiting for time position" });
      return;
    }

    const features = mpegts.getFeatureList();
    if (!features.mseLivePlayback) throw new Error("this WebView does not expose MSE live playback");
    player = mpegts.createPlayer(
      { type: "mpegts", url, isLive: true },
      mpegtsConfig(selectedCandidate === "tauri-mpegts"),
    );
    wireMpegts(player);
    player.attachMediaElement(video);
    player.load();
    await player.play();
  } catch (error) {
    dispatch({ type: "error", detail: error instanceof Error ? error.message : String(error) });
  }
}

function videoSample(): Partial<Extract<ProbeAction, { type: "sample" }>> {
  const quality = video.getVideoPlaybackQuality?.();
  const end = video.buffered.length ? video.buffered.end(video.buffered.length - 1) : video.currentTime;
  return {
    currentTime: Number.isFinite(video.currentTime) ? video.currentTime : undefined,
    bufferedSeconds: Math.max(0, end - video.currentTime),
    decodedFrames: quality?.totalVideoFrames,
    droppedFrames: quality?.droppedVideoFrames,
  };
}

function applyMpvSnapshot(snapshot: MpvSnapshot): void {
  if (snapshot.videoCodec || snapshot.audioCodec) {
    dispatch({ type: "media-info", videoCodec: snapshot.videoCodec ?? undefined, audioCodec: snapshot.audioCodec ?? undefined });
  }
  if (snapshot.timePosition !== null && snapshot.timePosition >= 0) dispatch({ type: "first-frame" });
  dispatch({
    type: "sample",
    currentTime: snapshot.timePosition ?? undefined,
    droppedFrames: snapshot.droppedFrames ?? undefined,
  });
  if (snapshot.paused !== null) dispatch({ type: "status", status: snapshot.paused ? "paused" : "playing" });
}

async function sampleNow(): Promise<void> {
  try {
    if (state.candidate === "mpv") {
      applyMpvSnapshot(await invoke<MpvSnapshot>("mpv_snapshot"));
    } else {
      dispatch({ type: "sample", ...videoSample() });
    }
  } catch (error) {
    dispatch({ type: "error", detail: error instanceof Error ? error.message : String(error) });
  }
}

document.querySelector<HTMLButtonElement>("#start")!.addEventListener("click", () => void startCandidate());
document.querySelector<HTMLButtonElement>("#restart")!.addEventListener("click", () => void startCandidate(true));
document.querySelector<HTMLButtonElement>("#stop")!.addEventListener("click", () => void cleanup(true));
document.querySelector<HTMLButtonElement>("#sample")!.addEventListener("click", () => void sampleNow());
document.querySelector<HTMLButtonElement>("#pause")!.addEventListener("click", async () => {
  try {
    if (state.candidate === "mpv") {
      applyMpvSnapshot(await invoke<MpvSnapshot>("mpv_command", { command: "pause" }));
    } else if (video.paused) {
      await video.play();
    } else {
      video.pause();
    }
  } catch (error) {
    dispatch({ type: "error", detail: error instanceof Error ? error.message : String(error) });
  }
});
document.querySelector<HTMLButtonElement>("#fullscreen")!.addEventListener("click", async () => {
  try {
    if (state.candidate === "mpv") {
      applyMpvSnapshot(await invoke<MpvSnapshot>("mpv_command", { command: "fullscreen" }));
    } else if (document.fullscreenElement) {
      await document.exitFullscreen();
    } else {
      await video.requestFullscreen();
    }
  } catch (error) {
    dispatch({ type: "error", detail: error instanceof Error ? error.message : String(error) });
  }
});
document.querySelector<HTMLButtonElement>("#export")!.addEventListener("click", () => {
  const blob = new Blob([JSON.stringify(exportableState(state), null, 2)], { type: "application/json" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = `sparrow-playback-${state.candidate ?? "idle"}-${Date.now()}.json`;
  link.click();
  URL.revokeObjectURL(link.href);
});

window.setInterval(() => {
  if (["starting", "playing", "paused", "stalled"].includes(state.status)) void sampleNow();
}, 60_000);
window.setInterval(() => {
  if (state.candidate === "mpv" && state.status === "starting") void sampleNow();
}, 1_000);
window.addEventListener("beforeunload", () => { void cleanup(false); });
