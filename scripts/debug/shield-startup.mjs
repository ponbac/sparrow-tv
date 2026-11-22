// Usage: ANDROID_SERIAL=<Shield serial> node scripts/debug/shield-startup.mjs 5
// Requires Node 22+, adb and an inspection-enabled release build with a catalog.
// Each run force-stops the app. Wall times start just before "am start" is sent.
// The page clock is aligned when CDP delivers the result, so transport adds a
// small error. Anything already rendered at attach is first observed at attach.
import { execFileSync, spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const pkg = "xyz.ponbac.sparrow";
const runs = Number(process.argv[2] ?? 3);
if (!process.env.ANDROID_SERIAL || !Number.isSafeInteger(runs) || runs < 1 || runs > 100) {
  throw new Error("Usage: ANDROID_SERIAL=<Shield serial> node shield-startup.mjs [1-100 runs]");
}

function adb(...args) {
  return execFileSync("adb", args, {
    encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 10000,
  }).trim();
}

// Serialized and run entirely inside the page. Never return source data.
function measurePage() {
  return new Promise((resolve, reject) => {
    const attached = performance.now();
    const tasks = [];
    const taskObserver = new PerformanceObserver(list => {
      for (const entry of list.getEntries()) tasks.push([Math.round(entry.startTime), Math.round(entry.duration)]);
    });
    taskObserver.observe({ type: "longtask", buffered: true });
    let firstShell = null, firstRows = null, firstFocus = null, stable = null;
    let lastKey = "", cls = 0;
    const observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) if (!entry.hadRecentInput) cls += entry.value;
    });
    observer.observe({ type: "layout-shift", buffered: true });

    function tick() {
      const now = performance.now();
      const shell = document.querySelector('.shell[data-pocket="television"]');
      const rows = document.querySelectorAll(".programme-guide__row");
      const focus = document.activeElement;
      if (shell && firstShell === null) firstShell = now;
      if (rows.length && firstRows === null) firstRows = now;
      const ready = rows.length > 0 && focus?.closest(".programme-guide__row")
        && document.querySelector(".programme-guide__board")?.getAttribute("aria-busy") === "false";
      if (ready && firstFocus === null) firstFocus = now;

      const selectors = [".stage", ".programme-guide__board", ".programme-guide__row", ".programme-guide__channel"];
      const key = ready ? selectors.map((selector) => {
        const box = document.querySelector(selector)?.getBoundingClientRect();
        return [box?.x, box?.y, box?.width, box?.height].map(Math.round).join(",");
      }).join("|") : "";
      if (!key || key !== lastKey) stable = key ? now : null;
      lastKey = key;
      if (stable !== null && now - stable >= 600 && document.fonts.status === "loaded") {
        observer.disconnect();
        taskObserver.disconnect();
        const assets = performance.getEntriesByType("resource").flatMap(entry => {
          const url = new URL(entry.name);
          if (url.origin !== location.origin || !/^\/assets\/[\w.-]+\.(js|css|woff2)$/.test(url.pathname)) return [];
          return [[url.pathname.slice(8), Math.round(entry.startTime), Math.round(entry.duration)]];
        });
        resolve({
          attached, fcp: performance.getEntriesByName("first-contentful-paint")[0]?.startTime,
          firstShell, firstRows, firstFocus, stable, done: now, cls,
          rows: rows.length, focus: focus?.classList[0], assets, tasks,
        });
      } else if (now - attached > 20000) {
        observer.disconnect();
        taskObserver.disconnect();
        reject(new Error("Guide did not settle within 20 seconds"));
      } else {
        requestAnimationFrame(tick);
      }
    }
    tick();
  });
}

async function webviewUrl(port, deadline) {
  while (performance.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
        signal: AbortSignal.timeout(1000),
      });
      const pages = await response.json();
      const page = pages.find((target) => target.type === "page" && target.url.includes("tauri"));
      if (page?.webSocketDebuggerUrl) return page.webSocketDebuggerUrl;
    } catch { /* The Activity may have created its process before the WebView. */ }
    await sleep(20);
  }
  throw new Error("No inspectable Sparrow WebView within 12 seconds");
}

async function inspect(url) {
  const socket = new WebSocket(url);
  let timer;
  try {
    return await new Promise((resolve, reject) => {
      timer = setTimeout(() => reject(new Error("WebView probe timed out")), 25000);
      socket.onopen = () => socket.send(JSON.stringify({
        id: 1, method: "Runtime.evaluate", params: {
          expression: `(${measurePage.toString()})()`, awaitPromise: true, returnByValue: true,
        },
      }));
      socket.onmessage = ({ data }) => {
        const message = JSON.parse(data);
        if (message.id !== 1) return;
        const value = message.result?.result?.value;
        if (value && Number.isFinite(value.done)) resolve(value);
        else reject(new Error("Guide did not settle, or CDP evaluation failed"));
      };
      socket.onerror = socket.onclose = () => reject(new Error("WebView connection closed"));
    });
  } finally {
    clearTimeout(timer);
    socket.close();
  }
}

async function measure(run) {
  adb("shell", "am", "force-stop", pkg);
  await sleep(900);
  const start = performance.now();
  const launch = spawn("adb", ["shell", "am", "start", "-W", "-n", pkg + "/.MainActivity"],
    { stdio: ["ignore", "pipe", "ignore"] });
  let launchOutput = "", port;
  launch.stdout.on("data", (chunk) => { launchOutput += chunk; });
  try {
    let pid;
    while (!pid && performance.now() - start < 8000) {
      try { pid = adb("shell", "pidof", pkg); } catch { /* Process starting. */ }
    }
    if (!pid || !/^\d+$/.test(pid)) throw new Error("Could not identify Sparrow process");
    port = adb("forward", "tcp:0", "localabstract:webview_devtools_remote_" + pid);
    const result = await inspect(await webviewUrl(port, start + 12000));
    const offset = performance.now() - start - result.done;
    const times = {};
    for (const key of ["attached", "fcp", "firstShell", "firstRows", "firstFocus", "stable"]) {
      times[key] = result[key] == null ? null : Math.round(offset + result[key]);
    }
    const activity = launchOutput.match(/TotalTime:\s*(\d+)/)?.[1];
    console.log(JSON.stringify({
      run, activity: activity ? Number(activity) : null, pageOriginMs: Math.round(offset),
      ...times, cls: result.cls, rows: result.rows, focus: result.focus,
      // Resource and long-task starts are relative to pageOriginMs.
      assets: result.assets, longTasks: result.tasks,
    }));
  } finally {
    if (launch.exitCode === null) launch.kill();
    if (port) adb("forward", "--remove", "tcp:" + port);
  }
}

for (let run = 1; run <= runs; run++) await measure(run);
