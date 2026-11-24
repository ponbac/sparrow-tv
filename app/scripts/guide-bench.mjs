// Desktop bench for the TV Timeline over a playing picture: the real shell at the
// Shield's CSS size, a deterministic catalog (src/bench) and a looping clip.
//   ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=30 -t 20 -c:v libvpx-vp9 -b:v 2M <dir>/bench-clip.webm
//   BENCH_PUBLIC=<dir> npx vite build -c vite.bench.config.ts
//   npx vite preview -c vite.bench.config.ts --port 4790 &
//   node scripts/guide-bench.mjs --runs 5 --out a.json
// Playwright is not an app dependency: pass --playwright <its index.mjs> or set
// PLAYWRIGHT_MODULE when it is installed elsewhere. The page's Date starts at
// --now (UTC) so every run lays out the same Programmes; event timestamps,
// performance.now() and animation frames stay native.
// Compare two builds by serving each on its own port (--url), alternating runs,
// then pool each side's files with --merge.
import { readFile, writeFile } from "node:fs/promises";
import { parseArgs } from "node:util";

const { values: options, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    merge: { type: "boolean", default: false },
    runs: { type: "string", default: "5" },
    throttle: { type: "string", default: "4" },
    url: { type: "string", default: "http://localhost:4790/bench.html" },
    now: { type: "string", default: "2026-11-23T20:10:00Z" },
    chromium: { type: "string", default: "/usr/bin/chromium" },
    playwright: { type: "string", default: process.env.PLAYWRIGHT_MODULE ?? "playwright" },
    out: { type: "string" },
  },
});

const SCENARIOS = [
  { name: "tap-down", key: "ArrowDown", count: 60, held: false },
  { name: "tap-up", key: "ArrowUp", count: 40, held: false },
  { name: "tap-right", key: "ArrowRight", count: 24, held: false },
  { name: "held-down", key: "ArrowDown", count: 60, held: true, intervalMs: 125 },
];
const METRICS = ["ScriptDuration", "LayoutDuration", "RecalcStyleDuration", "TaskDuration", "LayoutCount", "RecalcStyleCount", "Nodes", "JSHeapUsedSize"];

// Runs in the page: times each trusted arrow press to focus and to the first
// frame where the newly focused cell is visible, plus LoAF and picture health.
function installProbe(key) {
  const board = document.querySelector(".programme-guide__board");
  const ruler = board.querySelector(".programme-guide__timeline-ruler");
  const video = document.querySelector("video");
  const visible = (element) => {
    const box = element.getBoundingClientRect(), port = board.getBoundingClientRect();
    const top = Math.max(port.top, ruler?.getBoundingClientRect().bottom ?? port.top);
    return box.height > 0 && box.top >= top - 1 && box.bottom <= port.bottom + 1 &&
      box.left < port.right && box.right > port.left;
  };
  const samples = [];
  let pending = null;
  const onKey = (event) => {
    if (!event.isTrusted || event.key !== key) return;
    const sample = { eventMs: event.timeStamp, focusMs: null, frameMs: null, superseded: false, moved: false, panned: false, noop: false };
    const view = () => `${board.scrollTop}|${board.style.getPropertyValue("--tl-offset")}`;
    const before = view();
    // An earlier press still waiting for its frame cannot claim this one's pan.
    const previous = samples.at(-1);
    if (previous && !previous.moved && !previous.noop && previous.frameMs === null) previous.superseded = true;
    pending = { sample, from: document.activeElement };
    samples.push(sample);
    // A press that pans the timeline without moving focus is timed to the frame
    // after it; one that changed nothing is counted apart, with no frame time.
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (sample.moved || sample.superseded) return;
      if (view() !== before) {
        sample.panned = true;
        sample.frameMs = performance.now() - sample.eventMs;
      } else {
        sample.noop = true;
      }
    }));
  };
  const onFocus = (event) => {
    if (!pending || event.target === pending.from) return;
    const { sample } = pending;
    pending = null;
    const target = event.target;
    sample.moved = true;
    sample.focusMs = performance.now() - sample.eventMs;
    const check = () => {
      if (document.activeElement !== target) { sample.superseded = true; return; }
      if (!visible(target)) {
        if (performance.now() - sample.eventMs < 2500) requestAnimationFrame(check);
        return;
      }
      requestAnimationFrame(() => {
        if (document.activeElement !== target) { sample.superseded = true; return; }
        sample.frameMs = performance.now() - sample.eventMs;
      });
    };
    requestAnimationFrame(check);
  };
  const frames = [];
  const loaf = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) frames.push({ duration: entry.duration, blocking: entry.blockingDuration });
  });
  loaf.observe({ type: "long-animation-frame" });
  let presented = 0, watching = true;
  const onVideoFrame = () => { presented += 1; if (watching) video.requestVideoFrameCallback(onVideoFrame); };
  video.requestVideoFrameCallback(onVideoFrame);
  const quality = () => { const q = video.getVideoPlaybackQuality(); return { dropped: q.droppedVideoFrames, total: q.totalVideoFrames }; };
  const startQuality = quality();
  const startedAt = performance.now();
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("focusin", onFocus, true);
  window.__guideBench = {
    async settle(index) {
      const start = performance.now();
      while (performance.now() - start < 2500) {
        const sample = samples[index];
        if (sample && (sample.frameMs !== null || sample.superseded || sample.noop)) return;
        await new Promise((resolve) => setTimeout(resolve, 8));
      }
    },
    finish() {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("focusin", onFocus, true);
      loaf.disconnect();
      watching = false;
      const endQuality = quality();
      const seconds = (performance.now() - startedAt) / 1000;
      return {
        samples,
        loaf: { count: frames.length, totalMs: frames.reduce((a, f) => a + f.duration, 0), blockingMs: frames.reduce((a, f) => a + f.blocking, 0), maxMs: Math.max(0, ...frames.map((f) => f.duration)) },
        video: { presentedFps: presented / seconds, dropped: endQuality.dropped - startQuality.dropped, decoded: endQuality.total - startQuality.total },
        seconds,
      };
    },
  };
}

