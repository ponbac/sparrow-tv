// Run against an open, configured, debuggable Shield TV app with ANDROID_SERIAL set.
// node scripts/debug/shield-benchmark.mjs --scenario all --runs 3 --channels 0,1 --out results.json
// ADB may name a wrapper; SPARROW_CDP_PORT may name a pre-forwarded local WebView port.
import { execFile } from "node:child_process";
import { parseArgs, promisify } from "node:util";
import { open } from "node:fs/promises";
import { firstPageLoadTarget, walkGuide } from "./shield-guide-walk.mjs";
import { setTimeout as sleep } from "node:timers/promises";

const { values } = parseArgs({ options: {
  scenario: { type: "string", default: "all" },
  "browse-steps": { type: "string" },
  "browse-repeat-ms": { type: "string" },
  "browse-input": { type: "string", default: "all" },
  // Tune the first Channel after every walk's reload, so the Guide is browsed over a playing picture.
  "browse-playback": { type: "boolean", default: false },
  runs: { type: "string", default: "3" },
  channels: { type: "string", default: "0,1" },
  out: { type: "string" },
} });
const runs = Number(values.runs);
const walkSteps = values["browse-steps"] === undefined ? null : Number(values["browse-steps"]);
const repeatMs = values["browse-repeat-ms"] === undefined ? null : Number(values["browse-repeat-ms"]);
const walkInput = values["browse-input"];
const channels = values.channels.split(",").map(Number);
if (!process.env.ANDROID_SERIAL || !values.out || !["all", "guide", "playback", "quality", "controls", "tune", "catalog-search", "browse", "browse-now", "browse-timeline"].includes(values.scenario)
    || !["all", "single", "burst", "held", "return"].includes(walkInput) || (walkInput === "held" && repeatMs === null)
    || (repeatMs !== null && (!Number.isSafeInteger(repeatMs) || repeatMs < 50 || repeatMs > 1000))
    || (walkSteps !== null && (!Number.isSafeInteger(walkSteps) || walkSteps < 1 || walkSteps > 120))
    || !Number.isSafeInteger(runs) || runs < 1 || runs > 100 || !/^\d+,\d+$/.test(values.channels)
    || channels.some(n => !Number.isSafeInteger(n)) || channels[0] === channels[1]
    || (values["browse-playback"] && !values.scenario.startsWith("browse"))) {
  throw Error("Set ANDROID_SERIAL and pass --out <new JSON file>, --scenario all|guide|playback|quality|controls|tune|catalog-search|browse|browse-now|browse-timeline, --runs 1..100, optional --browse-playback (browse scenarios only), --browse-steps 1..120, --browse-repeat-ms 50..1000, --browse-input all|single|burst|held|return, --channels <two distinct zero-based row indices>.");
}

const exec = promisify(execFile);
async function adb(...args) {
  try {
    // One Android input command can contain the entire repeated-key walk.
    const timeout = Math.max(12000, args.length * 1000);
    return (await exec(process.env.ADB || "adb", args, { timeout, maxBuffer: 1024 * 1024 })).stdout.trim();
  } catch {
    throw Error("ADB command failed");
  }
}

