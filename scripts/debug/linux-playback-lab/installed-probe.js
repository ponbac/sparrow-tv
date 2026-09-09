// Evaluated only by an explicitly enabled candidate build. Selects the first
// visible Channel; use an isolated catalog with private sample aliases.
(() => {
  const probe = window.__sparrowPlaybackLab ??= {
    selected: false, video: null, presented: 0,
    frameGaps: [], mediaGaps: [], rafGaps: [], seeks: [], waiting: 0,
    seeking: 0, seeked: 0, lastFrame: null, lastMedia: null,
    lastRaf: null, installed: false,
  };
  if (!probe.installed) {
    probe.installed = true;
    const currentTime = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'currentTime');
    if (currentTime?.get && currentTime?.set) {
      Object.defineProperty(HTMLMediaElement.prototype, 'currentTime', {
        ...currentTime,
        set(value) {
          const before = currentTime.get.call(this);
          const end = this.buffered.length ? this.buffered.end(this.buffered.length - 1) : 0;
          probe.seeks.push({ from: before, to: value, bufferEnd: end });
          currentTime.set.call(this, value);
        },
      });
    }
    const raf = now => {
      if (probe.lastRaf !== null) probe.rafGaps.push(now - probe.lastRaf);
      probe.lastRaf = now;
      requestAnimationFrame(raf);
    };
    requestAnimationFrame(raf);
  }
  if (!probe.selected) {
    const channel = document.querySelector('button[aria-label^="Tune "]');
    if (channel) { channel.click(); probe.selected = true; }
  }
  const video = document.querySelector('video');
  if (video && probe.video !== video) {
    probe.video = video;
    probe.presented = 0;
    for (const name of ['waiting', 'seeking', 'seeked']) video.addEventListener(name, () => probe[name]++);
    const frame = (now, metadata) => {
      if (probe.video !== video) return;
      probe.presented++;
      if (probe.lastFrame !== null) probe.frameGaps.push(now - probe.lastFrame);
      if (probe.lastMedia !== null) probe.mediaGaps.push((metadata.mediaTime - probe.lastMedia) * 1000);
      probe.lastFrame = now;
      probe.lastMedia = metadata.mediaTime;
      video.requestVideoFrameCallback(frame);
    };
    if (video.requestVideoFrameCallback) video.requestVideoFrameCallback(frame);
  }
  if (__SWITCH_PLAYERS__) {
    const label = __ELAPSED__ >= 50 ? 'Play in app' : __ELAPSED__ >= 20 ? 'Open in mpv' : null;
    const key = __ELAPSED__ >= 50 ? 'returnedToApp' : 'openedMpv';
    const button = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === label);
    if (button && !button.disabled && !probe[key]) {
      probe[key] = true;
      button.click();
    }
  }
  const quality = video?.getVideoPlaybackQuality();
  const state = document.querySelector('.hosted-player__state')?.dataset.state;
  const distribution = values => {
    values.sort((a, b) => a - b);
    const result = { count: values.length, p50: values[Math.floor(values.length * .5)] ?? 0,
      p95: values[Math.floor(values.length * .95)] ?? 0, max: values.at(-1) ?? 0 };
    values.length = 0;
    return result;
  };
  const seekWrites = probe.seeks.splice(0);
  return {
    frameGaps: distribution(probe.frameGaps), mediaGaps: distribution(probe.mediaGaps),
    rafGaps: distribution(probe.rafGaps), seekWrites,
    waiting: probe.waiting, seeking: probe.seeking, seeked: probe.seeked,
    bufferAhead: video?.buffered.length ? video.buffered.end(video.buffered.length - 1) - video.currentTime : 0,
    elapsed: __ELAPSED__, selected: probe.selected,
    fullscreen: document.fullscreenElement !== null,
    mpv: !![...document.querySelectorAll("button")].find(b => b.textContent.trim() === "Stop mpv"),
    playing: state === 'playing', failed: state === 'failed',
    time: video?.currentTime ?? 0, total: quality?.totalVideoFrames ?? 0,
    dropped: quality?.droppedVideoFrames ?? 0, presented: probe.presented,
    ready: video?.readyState ?? 0, paused: video?.paused ?? true,
    width: video?.videoWidth ?? 0, height: video?.videoHeight ?? 0,
  };
})()