// Runs in the page before its scripts: Date counts on from `start`, leaving
// every other clock alone (Playwright's clock API replaces those too).
function pinDate(start) {
  const NativeDate = Date;
  const offset = start - NativeDate.now();
  globalThis.Date = class extends NativeDate {
    constructor(...args) {
      super(...(args.length === 0 ? [NativeDate.now() + offset] : args));
    }
    static now() {
      return NativeDate.now() + offset;
    }
  };
}

async function openTimeline(page) {
  await page.waitForSelector(".programme-guide__row .programme-guide__channel", { timeout: 20000 });
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => { const v = document.querySelector("video"); return v && !v.paused && v.currentTime > 1; }, null, { timeout: 20000 });
  await page.focus(".programme-guide__view");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => document.querySelector(".programme-guide")?.dataset.view === "timeline");
  await page.waitForSelector(".programme-guide__row .programme-guide__programme[data-live='true']");
}

async function focusFirstCell(page) {
  await page.evaluate(() => {
    const board = document.querySelector(".programme-guide__board");
    board.scrollTop = 0;
    board.querySelector(".programme-guide__row .programme-guide__programme[data-live='true']")?.focus();
  });
  await page.waitForTimeout(300);
}

async function press(cdp, key, { autoRepeat = false, type = "rawKeyDown" } = {}) {
  const codes = { ArrowDown: 40, ArrowUp: 38, ArrowRight: 39, ArrowLeft: 37 };
  await cdp.send("Input.dispatchKeyEvent", { type, key, code: key, windowsVirtualKeyCode: codes[key], autoRepeat });
}

async function metrics(cdp) {
  const { metrics } = await cdp.send("Performance.getMetrics");
  return Object.fromEntries(metrics.filter((m) => METRICS.includes(m.name)).map((m) => [m.name, m.value]));
}

async function runScenario(page, cdp, scenario) {
  await page.evaluate(installProbe, scenario.key);
  const before = await metrics(cdp);
  if (scenario.held) {
    const start = Date.now();
    for (let index = 0; index < scenario.count; index++) {
      const wait = start + index * scenario.intervalMs - Date.now();
      if (wait > 0) await new Promise((resolve) => setTimeout(resolve, wait));
      await press(cdp, scenario.key, { autoRepeat: index > 0 });
    }
    await press(cdp, scenario.key, { type: "keyUp" });
    await page.evaluate((i) => window.__guideBench.settle(i), scenario.count - 1);
  } else {
    for (let index = 0; index < scenario.count; index++) {
      await press(cdp, scenario.key);
      await press(cdp, scenario.key, { type: "keyUp" });
      await page.evaluate((i) => window.__guideBench.settle(i), index);
    }
  }
  const after = await metrics(cdp);
  const result = await page.evaluate(() => window.__guideBench.finish());
  const cost = Object.fromEntries(METRICS.map((name) => [name, after[name] - before[name]]));
  return { ...result, cost, end: after };
}

