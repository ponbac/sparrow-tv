// Runs inside the WebView. Only positions, counts, booleans and timings leave it.
function installWalkProbe(firstPageLast, key = "ArrowDown") {
  const board = document.querySelector(".programme-guide__board");
  const ruler = board.querySelector(".programme-guide__timeline-ruler");
  const row = element => element?.closest(".programme-guide__row");
  const number = element => element ? Number(element.dataset.number) : null;
  const boxVisible = element => {
    const box = element.getBoundingClientRect(), port = board.getBoundingClientRect();
    const top = Math.max(port.top, ruler?.getBoundingClientRect().bottom ?? port.top);
    return box.height > 0 && box.width > 0 && box.top >= top - 1 &&
      box.bottom <= port.bottom + 1 && box.left < port.right && box.right > port.left;
  };
  const samples = [];
  let pending = null;
  const recordKey = event => {
    if (!event.isTrusted || event.key !== key) return;
    const previous = row(document.activeElement);
    const box = previous?.getBoundingClientRect(), port = board.getBoundingClientRect();
    const sample = {
      from: number(previous), to: null, eventMs: event.timeStamp, receivedMs: performance.now(),
      repeat: event.repeat, focusMs: null, frameMs: null,
      atViewportEdge: box ? (key === "ArrowDown" ? box.bottom + box.height > port.bottom + 1
        : box.top - box.height < (ruler?.getBoundingClientRect().bottom ?? port.top) - 1) : false,
      scrolled: false, crossedFirstPage: false, superseded: false,
    };
    pending = { sample, scrollTop: board.scrollTop };
    samples.push(sample);
  };
  const recordFocus = event => {
    const target = event.target, next = row(target);
    if (!next || !pending) return;
    const { sample, scrollTop } = pending;
    if (number(next) === sample.from) return;
    pending = null;
    sample.to = number(next);
    sample.focusMs = Math.round(performance.now() - sample.eventMs);
    sample.crossedFirstPage = sample.from !== null && (sample.from <= firstPageLast) !== (sample.to <= firstPageLast);
    // A guide render or scroll can run after focusin. Check the new focus is visible
    // at a frame boundary, then remains there through a browser paint opportunity.
    const checkFrame = () => {
      if (row(document.activeElement) !== next) { sample.superseded = true; return; }
      if (!boxVisible(target)) {
        if (performance.now() - sample.eventMs < 2500) requestAnimationFrame(checkFrame);
        return;
      }
      requestAnimationFrame(() => {
        if (row(document.activeElement) !== next) { sample.superseded = true; return; }
        if (!boxVisible(target)) { checkFrame(); return; }
        sample.scrolled = board.scrollTop !== scrollTop;
        sample.frameMs = Math.round(performance.now() - sample.eventMs);
      });
    };
    requestAnimationFrame(checkFrame);
  };
  window.addEventListener("keydown", recordKey, true);
  window.addEventListener("focusin", recordFocus, true);
  window.__sparrowGuideWalk = {
    async next(index) {
      const start = performance.now();
      while (performance.now() - start < 2500) {
        const sample = samples[index];
        if (sample && (sample.frameMs !== null || sample.superseded)) return sample;
        await new Promise(resolve => setTimeout(resolve, 16));
      }
      return samples[index] ?? null;
    },
    finish() {
      window.removeEventListener("keydown", recordKey, true);
      window.removeEventListener("focusin", recordFocus, true);
      delete window.__sparrowGuideWalk;
      const origin = samples[0]?.eventMs ?? 0;
      return samples.map(sample => ({ ...sample, eventMs: Math.round(sample.eventMs - origin),
        receivedMs: Math.round(sample.receivedMs - origin) }));
    },
  };
}

async function pageBoundaries(maxChannel = 0) {
  const d = new Date();
  d.setMinutes(d.getMinutes() < 30 ? 0 : 30, 0, 0);
  const board = document.querySelector(".programme-guide__board");
  const span = Number(board.dataset.windowMinutes) || 180;
  const input = { startsAt: d.toISOString(), endsAt: new Date(+d + span * 60000).toISOString(), channelLimit: 40 };
  const ends = [];
  let cursor = null, generation, openingNumbers;
  do {
    const page = await window.__TAURI_INTERNALS__.invoke("catalog_guide_window", {
      input: cursor ? { ...input, channelLimit: 100, cursor } : input,
    });
    if (generation !== undefined && page.generation !== generation) throw Error("Guide changed");
    generation = page.generation;
    if (cursor === null) openingNumbers = [...new Set(page.items.map(item => item.channel.number))];
    const last = page.items.at(-1)?.channel.number;
    if (page.next === null || last === undefined) break;
    ends.push(last);
    if (last >= maxChannel) break;
    cursor = page.next;
  } while (ends.length <= 120);
  return { generation, openingRows: openingNumbers.length, beforeTail: openingNumbers.at(-9), ends };

}

