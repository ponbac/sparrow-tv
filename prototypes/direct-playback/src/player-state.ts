export type Candidate = "direct-video" | "browser-mpegts" | "tauri-mpegts" | "mpv";

export type PlaybackStatus =
  | "idle"
  | "starting"
  | "playing"
  | "paused"
  | "stalled"
  | "stopped"
  | "error";

export interface ProbeEvent {
  at: string;
  kind: string;
  detail?: string;
}

export interface ProbeState {
  candidate: Candidate | null;
  source: string | null;
  sourceLabel: string | null;
  status: PlaybackStatus;
  sessionStartedAt: string | null;
  firstFrameMs: number | null;
  elapsedSeconds: number;
  switches: number;
  restarts: number;
  stalls: number;
  errors: number;
  nativeBytes: number;
  decodedFrames: number | null;
  droppedFrames: number | null;
  speedKBps: number | null;
  videoCodec: string | null;
  audioCodec: string | null;
  resolution: string | null;
  currentTime: number | null;
  bufferedSeconds: number | null;
  lastEvent: string | null;
  events: ProbeEvent[];
}

export type ProbeAction =
  | { type: "start"; candidate: Candidate; source: string; sourceLabel: string }
  | { type: "status"; status: PlaybackStatus; detail?: string }
  | { type: "first-frame" }
  | { type: "restart"; detail: string }
  | { type: "bytes"; bytes: number }
  | { type: "media-info"; videoCodec?: string; audioCodec?: string; resolution?: string }
  | {
      type: "sample";
      currentTime?: number;
      bufferedSeconds?: number;
      decodedFrames?: number;
      droppedFrames?: number;
      speedKBps?: number;
      recordEvent?: boolean;
    }
  | { type: "error"; detail: string }
  | { type: "stop" };

export const initialProbeState: ProbeState = {
  candidate: null,
  source: null,
  sourceLabel: null,
  status: "idle",
  sessionStartedAt: null,
  firstFrameMs: null,
  elapsedSeconds: 0,
  switches: 0,
  restarts: 0,
  stalls: 0,
  errors: 0,
  nativeBytes: 0,
  decodedFrames: null,
  droppedFrames: null,
  speedKBps: null,
  videoCodec: null,
  audioCodec: null,
  resolution: null,
  currentTime: null,
  bufferedSeconds: null,
  lastEvent: null,
  events: [],
};

const event = (state: ProbeState, kind: string, detail?: string): ProbeState => ({
  ...state,
  lastEvent: detail ? `${kind}: ${detail}` : kind,
  events: [...state.events, { at: new Date().toISOString(), kind, detail }].slice(-120),
});

const sampleDetail = (state: ProbeState): string => [
  `elapsed=${state.elapsedSeconds}s`,
  state.currentTime === null ? null : `position=${state.currentTime.toFixed(1)}s`,
  state.bufferedSeconds === null ? null : `buffer=${state.bufferedSeconds.toFixed(1)}s`,
  state.decodedFrames === null ? null : `decoded=${state.decodedFrames}`,
  state.droppedFrames === null ? null : `dropped=${state.droppedFrames}`,
  state.speedKBps === null ? null : `speed=${state.speedKBps.toFixed(1)} KB/s`,
].filter((part): part is string => part !== null).join(" · ");

export function reduceProbe(state: ProbeState, action: ProbeAction): ProbeState {
  switch (action.type) {
    case "start": {
      const switched = state.source !== null &&
        (state.source !== action.source || state.candidate !== action.candidate);
      return event(
        {
          ...initialProbeState,
          candidate: action.candidate,
          source: action.source,
          sourceLabel: action.sourceLabel,
          status: "starting",
          sessionStartedAt: new Date().toISOString(),
          switches: state.switches + (switched ? 1 : 0),
          restarts: switched ? 0 : state.restarts,
          stalls: switched ? 0 : state.stalls,
          errors: switched ? 0 : state.errors,
          events: switched ? [] : state.events,
        },
        "start",
        `${action.candidate} · ${action.sourceLabel}`,
      );
    }
    case "status":
      return event(
        {
          ...state,
          status: action.status,
          stalls: state.stalls + (action.status === "stalled" ? 1 : 0),
        },
        action.status,
        action.detail,
      );
    case "first-frame": {
      if (state.firstFrameMs !== null || state.sessionStartedAt === null) return state;
      const firstFrameMs = Date.now() - Date.parse(state.sessionStartedAt);
      return event({ ...state, firstFrameMs, status: "playing" }, "first-frame", `${firstFrameMs} ms`);
    }
    case "restart":
      return event(
        { ...state, status: "starting", restarts: state.restarts + 1, firstFrameMs: null },
        "restart",
        action.detail,
      );
    case "bytes":
      return { ...state, nativeBytes: state.nativeBytes + action.bytes };
    case "media-info":
      return event(
        {
          ...state,
          videoCodec: action.videoCodec ?? state.videoCodec,
          audioCodec: action.audioCodec ?? state.audioCodec,
          resolution: action.resolution ?? state.resolution,
        },
        "media-info",
        [action.videoCodec, action.audioCodec, action.resolution].filter(Boolean).join(" · "),
      );
    case "sample": {
      const sessionStarted = state.sessionStartedAt ? Date.parse(state.sessionStartedAt) : Date.now();
      const sampledState = {
        ...state,
        elapsedSeconds: Math.round((Date.now() - sessionStarted) / 1000),
        currentTime: action.currentTime ?? state.currentTime,
        bufferedSeconds: action.bufferedSeconds ?? state.bufferedSeconds,
        decodedFrames: action.decodedFrames ?? state.decodedFrames,
        droppedFrames: action.droppedFrames ?? state.droppedFrames,
        speedKBps: action.speedKBps ?? state.speedKBps,
      };
      return action.recordEvent
        ? event(sampledState, "sample", sampleDetail(sampledState))
        : sampledState;
    }
    case "error":
      return event(
        { ...state, status: "error", errors: state.errors + 1 },
        "error",
        redactSensitiveText(action.detail),
      );
    case "stop":
      return event({ ...state, status: "stopped" }, "stop");
  }
}

export function redactSource(raw: string): string {
  try {
    const url = new URL(raw);
    return `${url.protocol}//${url.hostname}${url.port ? `:${url.port}` : ""}/…`;
  } catch {
    return "invalid URL";
  }
}

export function redactSensitiveText(text: string): string {
  return text.replace(/https?:\/\/[^\s"'\\]+/gi, (candidate) => redactSource(candidate));
}

export function exportableState(state: ProbeState): Omit<ProbeState, "source"> {
  const { source: _source, ...safe } = state;
  return safe;
}