function percentile(values, p) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
}

function summarize(runs) {
  const summary = {};
  for (const scenario of SCENARIOS) {
    const results = runs.map((run) => run[scenario.name]);
    const samples = results.flatMap((result) => result.samples.slice(5));
    const focus = samples.filter((s) => s.focusMs !== null).map((s) => s.focusMs);
    const frame = samples.filter((s) => s.frameMs !== null).map((s) => s.frameMs);
    const mean = (pick) => results.reduce((a, r) => a + pick(r), 0) / results.length;
    summary[scenario.name] = {
      presses: samples.length,
      moved: samples.filter((s) => s.moved).length,
      panned: samples.filter((s) => s.panned).length,
      noop: samples.filter((s) => s.noop).length,
      // The frame percentiles come from `framesMeasured` presses only.
      framesMeasured: frame.length,
      superseded: samples.filter((s) => s.superseded).length,
      unresolved: samples.filter((s) => s.frameMs === null && !s.superseded && !s.noop).length,
      focusMedianMs: percentile(focus, 0.5), focusP95Ms: percentile(focus, 0.95),
      frameMedianMs: percentile(frame, 0.5), frameP95Ms: percentile(frame, 0.95),
      loafBlockingMsPerRun: mean((r) => r.loaf.blockingMs), loafMaxMs: Math.max(...results.map((r) => r.loaf.maxMs)),
      droppedVideoFramesPerRun: mean((r) => r.video.dropped), presentedFps: mean((r) => r.video.presentedFps),
      scriptMsPerRun: mean((r) => r.cost.ScriptDuration * 1000), layoutMsPerRun: mean((r) => r.cost.LayoutDuration * 1000),
      styleMsPerRun: mean((r) => r.cost.RecalcStyleDuration * 1000), taskMsPerRun: mean((r) => r.cost.TaskDuration * 1000),
      domNodesAtEnd: mean((r) => r.end.Nodes), heapMbAtEnd: mean((r) => r.end.JSHeapUsedSize / 2 ** 20),
    };
  }
  return summary;
}

if (options.merge) {
  // Pools the raw runs of several reports, e.g. one file per paired round.
  const reports = await Promise.all(positionals.map(async (file) => JSON.parse(await readFile(file, "utf8"))));
  console.log(JSON.stringify(summarize(reports.flatMap((report) => report.raw)), null, 2));
  process.exit(0);
}

const { chromium } = await import(options.playwright);
const browser = await chromium.launch({
  executablePath: options.chromium,
  args: ["--autoplay-policy=no-user-gesture-required", "--enable-precise-memory-info"],
});
const runs = [];
try {
  for (let run = 0; run < Number(options.runs); run++) {
    const context = await browser.newContext({ viewport: { width: 960, height: 540 }, deviceScaleFactor: 2, timezoneId: "UTC" });
    await context.addInitScript(pinDate, Date.parse(options.now));
    const page = await context.newPage();
    const cdp = await context.newCDPSession(page);
    await cdp.send("Performance.enable");
    const started = Date.now();
    await page.goto(options.url);
    await openTimeline(page);
    const timelineMs = Date.now() - started;
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: Number(options.throttle) });
    const result = { timelineMs };
    for (const scenario of SCENARIOS) {
      await focusFirstCell(page);
      if (scenario.key === "ArrowUp") {
        for (let i = 0; i < scenario.count; i++) { await press(cdp, "ArrowDown"); await press(cdp, "ArrowDown", { type: "keyUp" }); }
        await page.waitForTimeout(800);
      }
      result[scenario.name] = await runScenario(page, cdp, scenario);
    }
    runs.push(result);
    await context.close();
    process.stderr.write(`run ${run + 1}/${options.runs} done\n`);
  }
} finally {
  await browser.close();
}
const report = { url: options.url, now: options.now, throttle: Number(options.throttle), runs: runs.length, summary: summarize(runs) };
console.log(JSON.stringify(report.summary, null, 2));
if (options.out) await writeFile(options.out, JSON.stringify({ ...report, raw: runs }, null, 2));