// The last nine displayed families of the original first raw page are the
// same on a complete DOM and on the windowed TV grid.
export async function firstPageLoadTarget(evaluate) {
  const first = await evaluate(`(${pageBoundaries.toString()})()`);
  if (!first.ends.length || !Number.isSafeInteger(first.beforeTail)) return null;
  const loadedBeyond = await evaluate(`(() => {
    const rows = document.querySelector('.programme-guide__rows');
    if (!rows) return false;
    if (!rows.hasAttribute('data-windowed')) {
      return rows.querySelectorAll('.programme-guide__row').length > ${first.openingRows};
    }
    const row = rows.querySelector('.programme-guide__row');
    if (!row) return false;
    const gap = parseFloat(getComputedStyle(rows).rowGap) || 0;
    const pitch = row.getBoundingClientRect().height + gap;
    const lines = Math.round((rows.getBoundingClientRect().height + gap) / pitch);
    const columns = Number(rows.style.getPropertyValue('--guide-columns'));
    return lines > Math.ceil(${first.openingRows} / columns);
  })()`);
  return { number: first.beforeTail, loadedBeyond };
}

// Send key-down repeats at host deadlines even while the renderer is busy.
// Set each event's timestamp on the device clock so queueing appears in the
// input-to-focus latency. Clock alignment error is bounded by the CDP round trip.
async function heldDown(count, interval, { cdp, evaluate, sleep }) {
  const before = performance.now();
  const deviceEpochMs = await evaluate("performance.timeOrigin + performance.now()");
  const after = performance.now();
  const offset = deviceEpochMs - (before + after) / 2;
  const event = (type, autoRepeat = false) => cdp("Input.dispatchKeyEvent", {
    type, key: "ArrowDown", code: "ArrowDown", windowsVirtualKeyCode: 40,
    nativeVirtualKeyCode: 20, autoRepeat, timestamp: (offset + performance.now()) / 1000,
  });
  const outstanding = [], sent = [];
  const start = performance.now();
  try {
    for (let index = 0; index < count; index++) {
      await sleep(Math.max(0, start + index * interval - performance.now()));
      sent.push(Math.round(performance.now() - start));
      // Collect failures without rejecting until all already-sent events settle.
      outstanding.push(event("rawKeyDown", index > 0).then(() => true, () => false));
    }
  } finally {
    await event("keyUp");
  }
  if ((await Promise.all(outstanding)).includes(false)) throw Error("WebView probe failed");
  return { intervalMs: interval, actualSendMs: sent, clockSyncRoundTripMs: Math.round(after - before) };
}