let forward;
let socket;
let nextId = 0;
const pending = new Map();
async function connect() {
  let port = process.env.SPARROW_CDP_PORT;
  if (!port) {
    const pid = await adb("shell", "pidof", "-s", "xyz.ponbac.sparrow");
    if (!/^\d+$/.test(pid)) throw Error("Sparrow process unavailable");
    port = await adb("forward", "tcp:0", "localabstract:webview_devtools_remote_" + pid);
    if (!/^\d+$/.test(port)) throw Error("WebView forward unavailable");
    forward = port;
  }
  if (!/^\d+$/.test(port)) throw Error("Invalid CDP port");
  const host = "127.0.0.1:" + port;
  const targets = await fetch("http://" + host + "/json/list", { signal: AbortSignal.timeout(5000) }).then(r => r.json());
  const page = targets.find(t => t.type === "page" && /^https?:\/\/tauri\.localhost(?:[:/]|$)/.test(t.url));
  if (!page) throw Error("Bundled Sparrow page unavailable");
  socket = new WebSocket("ws://" + host + new URL(page.webSocketDebuggerUrl).pathname);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(Error("WebView connection timed out")), 5000);
    socket.onopen = () => { clearTimeout(timer); resolve(); };
    socket.onerror = () => { clearTimeout(timer); reject(Error("WebView connection failed")); };
  });
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    const reply = pending.get(message.id);
    if (!reply) return;
    pending.delete(message.id);
    reply(message);
  };
}
async function cdp(method, params) {
  const id = ++nextId;
  const result = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(Error("WebView probe timed out")); }, 25000);
    pending.set(id, message => { clearTimeout(timer); resolve(message); });
    socket.send(JSON.stringify({ id, method, params }));
  });
  if (result.error || result.result?.exceptionDetails) throw Error("WebView probe failed");
  return result.result;
}
async function evaluate(expression) {
  const response = await cdp("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  return response?.result?.value;
}

// Only the page holds DOM references. No text, input contents, or identifiers cross CDP.
const guideRow = ".programme-guide__row .programme-guide__channel";
const group = "[data-acceptance-group]";
const search = "[data-acceptance-search] input";
const view = ".programme-guide__view";
const probe = `const q = s => document.querySelector(s);
  const video = q('.hosted-player video');
  const playing = video?.dataset.playbackEngine === 'android-media3' &&
    video.dataset.playbackState === 'playing' && Number(video.dataset.decodedFrames) > 0;
  const focus = document.activeElement;
  const selected = q('.programme-guide__row .programme-guide__channel[aria-pressed="true"]');
  const busy = q('.programme-guide__board')?.getAttribute('aria-busy') === 'true';`;

async function check(expression) {
  return evaluate(`(() => { ${probe} return Boolean(${expression}); })()`);
}
async function focusAt(selector, index = 0) {
  return evaluate(`(async () => {
    const el = document.querySelectorAll(${JSON.stringify(selector)})[${index}];
    if (!el || !el.getClientRects().length || el.disabled) return false;
    el.focus({ preventScroll: true });
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return el === document.activeElement;
  })()`);
}

const samples = [];
let run = 0;
async function action(name, input, condition, { target, index = 0, timeout = 3500, details = null, detectPlaybackFailure = false } = {}) {
  const sample = { run, action: name };
  const nativeMessage = ({ MEDIA_PAUSE: "command:pause", MEDIA_PLAY: "command:play",
    CHANNEL_UP: "tv:channel-up", CHANNEL_DOWN: "tv:channel-down" })[input[1]] ?? null;
  if (target && !(await focusAt(target, index))) {
    Object.assign(sample, { outcome: "target-unavailable" });
  } else {
    const state = await evaluate(`(() => {
      ${probe}
      const oldVideo = video;
      const oldFocus = focus;
      const oldSelected = selected;
      const oldRow = focus?.closest('.programme-guide__row');
      ${name === "load-more-channels" ? `
        const board = q('.programme-guide__board');
        const rows = q('.programme-guide__rows');
        const virtual = rows.hasAttribute('data-windowed');
        const extent = () => virtual ? rows.offsetHeight
          : rows.querySelectorAll('.programme-guide__row').length;
        const originalExtent = extent();
        const originalHeight = board.scrollHeight;
      ` : ""}
      const test = () => { ${probe} return Boolean(${condition}); };
      if (test()) return 'already-satisfied';
      const armed = performance.now();
      let eventTime = null;
      let eventSource = null;
      const message = ${JSON.stringify(nativeMessage)};
      const port = window.sparrowAndroidPlayer;
      const bridgeObservable = message && typeof port?.addEventListener === 'function' &&
        typeof port?.removeEventListener === 'function';
      const onNative = e => {
        if (e.data === message && eventTime === null) {
          eventTime = performance.now();
          eventSource = 'native-message';
        }
      };
      if (bridgeObservable) port.addEventListener('message', onNative);
      const note = e => {
        if (eventTime !== null) return;
        eventTime = Math.max(armed, e.timeStamp);
        eventSource = e.type;
      };
      const events = message ? [] : ['keydown', 'click', 'input'];
      events.forEach(t => window.addEventListener(t, note, true));
      let observer;
      let raf;
      let timer;
      const acknowledgementReached = ${details ? '() => { ' + probe + ' return Boolean(' + details + '); }' : 'null'};
      let acknowledgement = null;
      // Watch only for long media waits; an every-frame DOM scan changes the work being measured.
      window.__sparrowBenchmarkDone = new Promise(resolve => {
        const finish = (outcome, end) => {
          observer.disconnect();
          cancelAnimationFrame(raf);
          clearTimeout(timer);
          events.forEach(t => window.removeEventListener(t, note, true));
          if (bridgeObservable) port.removeEventListener('message', onNative);
          resolve({ outcome, eventSource, armedToFrameMs: end === null ? null : Math.round(end - armed),
            eventToFrameMs: end === null || eventTime === null ? null : Math.round(end - eventTime),
            eventToAckMs: acknowledgement === null || eventTime === null ? null : Math.round(acknowledgement - eventTime) });
        };
        function inspect() {
          if (acknowledgementReached && acknowledgement === null && acknowledgementReached()) acknowledgement = performance.now();
          if (${detectPlaybackFailure} && q('.hosted-player__state[data-state="failed"]') &&
              q('.hosted-player video') !== oldVideo) {
            finish('playback-failed', null);
            return;
          }
          if (!test()) return;
          observer.disconnect();
          cancelAnimationFrame(raf);
          // A second rAF provides one paint opportunity after the expected DOM state.
          raf = requestAnimationFrame(() => finish('ok', performance.now()));
        }
        const schedule = () => { if (!raf) raf = requestAnimationFrame(() => { raf = 0; inspect(); }); };
        observer = new MutationObserver(schedule);
        observer.observe(document.body, { subtree: true, attributes: true, childList: true, characterData: true });
        const onSignal = () => schedule();
        const signals = ['focusin', 'focusout', 'fullscreenchange', 'sparrow-tv-fullscreenchange', 'input', 'keyup'];
        signals.forEach(t => document.addEventListener(t, onSignal, true));
        window.__sparrowBenchmarkCleanup = () => signals.forEach(t => document.removeEventListener(t, onSignal, true));
        timer = setTimeout(() => finish('timeout', null), ${timeout});
      }).finally(() => window.__sparrowBenchmarkCleanup());
      return 'armed';
    })()`);
    if (state === "armed") {
      const started = performance.now();
      const observation = evaluate("window.__sparrowBenchmarkDone").then(value => ({
        ...value, hostDispatchToObservationMs: Math.round(performance.now() - started),
      }));
      const [result] = await Promise.all([observation, adb("shell", "input", ...input)]);
      Object.assign(sample, result);
    } else {
      sample.outcome = state;
    }
  }
  samples.push(sample);
  console.log(`${run} ${name} ${sample.outcome} ${sample.eventToFrameMs ?? "-"} / ${sample.armedToFrameMs ?? "-"} ms`);
  return sample.outcome === "ok";
}
const key = code => ["keyevent", code];
const center = key("DPAD_CENTER");
const back = key("BACK");
const has = selector => `q(${JSON.stringify(selector)}) !== null`;
const openDialog = selector => has(selector + "[data-open]");
async function dialog(name, trigger, popup) {
  if (await action(name + "-open", center, openDialog(popup), { target: trigger })) {
    await action(name + "-back", back, "!(" + openDialog(popup) + ")");
  }
}
async function guide() {
  if (await check('q(".programme-guide")?.dataset.view === "timeline"')) {
    await action("now-restore", center, 'q(".programme-guide")?.dataset.view === "now"', { target: view });
  }
  await action("now-horizontal-focus", key("DPAD_RIGHT"), "focus !== oldFocus && focus?.closest('.programme-guide__row') !== null", { target: guideRow });
  await action("now-vertical-focus", key("DPAD_DOWN"), "focus?.closest('.programme-guide__row') && focus.closest('.programme-guide__row') !== oldRow", { target: guideRow });
  await action("group-focus", key("DPAD_RIGHT"), "focus !== oldFocus && focus?.matches('[data-acceptance-group]')", { target: group });
  for (const [name, index] of [["group-select", 1], ["all-groups-select", 0]]) {
    await action(name, center, "oldFocus.getAttribute('aria-checked') === 'true' && !busy && !!q('.programme-guide__row')", { target: group, index });
  }
  const timeline = await action("timeline-open", center, has('.programme-guide__board[data-view="timeline"] .programme-guide__row'), { target: view });
  if (timeline) {
    await action("timeline-vertical-focus", key("DPAD_DOWN"), "focus?.closest('.programme-guide__row') && focus.closest('.programme-guide__row') !== oldRow", { target: guideRow });
    await action("timeline-horizontal-focus", key("DPAD_RIGHT"), "focus !== oldFocus && focus?.closest('.programme-guide__row') === oldRow", {
      target: ".programme-guide__track .programme-guide__programme:first-child:has(+ .programme-guide__programme)",
    });
    await action("now-open", center, 'q(".programme-guide")?.dataset.view === "now"', { target: view });
  }
  // The TV loads the next page automatically when D-pad focus enters its last eight rows.
  // Start one row outside that threshold; the key, not setup focus, starts pagination.
  if (await check(has(".programme-guide__more"))) {
    const edge = await firstPageLoadTarget(evaluate);
    if (edge && !edge.loadedBeyond) {
      await action("load-more-channels", key("DPAD_DOWN"),
        "focus !== oldFocus && (extent() > originalExtent || board.scrollHeight > originalHeight) && !busy",
        { target: `.programme-guide__row[data-number="${edge.number}"] .programme-guide__channel` });
    } else {
      samples.push({ run, action: "load-more-channels", outcome: edge ? "already-loaded" : "target-unavailable" });
    }
  }
  await dialog("group-chooser", ".group-roster-trigger", ".group-roster__popup");
  await dialog("sources", ".feeds-trigger", ".feeds-dialog__popup");
  if (await action("search-edit", center, "focus === oldFocus && !oldFocus.readOnly", { target: search })) {
    await action("search-results", ["text", "news"], "oldFocus.value === 'news' && !!q('.board-search__result:not(.board-search__result--desk)')");
    if (await check(has(".board-search__result--desk"))) {
      // Hide the OS keyboard by moving focus to a suggestion, as the app's D-pad handler does.
      if (await focusAt(".board-search__result--desk")) {
        if (await action("search-sheet-open", center, openDialog(".board-search-desk__popup"))) {
          await action("search-sheet-back", back, "!(" + openDialog(".board-search-desk__popup") + ")");
        }
      }
    }
    if (await focusAt('[aria-label="Clear search"]')) {
      await action("search-clear", center, `q(${JSON.stringify(search)})?.value === ''`);
    } else if (await check(`q(${JSON.stringify(search)})?.value.length > 0`)) {
      // Reset the ephemeral query for the next measured repetition.
      await evaluate(`(() => { document.querySelector('[aria-label="Clear search"]')?.click(); return true; })()`);
    }
  }
}

const newPlaying = "video !== oldVideo && playing";
const full = "!!(document.fullscreenElement || document.webkitFullscreenElement || q('.hosted-player[popover=manual]'))";
async function cleanupPlayer() {
  if (!await check(has(".hosted-player"))) return;
  // Hidden control is used solely for teardown after an inaccessible or failed remote stop.
  await evaluate(`(() => { document.querySelector('.hosted-player__stop')?.click(); return true; })()`);
  for (let i = 0; i < 30; i++) {
    if (!await check(has(".hosted-player"))) return;
    await sleep(100);
  }
  throw Error("Could not clean up the Playback Session");
}
async function fullscreenControls() {
  return action("fullscreen-controls", key("DPAD_RIGHT"), "!!focus?.closest('.hosted-player__controls')", { target: ".hosted-player" });
}
async function playback() {
  if (await check(has(".hosted-player"))) throw Error("Playback scenario needs the Guide with no active player");
  const tune = (name, index) => action(name, center, newPlaying + " && selected === oldFocus", {
    target: guideRow, index, timeout: 18000, detectPlaybackFailure: true, details: "selected === oldFocus && video !== oldVideo",
  });
  await tune("start-playback", channels[0]);
  await tune("switch-channel", channels[1]);
  if (await check("!!selected")) {
    if (await action("fullscreen", center, full, { target: ".stage__undock" })) {
      await fullscreenControls();
      await action("fullscreen-back", back, "!(" + full + ")");
    }
    if (await action("pause", key("MEDIA_PAUSE"), has('.hosted-player__state[data-state="paused"]') + " && !video?.dataset.playbackEngine")) {
      await action("resume", key("MEDIA_PLAY"), "playing", { timeout: 18000 });
    }
    for (const [name, code] of [["channel-up", "CHANNEL_UP"], ["channel-down", "CHANNEL_DOWN"]]) {
      await action(name, key(code), newPlaying + " && !!selected && selected !== oldSelected", { timeout: 18000, detectPlaybackFailure: true });
    }
    if (!await focusAt(".hosted-player__stop") && await focusAt(".stage__undock")) {
      if (await action("fullscreen-to-stop", center, full)) await fullscreenControls();
    }
    await action("stop", center, "!q('.hosted-player')", { target: ".hosted-player__stop" });
  }
}


// Keep the original preference bytes on-device, including the distinction between
// a stored preference and the default the Guide shows when none has been saved.
const preferenceKey = "sparrow.variant-preferences:v1";
const backupKey = "__sparrowShieldBenchmarkOriginalQuality";
async function restoreQualityStorage() {
  const restored = await evaluate(`(() => {
    const saved = sessionStorage.getItem(${JSON.stringify(backupKey)});
    if (saved === null) return true;
    const key = ${JSON.stringify(preferenceKey)};
    const original = JSON.parse(saved);
    if (original === null) localStorage.removeItem(key); else localStorage.setItem(key, original);
    if (localStorage.getItem(key) !== original) return false;
    sessionStorage.removeItem(${JSON.stringify(backupKey)});
    return true;
  })()`);
  if (!restored) throw Error("Could not restore original quality preference");
}

// React keeps a copy of the choice map in memory. A fresh page reads the exact
// restored map, including when the selected chip was originally an implicit default.
async function reloadGuide() {
  await evaluate("(() => { setTimeout(() => location.reload(), 0); return true; })()");
  await sleep(250);
  for (let i = 0; i < 100; i++) {
    if (await check("!!q('.programme-guide__row')").catch(() => false)) return;
    await sleep(100);
  }
  throw Error("Populated TV Guide unavailable");
}

// Tunes the first row and waits for Media3 to decode, then leaves focus on that row.
let tunedForWalk = false;
async function tuneFirstRow() {
  if (!await focusAt(guideRow)) throw Error("Guide walk setup failed");
  tunedForWalk = true;
  await adb("shell", "input", "keyevent", "DPAD_CENTER");
  for (let i = 0; i < 250; i++) {
    if (await check("playing")) return;
    await sleep(100);
  }
  throw Error("Guide walk playback unavailable");
}
// Snapshots Media3's counters on the playing video. While a leg runs, any
// playback state other than "playing" (a stall or recovery) is recorded, so
// the walk can withhold counter deltas that might span two players.
let videoIds = 0;
async function pictureCounters() {
  return evaluate(`(() => {
    const video = document.querySelector('.hosted-player video');
    const watch = window.__sparrowWalkPicture ??= { ids: new WeakMap(), interrupted: false, observer: null };
    if (video && !watch.ids.has(video)) watch.ids.set(video, ${++videoIds});
    const count = field => /^[0-9]+$/.test(video?.dataset[field] ?? "") ? Number(video.dataset[field]) : null;
    const snapshot = { video: video ? watch.ids.get(video) : null, playing: video?.dataset.playbackState === 'playing',
      interrupted: watch.interrupted, decoded: count('decodedFrames'), dropped: count('droppedFrames') };
    watch.observer?.disconnect();
    watch.interrupted = false;
    if (video) {
      watch.observer = new MutationObserver(() => {
        if (video.dataset.playbackState !== 'playing' || !video.isConnected) watch.interrupted = true;
      });
      watch.observer.observe(video, { attributes: true, attributeFilter: ['data-playback-state', 'data-playback-engine'] });
    }
    return snapshot;
  })()`);
}

async function quality() {
  // The first --channels index identifies the row with two or more Quality Variants.
  const index = channels[0];
  const initial = await evaluate(`(() => {
    const row = document.querySelectorAll(${JSON.stringify(guideRow)})[${index}]?.closest('.programme-guide__row');
    const chips = Array.from(row?.querySelectorAll('.programme-guide__variants button') ?? []);
    const current = chips.findIndex(e => e.dataset.current === 'true');
    if (chips.length < 2 || current < 0 || !/^\\d+$/.test(row.dataset.number)) return null;
    sessionStorage.setItem(${JSON.stringify(backupKey)}, JSON.stringify(localStorage.getItem(${JSON.stringify(preferenceKey)})));
    return { number: Number(row.dataset.number), current };
  })()`);
  if (!initial) {
    samples.push({ run, action: "quality-switch", outcome: "target-unavailable" });
    return;
  }
  const chips = `.programme-guide__row[data-number="${initial.number}"] .programme-guide__variants button`;
  const ready = "oldFocus.getAttribute('aria-pressed') === 'true' && " + newPlaying;
  const options = { timeout: 18000, detectPlaybackFailure: true };
  try {
    if (await action("quality-initial-tune", center, newPlaying + " && selected === oldFocus",
      { target: guideRow, index, ...options })) {
      await action("quality-switch", center, ready, { target: chips + '[aria-pressed="false"]', ...options });
    }
  } finally {
    try {
      if (await check(`document.querySelectorAll(${JSON.stringify(chips)})[${initial.current}]?.dataset.current === 'false'`)) {
        await action("quality-restore", center, ready, { target: chips, index: initial.current, ...options });
      }
    } finally {
      await restoreQualityStorage();
    }
  }
}

// The WebView publishes Media3 counters on a 250 ms cadence. The gap metric
// includes sampling granularity and estimates the longest *observed* gap.
async function observeSteadyPlayback() {
  return evaluate(`(async () => {
    const selector = '.hosted-player video';
    const video = document.querySelector(selector);
    const counter = (element, field) => {
      const raw = element?.dataset[field];
      if (!raw || !/^[0-9]+$/.test(raw)) return null;
      const n = Number(raw);
      return Number.isSafeInteger(n) ? n : null;
    };
    const started = performance.now();
    const firstDecoded = counter(video, 'decodedFrames');
    const firstDropped = counter(video, 'droppedFrames');
    let previousDecoded = firstDecoded;
    let previousDropped = firstDropped;
    let lastProgress = started;
    let largestGap = 0;
    let sampleCount = 0;
    let nonPlayingSamples = 0;
    let identityChanged = false;
    let stopped = false;
    let countersInvalid = firstDecoded === null || firstDropped === null;
    let elapsed;
    do {
      const now = performance.now();
      elapsed = now - started;
      const current = document.querySelector(selector);
      const state = current?.dataset.playbackState;
      sampleCount++;
      identityChanged ||= current !== video;
      stopped ||= !current || current.dataset.playbackEngine !== 'android-media3' ||
        state === 'failed' || state === 'stopped';
      if (state !== 'playing') nonPlayingSamples++;
      const decoded = counter(current, 'decodedFrames');
      const dropped = counter(current, 'droppedFrames');
      if (decoded === null || dropped === null || decoded < previousDecoded || dropped < previousDropped) {
        countersInvalid = true;
      }
      largestGap = Math.max(largestGap, now - lastProgress);
      if (decoded !== null && previousDecoded !== null && decoded > previousDecoded) lastProgress = now;
      previousDecoded = decoded;
      previousDropped = dropped;
      if (elapsed < 6000) await new Promise(resolve => setTimeout(resolve, Math.min(250, 6000 - elapsed)));
    } while (elapsed < 6000);
    const valid = !countersInvalid && !identityChanged;
    return { durationMs: Math.round(elapsed), sampleCount, nonPlayingSamples, identityChanged, stopped,
      countersValid: valid, decodedDelta: valid ? previousDecoded - firstDecoded : null,
      droppedDelta: valid ? previousDropped - firstDropped : null,
      maxObservedGapMs: valid ? Math.round(largestGap) : null };
  })()`);
}
async function tuneSequence() {
  let turn = 0;
  for (const index of [...channels, ...channels]) {
    const name = "tune-" + ++turn;
    const ok = await action(name, center, newPlaying + " && selected === oldFocus", {
      target: guideRow, index, timeout: 18000, detectPlaybackFailure: true,
      details: "selected === oldFocus && video !== oldVideo && video?.dataset.playbackEngine === 'android-media3'",
    });
    if (ok) {
      const steady = await observeSteadyPlayback();
      samples[samples.length - 1].steady = steady;
      console.log(`${run} ${name} steady decoded=${steady.decodedDelta} dropped=${steady.droppedDelta} nonplaying=${steady.nonPlayingSamples} max-gap=${steady.maxObservedGapMs} ms`);
    }
  }
}

async function catalogSearch() {
  for (const term of ["news", "sport", "weather report", "zzqxx", "abc"]) {
    const result = await evaluate(`(async () => {
      const input = {
        requestId: 'srch1_' + crypto.randomUUID().replaceAll('-', '') + '_1',
        term: ${JSON.stringify(term)}, channelLimit: 40, programmeLimit: 40,
      };
      const started = performance.now();
      const result = await window.__TAURI_INTERNALS__.invoke('catalog_search', { input });
      return { elapsedMs: Math.round(performance.now() - started), generation: result.generation,
        channels: result.channels.items.length, programmes: result.programmes.items.length };
    })()`);
    samples.push({ run, action: "catalog-search", term, ...result });
  }
}

async function controls() {
  const tuned = await action("controls-initial-tune", center, newPlaying + " && selected === oldFocus",
    { target: guideRow, index: channels[0], timeout: 18000, detectPlaybackFailure: true });
  if (!tuned || !await action("controls-fullscreen", center, full, { target: ".stage__undock" })) return;
  await fullscreenControls();
  const mute = '.hosted-player__controls button[aria-pressed]:not([data-stage-action]):not(.hosted-player__fullscreen)';
  const original = await evaluate(`document.querySelector(${JSON.stringify(mute)})?.getAttribute('aria-pressed') ?? null`);
  if (original === "false" || original === "true") {
    try {
      await action("toggle-mute", center, `oldFocus.getAttribute('aria-pressed') !== ${JSON.stringify(original)}`, { target: mute });
    } finally {
      if (!await check(`q(${JSON.stringify(mute)})?.getAttribute('aria-pressed') === ${JSON.stringify(original)}`)) {
        await action("restore-mute", center, `oldFocus.getAttribute('aria-pressed') === ${JSON.stringify(original)}`, { target: mute });
      }
    }
  }
  await action("controls-back", back, "!(" + full + ")");
}

let file;
try {
  file = await open(values.out, "wx", 0o600);
  await connect();
  if (!await check("document.documentElement.hasAttribute('data-television') && !!q('.programme-guide__row')")) {
    throw Error("Populated TV Guide unavailable");
  }
  const usesPlayer = !["guide", "catalog-search", "browse", "browse-now", "browse-timeline"].includes(values.scenario);
  if (usesPlayer) await cleanupPlayer();
  if (values.scenario === "quality") await restoreQualityStorage();
  for (run = 1; run <= runs; run++) {
    for (const mode of ["now", "timeline"]) {
      if (values.scenario === "browse" || values.scenario === "browse-" + mode) {
        const playback = values["browse-playback"];
        samples.push(...await walkGuide({ mode, run, walkSteps, walkInput, repeatMs, adb, cdp, evaluate, focusAt, sleep,
          // Stop the previous walk's session while its controls are still on the page.
          freshGuide: playback ? async () => { if (tunedForWalk) await cleanupPlayer(); await reloadGuide(); await tuneFirstRow(); } : reloadGuide,
          picture: playback ? pictureCounters : null }));
      }
    }
    if (values.scenario === "catalog-search") await catalogSearch();
    if (["all", "guide"].includes(values.scenario)) await guide();
    if (usesPlayer) {
      try {
        if (values.scenario === "quality") await quality();
        else if (values.scenario === "controls") await controls();
        else if (values.scenario === "tune") await tuneSequence();
        else await playback();
      } finally {
        await cleanupPlayer();
        if (values.scenario === "quality" && run === runs) await reloadGuide();
      }
    }
  }
} catch (error) {
  // ADB, CDP, and system errors can carry private endpoints and page data.
  const safe = ["Sparrow process unavailable", "WebView forward unavailable", "Invalid CDP port",
    "Bundled Sparrow page unavailable", "WebView connection timed out", "WebView connection failed",
    "WebView probe timed out", "WebView probe failed", "ADB command failed",
    "Populated TV Guide unavailable", "Playback scenario needs the Guide with no active player",
    "Could not clean up the Playback Session", "Could not restore original quality preference",
    "Guide walk requires All groups", "Guide walk setup failed", "Guide walk needs at least two Channel pages",
    "Guide walk playback unavailable"];
  console.error(safe.includes(error.message) ? error.message : "Benchmark setup or I/O failed");
  process.exitCode = 1;
} finally {
  // Stops only the Playback Session a walk started.
  if (tunedForWalk) {
    try {
      await cleanupPlayer();
    } catch {
      console.error("Could not clean up the Playback Session");
      process.exitCode = 1;
    }
  }
  socket?.close();
  if (forward) await adb("forward", "--remove", "tcp:" + forward);
  if (file) {
    await file.writeFile(JSON.stringify({ scenario: values.scenario, runs, channelRowIndices: channels, samples }, null, 2) + "\n");
    await file.close();
  }
}
