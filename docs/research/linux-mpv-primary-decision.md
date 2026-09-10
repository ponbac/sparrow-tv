# Why Linux playback defaulted to system mpv

Date: 2026-09-02

## Question

The first recorded Sparrow Next playback decision was an in-app player as the Primary Playback Engine, with system mpv only as a user-clicked Linux fallback. The installed Linux app now starts mpv for every Playback Session. Why did that change, and do comparable open-source IPTV clients make the same split?

## Conclusion

The original decision still stands as the *first* decision. System mpv became the Linux default later, after the original ADR's revisit gates were crossed on the representative MPEG-TS stream.

On 2026-08-29, [GitHub issue #7](https://github.com/ponbac/sparrow-tv/issues/7) and the first [ADR 0001](../adr/0001-shared-native-http-playback.md) selected `mpegts.js` over a Tauri native byte stream as primary on Linux and Android. Linux mpv was an explicit Fallback Playback Engine: user-authorized, after the primary connection was released, never bundled. Issue #7 recorded the reason mpv was *not* primary: the in-app path worked in the prototype, and a separate mpv window had not earned that cost.

On 2026-08-30, [PR #54](https://github.com/ponbac/sparrow-tv/pull/54) implemented that fallback as an **Open in mpv** button ([issue #31](https://github.com/ponbac/sparrow-tv/issues/31)).

On 2026-09-01, [PR #58](https://github.com/ponbac/sparrow-tv/pull/58) rewrote ADR 0001 and promoted the already-built privacy-safe mpv adapter to the Linux Primary Playback Engine. The measured claim is that the representative Linux MPEG-TS stream held its expected frame rate with no decoder drops in mpv, while WebKit/MSE produced short decoded-frame runs and drops. Raising the native-read cap, coalescing reads, and enabling the `mpegts.js` stash buffer reduced bridge calls without fixing the frame behavior. Android took the parallel native path (Media3) for the same family of failure.

That promotion is what the desktop app does now: Linux `start_playback` always calls `start_mpv_primary`. The **Open in mpv** control is gone; tests assert it is absent. Installed capabilities advertise `mpvFailover: false`. Hosted playback is unchanged and still uses `mpegts.js`.

Comparable Linux IPTV apps split the same way Sparrow did, twice: Electron/WebView clients keep an in-app HTML player as default and offer mpv as backup (IPTVnator). Native live-TV clients whose job is MPEG-TS default to mpv/libmpv (Hypnotix, Cathode, MaxVideoPlayer). Sparrow's current Linux shape is the second camp, with a cheaper separate mpv window rather than an embedded libmpv surface. Embedding remains deferred, not rejected.

## First decision: in-app primary, mpv as a button

Issue #7's recorded resolution (2026-08-29) is explicit:

- **Primary on Linux and Android:** `mpegts.js` over a proxy-free Tauri native HTTP stream.
- **Linux fallback:** already-installed system mpv in its own Wayland window. Failover is always user-invoked and starts only after the primary has fully released its provider connection.
- **Rejected:** “mpv is not the Linux primary because N1 works and the separate-window/native packaging cost has not earned primary status.”
- **Revisit:** “Linux uses manual mpv immediately; repeated N1 failures may justify promoting mpv to primary.”

The first ADR 0001 text (`50994be`, 2026-08-29) used the same split and the same promotion clause:

> Repeated Linux failures may promote mpv from fallback to primary.

The architecture handoff locked the same model the same day: Linux and Android use `mpegts.js` as the Primary Playback Engine; Linux alone offers system mpv as an explicit Fallback Playback Engine; “mpv starts only after explicit user action.”

PR #54 then shipped that button. The installed player queried capabilities for `playbackTransport === "tauri-native-stream"` and `mpvFailover`, and rendered **Open in mpv** only from a failed or primary-stopped session that still allowed failover. Rust kept the Playback Source off argv and JavaScript, sending it over a private JSON IPC socket after mpv was running.

That matches the memory of the first decision.

## Second decision: the original gates were crossed

PR #58 (`3c210e1`, merged 2026-09-01) is the promotion. The rewritten ADR 0001 opens with:

> The measured Linux and Android failures crossed this ADR's original native-engine revisit gates: installed WebKit/MSE playback is no longer the Primary Playback Engine. Linux now uses system mpv and Android uses Media3/ExoPlayer…

Rejected alternatives in that rewrite:

- A shared installed `mpegts.js`/MSE primary. Representative Linux MPEG-TS held expected FPS with no decoder drops in mpv; WebKit repeatedly produced short decoded-frame runs and drops. Android ordinary UI stayed responsive while the same installed media path was choppy.
- Bridge-only fixes (native-read cap, coalesced reads, `mpegts.js` stash buffer). They cut invoke traffic without materially improving Linux frame behavior.
- Passing the Playback Source on mpv's command line (privacy).
- Automatic overlapping fallback (two provider connections).
- Embedding mpv into the WebView window. Deferred: a separately owned system window was the smaller proven Linux boundary.

Current Linux start path in `app/src-tauri/src/runtime.rs`:

```rust
#[cfg(target_os = "linux")]
{
    self.playback
        .start_mpv_primary(session_id, channel_id)
        .await
        .map(InstalledPlaybackStart::LinuxMpv)
}
```

The installed player now labels the surface “Live monitor · system mpv”, stop is **Stop mpv**, and tests assert **Open in mpv** is not in the document. `CapabilitiesDto::installed_catalog` hard-codes `mpv_failover: false`. If system mpv is missing, playback fails with “System mpv is required for Linux playback” rather than falling back to WebKit.

Android is a sibling of the same promotion, not the Linux reason: Media3 over a JNI `DataSource` from the Rust stream actor. Hosted web playback was left on `mpegts.js`.

The Linux video still lives in mpv's own Wayland window. Pause is resource-releasing (reap mpv, close the provider connection, resume at the live edge). That is an accepted consequence of the promotion, not an accident.

## What did not get updated

Several documents still describe the first decision as current:

- [`docs/architecture/sparrow-next-handoff.md`](../architecture/sparrow-next-handoff.md) still says Linux and Android use `mpegts.js` as primary and documents the **Open in mpv** failover flow.
- [ADR 0003](../adr/0003-share-one-core-across-sibling-adapters.md), [ADR 0004](../adr/0004-rewrite-on-a-replacement-branch.md), and [ADR 0005](../adr/0005-build-candidates-on-tags-and-publish-after-device-acceptance.md) still mention Linux mpv failover as a feature of the replacement.
- TypeScript still types `InstalledCapabilities.mpvFailover` as a boolean, and some native-client tests still fixture `mpvFailover: true`, even though the Tauri DTO always returns `false`.

`CONTEXT.md` is still correct at the glossary level: Primary is “attempted first,” Fallback starts only after Primary has stopped or failed. It does not name which engine is which; ADR 0001 does.

Installed WebKit/MSE remains in the tree as a non-primary compatibility path (`native-mpegts-engine`, `presentation: "webview-mse"`). Linux no longer selects it.

## Comparable open-source clients

### In-app HTML default, mpv as backup (Sparrow's first design)

[IPTVnator](https://github.com/4gray/iptvnator) is the closest product analogue: Electron catalog UI, built-in HTML5 / Video.js / ArtPlayer / HLS.js as default, optional external MPV / VLC / IINA, plus experimental embedded mpv.

Its own write-up, [Why some streams need an external player](https://4gray.github.io/iptvnator/blog/why-external-players-help/) (2026-05-14), states the browser-player limit directly: internal players inherit Chromium media support, so MPEG-TS, HEVC, AC-3, and high-bitrate live variants often need MPV/VLC. External open is “one of the intended playback paths,” not a hack. [Issue #585](https://github.com/4gray/iptvnator/issues/585) records that the in-app JS stack does not support MPEG-TS; users send those streams to mpv/VLC. Linux embedded mpv is experimental and currently X11/XWayland-only; native Wayland embedding is not supported.

That is Sparrow's original Linux plan, written down by another IPTV desktop: keep the in-app player for web-friendly streams, keep a one-click native player for MPEG-TS.

[TuxPlayerX](https://github.com/eoliann/TuxPlayerX/releases/tag/v2.0.0) (Tauri + React) makes the same WebView-vs-native split: some IPTV streams fail in WebView with “no supported source,” so it offers **Open in VLC** and a VLC transcode bridge. It notes that Tauri WebView content does not expose a native video surface handle, so in-window libVLC is a later stage.

### mpv/libmpv as the default (Sparrow's current Linux design)

[Hypnotix](https://github.com/linuxmint/hypnotix) is Linux Mint's M3U IPTV player. Playback is libmpv by default; there is no HTML/MSE primary. [Issue #198](https://github.com/linuxmint/hypnotix/issues/198) shows the same separate-window failure mode Sparrow accepted: on some sessions mpv appears as its own window instead of nested in the app. The project workaround is `vo=x11` / `GDK_BACKEND=x11`, which is an embedding/compositor problem, not a “use HTML instead” decision.

Two Tauri IPTV players that are closer to Sparrow's stack than Hypnotix still default to native mpv, not WebView MSE:

- [Cathode](https://github.com/kaiserbh/cathode) (Tauri v2 + Dioxus): “Playback through libmpv's render API… Video draws on a native GL surface behind the transparent webview” (`GtkGLArea` on Linux). No spawned process, no `--wid`.
- [MaxVideoPlayer](https://github.com/MaxMB15/MaxVideoPlayer) (Tauri v2 + React): custom `tauri-plugin-mpv` embeds libmpv (EGL + X11 child window / Wayland subsurface). A spawned mpv window is the fallback if the embedded renderer fails.

Those projects treat MPEG-TS live TV as a native-decoder problem. They still want an in-app picture, so they embed libmpv rather than spawning the CLI player. That is exactly the alternative ADR 0001 deferred.

### Why the second Sparrow decision looks like the mpv-default camp

Sparrow's representative source is live MPEG-TS, not HLS-in-the-browser. IPTVnator's own docs say that combination is where the in-app player loses. Sparrow then *measured* that loss on the target WebKitGTK path and promoted the already-built mpv adapter instead of embedding libmpv.

The remaining product tension is not “mpv vs nothing.” It is:

1. Restore WebKit/MSE as Linux primary only after a packaged-build differential matches the failing representative streams. ADR 0001 says bridge batching or stash-policy changes alone do not meet that gate.
2. Keep mpv as primary but embed it (Cathode / MaxVideoPlayer / Hypnotix). ADR 0001 already lists this as a revisit: reconsider if the separate-window model fails fullscreen, switching, accessibility, or desktop integration, or if an embedded native engine matches representative-stream performance without weakening source privacy or single-owner cleanup.

## Sources

### Sparrow, first decision

- [Issue #7](https://github.com/ponbac/sparrow-tv/issues/7) resolution comment, 2026-08-29.
- `git show 50994be:docs/adr/0001-shared-native-http-playback.md`
- [`docs/architecture/sparrow-next-handoff.md`](../architecture/sparrow-next-handoff.md) (still the first-decision text).
- [Issue #31](https://github.com/ponbac/sparrow-tv/issues/31) and [PR #54](https://github.com/ponbac/sparrow-tv/pull/54), 2026-08-30.
- `git show 794389c:app/src/features/playback/installed-player.tsx` (**Open in mpv**).

### Sparrow, second decision

- [PR #58](https://github.com/ponbac/sparrow-tv/pull/58) / `3c210e1`, 2026-09-01.
- Current [`docs/adr/0001-shared-native-http-playback.md`](../adr/0001-shared-native-http-playback.md).
- `app/src-tauri/src/runtime.rs` (`start_mpv_primary` on Linux).
- `app/src-tauri/src/ipc/dto.rs` (`mpv_failover: false`).
- `app/src/features/playback/installed-player.test.tsx` (asserts **Open in mpv** is absent).

### Comparable projects

- [IPTVnator README](https://github.com/4gray/iptvnator) and [Why some streams need an external player](https://4gray.github.io/iptvnator/blog/why-external-players-help/).
- [IPTVnator #585](https://github.com/4gray/iptvnator/issues/585) (in-app JS does not support MPEG-TS).
- [Hypnotix](https://github.com/linuxmint/hypnotix) and [issue #198](https://github.com/linuxmint/hypnotix/issues/198).
- [Cathode](https://github.com/kaiserbh/cathode).
- [MaxVideoPlayer](https://github.com/MaxMB15/MaxVideoPlayer).
- [TuxPlayerX 2.0.0 notes](https://github.com/eoliann/TuxPlayerX/releases/tag/v2.0.0).