export async function walkGuide({ mode, run, walkSteps, walkInput, repeatMs, adb, cdp, evaluate, focusAt, freshGuide, sleep, picture = null }) {
  const summary = [];
  const stimuli = walkInput === "all" ? ["single", "burst", ...(repeatMs === null ? [] : ["held"])] : [walkInput];
  for (const stimulus of stimuli) {
    await freshGuide();
    if (!await evaluate("document.querySelector('[data-acceptance-group]')?.getAttribute('aria-checked') === 'true'")) {
      throw Error("Guide walk requires All groups");
    }
    if (await evaluate("document.querySelector('.programme-guide').dataset.view") !== mode) {
      await focusAt(".programme-guide__view");
      await adb("shell", "input", "keyevent", "DPAD_CENTER");
      for (let i = 0; i < 60; i++) {
        if (await evaluate("document.querySelector('.programme-guide').dataset.view") === mode) break;
        await sleep(35);
      }
    }
    const cell = ".programme-guide__row .programme-guide__channel";
    if (await evaluate("document.querySelector('.programme-guide').dataset.view") !== mode || !await focusAt(cell)) {
      throw Error("Guide walk setup failed");
    }
    const opening = await evaluate(`(${pageBoundaries.toString()})()`);
    const boundary = opening.ends[0];
    if (!Number.isSafeInteger(boundary)) throw Error("Guide walk needs at least two Channel pages");
    const setup = await evaluate(`(() => {
      const rows = [...document.querySelectorAll('.programme-guide__row')];
      const top = rows[0].getBoundingClientRect().top;
      const nextBand = rows.findIndex(row => row.getBoundingClientRect().top > top + 1);
      return { openingRows: ${opening.openingRows}, columns: ${mode === "now"} ? Math.max(1, nextBand) : 1 };
    })()`);
    const count = walkSteps ?? Math.ceil(setup.openingRows / setup.columns) + 5;
    let pictureBefore = picture && await picture();
    await evaluate(`(${installWalkProbe.toString()})(${boundary})`);
    const tapIndividually = async (code) => {
      for (let i = 0; i < count; i++) {
        await adb("shell", "input", "keyevent", code);
        await evaluate(`window.__sparrowGuideWalk.next(${i})`);
      }
    };
    let steps, pacing = null, outward = null;
    try {
      if (stimulus === "single" || stimulus === "return") {
        await tapIndividually("DPAD_DOWN");
        if (stimulus === "return") {
          outward = await evaluate("window.__sparrowGuideWalk.finish()");
          pictureBefore = picture && await picture();
          await evaluate(`(${installWalkProbe.toString()})(${boundary}, "ArrowUp")`);
          await tapIndividually("DPAD_UP");
        }
      } else if (stimulus === "held") {
        pacing = await heldDown(count, repeatMs, { cdp, evaluate, sleep });
        await evaluate(`window.__sparrowGuideWalk.next(${count - 1})`);
      } else {
        // One real Android input command sends individual Down press/release pairs
        // in succession. eventMs records the delivered cadence of this burst.
        await adb("shell", "input", "keyevent", ...Array(count).fill("DPAD_DOWN"));
        await evaluate(`window.__sparrowGuideWalk.next(${count - 1})`);
      }
    } finally {
      steps = await evaluate("window.__sparrowGuideWalk.finish()");
    }
    const pictureAfter = picture && await picture();
    // Media3's counters around the reported leg, when a Channel plays behind the
    // Guide. Counted only while one player was seen playing the whole leg; its
    // state is polled every 250 ms, so a shorter stall can go unseen. Media3
    // reports drops in batches: a zero delta means none were reported.
    const observedPlaying = Boolean(pictureBefore && pictureAfter && pictureBefore.video !== null &&
      pictureAfter.video === pictureBefore.video && pictureBefore.playing && pictureAfter.playing &&
      !pictureAfter.interrupted);
    const delta = (field) => observedPlaying && pictureBefore[field] !== null && pictureAfter[field] !== null &&
      pictureAfter[field] >= pictureBefore[field] ? pictureAfter[field] - pictureBefore[field] : null;
    const pictureDuringWalk = pictureBefore && pictureAfter ? {
      observedPlayingThroughout: observedPlaying,
      decodedFrames: delta("decoded"),
      reportedDroppedFrames: delta("dropped"),
    } : null;
    // Read boundary positions after the timed walk. Cursor reads here must never
    // prefetch a continuation before the real remote crosses it.
    const maximum = Math.max(0, ...steps.map(s => Math.max(s.from ?? 0, s.to ?? 0)));
    const { generation, ends } = await evaluate(`(${pageBoundaries.toString()})(${maximum})`);
    const comparable = generation === opening.generation && ends[0] === boundary;
    for (const step of steps) {
      step.crossedPageEnds = comparable && step.from !== null && step.to !== null
        ? ends.filter(end => (step.from <= end) !== (step.to <= end)) : [];
    }
    const returning = stimulus === "return";
    const focused = steps.filter(s => s.from !== null && s.to !== null && (returning ? s.to < s.from : s.to > s.from)).length;
    const painted = steps.filter(s => s.frameMs !== null).length;
    const crossed = steps.some(s => s.crossedFirstPage);
    const observed = steps.length;
    const reversal = outward && { retraced: outward.length === count && steps.length === count &&
      steps.every((s, i) => s.to !== null && s.to === outward[count - 1 - i].from) };
    summary.push({ run, action: mode + (returning ? "-up-" : "-down-") + stimulus, stimulus, ...reversal, ...setup, firstPageLast: boundary, measuredPageEnds: ends, comparable,
      requested: count, observed, focused, painted, crossed, pacing, picture: pictureDuringWalk, missingKeys: Math.max(0, count - observed),
      noForwardFocus: observed - focused, supersededBeforePaint: steps.filter(s => s.superseded).length, steps });
    const pictureNote = pictureDuringWalk ? `, picture ${pictureDuringWalk.observedPlayingThroughout ? "seen playing throughout" : "INTERRUPTED"} reported dropped=${pictureDuringWalk.reportedDroppedFrames}/${pictureDuringWalk.decodedFrames}` : "";
    console.log(`${run} ${mode} ${stimulus}: ${focused}/${count} advanced, ${painted} visibly settled, page crossed=${crossed}${pictureNote}`);
  }
  return summary;
}
