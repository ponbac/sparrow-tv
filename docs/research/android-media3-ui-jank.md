# Android Media3 UI-jank research

Date: 2026-08-31

## Question

PR #58's physical-device acceptance run passed MPEG-TS playback and every interaction, but missed Sparrow's UI-frame gate by a small margin: 21 of 979 `dumpsys gfxinfo` frames missed their deadline (2.15%, limit 2%). The same run decoded 6,031 video frames with zero dropped frames and no buffer starvation.

This research asks what Android and comparable open-source video players do differently, and what Sparrow should change or measure next. It deliberately excludes MPV and offline playback.

## Conclusion

Keep `SurfaceView`. The most useful next change is to keep an Activity-scoped `PlayerView` **and** `ExoPlayer` alive across ordinary playback transitions, while keeping the Rust-backed stream/data-source binding short-lived. Replace the media source for channel changes, use Media3 track-selection overrides for audio changes where possible, and reserve full player/view destruction for the Activity lifecycle policy.

The earlier PlayerView-reuse experiment did not test this architecture: it retained the view but still rebuilt `ExoPlayer` and changed view visibility. The comparable applications below avoid that combined churn.

The current 2.15% result is a real miss against Sparrow's strict repository gate, but it is not evidence of broken video rendering. `gfxinfo` measured the app's View/UI frames, whereas the native playback counter measured the `SurfaceView` video frames separately. It is also not an official Android “2% jank” threshold; Google Play's slow-frame vital is a field/session metric with a different definition.

## What the primary sources show

### AndroidX Media3

- [`PlayerView` recommends `SurfaceView` by default](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/libraries/ui/src/main/java/androidx/media3/ui/PlayerView.java#L157-L166). Its `switchTargetView` helper [attaches the player to the new view before detaching the old one](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/libraries/ui/src/main/java/androidx/media3/ui/PlayerView.java#L612-L626), explicitly to make a view switch more efficient and seamless.
- Media3's [`VideoFrameReleaseHelper`](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/libraries/exoplayer/src/main/java/androidx/media3/exoplayer/video/VideoFrameReleaseHelper.java#L308-L327) already derives the content frame rate and calls `Surface.setFrameRate`; it [clears the surface hint when stopped](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/libraries/exoplayer/src/main/java/androidx/media3/exoplayer/video/VideoFrameReleaseHelper.java#L390-L401). Android documents this API as a display-rate hint, not a frame throttle. Forcing Sparrow's whole window to 60 Hz duplicates a concern Media3 already handles and can change the measurement denominator without fixing work on the UI thread.
- The official Media3 demo declares one PlayerView in the Activity layout, initializes a player on the appropriate lifecycle transition, and [sets media items and prepares the same player](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/demos/main/src/main/java/androidx/media3/demo/main/PlayerActivity.java#L264-L295). It [releases at the Activity lifecycle boundary](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/demos/main/src/main/java/androidx/media3/demo/main/PlayerActivity.java#L386-L396), consistent with the [official playback-app lifecycle guidance](https://developer.android.com/media/implement/playback-app).

### Comparable open-source applications

All source links below are pinned to exact commits.

| Application | Surface/view ownership | Media and track changes | Release boundary |
| --- | --- | --- | --- |
| [Just (Video) Player](https://github.com/moneytoo/Player/blob/fb436e14a5cc03998e69a166f00401ddbc71a138/app/src/main/java/com/brouken/player/PlayerActivity.java#L221-L230) | Static Activity-layout PlayerView; SurfaceView normally, with a narrow TextureView workaround for one Xiaomi/API 28 case | [Initializes/rebinds ExoPlayer to the existing view](https://github.com/moneytoo/Player/blob/fb436e14a5cc03998e69a166f00401ddbc71a138/app/src/main/java/com/brouken/player/PlayerActivity.java#L1178-L1261) | [Initialize on start, release on stop](https://github.com/moneytoo/Player/blob/fb436e14a5cc03998e69a166f00401ddbc71a138/app/src/main/java/com/brouken/player/PlayerActivity.java#L720-L759) |
| [Jellyfin Android TV](https://github.com/jellyfin/jellyfin-androidtv/blob/d45a6a19aba6075678354fb62effa1b2b872e651/app/src/main/java/org/jellyfin/androidtv/ui/playback/VideoManager.java#L120-L122) | Binds one PlayerView to its player manager | [Sets and prepares a new MediaItem on the existing ExoPlayer](https://github.com/jellyfin/jellyfin-androidtv/blob/d45a6a19aba6075678354fb62effa1b2b872e651/app/src/main/java/org/jellyfin/androidtv/ui/playback/VideoManager.java#L417-L424); [audio changes use `TrackSelectionOverride`](https://github.com/jellyfin/jellyfin-androidtv/blob/d45a6a19aba6075678354fb62effa1b2b872e651/app/src/main/java/org/jellyfin/androidtv/ui/playback/VideoManager.java#L574-L578) | [Releases when the manager is destroyed](https://github.com/jellyfin/jellyfin-androidtv/blob/d45a6a19aba6075678354fb62effa1b2b872e651/app/src/main/java/org/jellyfin/androidtv/ui/playback/VideoManager.java#L603-L615) |
| [NextPlayer](https://github.com/anilbeesetti/nextplayer/blob/5824581a828e9eb311ac1f8a2141f7825850ef74/feature/player/src/main/java/dev/anilbeesetti/nextplayer/feature/player/PlayerContentFrame.kt) | Uses Media3 `PlayerSurface` with `SURFACE_TYPE_SURFACE_VIEW` | Reuses its service-owned player/controller for media items; [audio changes use `TrackSelectionOverride`](https://github.com/anilbeesetti/nextplayer/blob/5824581a828e9eb311ac1f8a2141f7825850ef74/feature/player/src/main/java/dev/anilbeesetti/nextplayer/feature/player/extensions/Player.kt#L23-L52) | [Activity connects and disconnects from the service/controller](https://github.com/anilbeesetti/nextplayer/blob/5824581a828e9eb311ac1f8a2141f7825850ef74/feature/player/src/main/java/dev/anilbeesetti/nextplayer/feature/player/PlayerActivity.kt#L162-L198); service owns the player |
| [NewPipe](https://github.com/TeamNewPipe/NewPipe/blob/bbbac9b223f21a1a5a714044353cf0412de57b98/app/src/main/java/org/schabi/newpipe/player/playback/SurfaceHolderCallback.java) | Explicit SurfaceHolder lifecycle; substitutes a placeholder surface when the real surface is destroyed | Reattaches the real surface without rebuilding the player | [UI setup/clear owns the surface callbacks](https://github.com/TeamNewPipe/NewPipe/blob/bbbac9b223f21a1a5a714044353cf0412de57b98/app/src/main/java/org/schabi/newpipe/player/ui/VideoPlayerUi.java#L1587-L1625) |

The common shape is not “never release anything.” It is to make player/view ownership broader than one ordinary media or track selection, while giving the media source or surface binding its own narrower lifetime.

## Implemented result

Sparrow now retains one Activity-scoped `PlayerView` and `ExoPlayer` in
[`NativePlaybackController`](../../app/src-tauri/gen/android/app/src/main/java/xyz/ponbac/sparrow/NativePlaybackController.kt),
while each opaque Rust stream owns a shorter-lived media binding, listeners,
and counters. Ordinary media replacement stops and clears the old media source,
then prepares the new source on the retained player. Explicit final Stop and
Activity destruction still release the complete host.

This separates the four lifetimes that were previously coupled:

1. Activity presentation host (`PlayerView` and its attachment to the decor hierarchy).
2. Activity playback engine (`ExoPlayer`).
3. One media binding (`MediaItem`/`MediaSource` and its Rust-backed data source).
4. One Rust stream identity/handle.

Physical-device journey measurement after the change, repeated five times per
warm journey, showed 1.04% modern UI jank for audio replacement and 1.26% for
Channel replacement. The combined strict sample was 13 of 1,133 frames (1.15%).
The same run decoded 6,031 video frames with zero dropped frames and no buffer
starvation over 120.59 seconds. The whole-run diagnostic was exactly 2.00%, but
it still mixed cold launch, OS lifecycle, and a four-frame Stop sample. The
harness therefore records every journey while gating the repeated warm
media-replacement scope described below.

## Implemented architecture

1. Keep the existing SurfaceView and its current geometry. Do not combine the experiment with TextureView or refresh-rate changes.
2. Extract an Activity-scoped native presentation host that owns one PlayerView and one ExoPlayer. Attach the view once. “No presentation” should mean no bound media/stream, not necessarily no Java view object.
3. Give each Rust stream identity its own closeable media binding. For a channel change, stop/clear the old media item, prove the old data source and opaque Rust stream are closed, then bind and prepare the new media source on the retained player. Preserve the existing no-overlap invariant.
4. Sparrow's `PacketSelector` emits video plus only the selected audio PID, so Media3 cannot switch to an audio track that is absent from its input. Keep the Rust transport restart, but replace the retained player's media source rather than rebuilding the player or view. A future change that emits all compatible audio PIDs could map Sparrow's identifiers to Media3 tracks and use `TrackSelectionOverride` instead.
5. For explicit user pause, retaining the player/view while clearing or stopping the media item can preserve Sparrow's requirement to close the live transport. Background lifecycle release may remain stricter if desired; official examples commonly release at `onStop` on modern Android.
6. Measure channel change, audio change, pause/resume, and background/foreground separately. The physical harness repeats warm audio and Channel replacement five times each for the strict 2% UI deadline gate. If another transition needs a release-performance claim, capture a focused Macrobenchmark/Perfetto trace before changing the surface type.

The result supports the retained-host design for media replacement. It does not
claim that object allocation caused every miss in the earlier whole-run sample.

## Measurement changes

Keep the current physical-device acceptance test for functional behavior, privacy, no-overlap, buffering, and video-frame drop assertions. Do not use its single debug-build `gfxinfo` percentage as the sole performance verdict.

Android's rendering documentation notes that View rendering metrics describe the View hierarchy and do not cover every part of rendering; a first frame can also be legitimately slower. The [Macrobenchmark guidance](https://developer.android.com/topic/performance/benchmarking/macrobenchmark-overview) calls for a profileable, non-debuggable, release-like target and repeated measurements. It provides separate [`StartupTimingMetric` and `FrameTimingMetric`](https://developer.android.com/topic/performance/benchmarking/macrobenchmark-metrics) measurements and Perfetto traces.

Recommended split:

- Cold launch: `StartupTimingMetric`, separate from playback actions.
- Steady playback: Media3/native decoded and dropped-frame counters plus buffer starvation.
- Each interaction: a repeatable warm journey with `FrameTimingMetric`, at least five iterations initially.
- Report frame-overrun distributions (median and tail percentiles) and missed-frame count, not only one ratio.
- Retain Perfetto traces locally/private, because traces may contain identifiers or URLs.

The current run resets one `gfxinfo` counter before cold launch, then combines launch, 120 seconds of playback, status/UI polling, every action, and shutdown. Its denominator is sensitive to unrelated View invalidations. This makes a 2.00% boundary unstable and makes the aggregate poor at identifying which transition regressed.

Google Play's [“excessive slow frames” Android vital](https://support.google.com/googleplay/android-developer/answer/9844486) is based on field sessions where a large share of UI frames miss their deadlines. It is not a published requirement that one local run stay below 2%. Sparrow can keep a stricter project-specific target, but it should label it as such and apply it to a repeatable, release-like journey.

## Approaches not supported by the evidence

- Switching to TextureView: Media3 and the comparable applications prefer SurfaceView, and Sparrow's physical trial did not improve the result.
- Forcing the entire app/window to 60 Hz: Media3 already supplies content-rate hints to the video surface, and the physical trial changed the UI-frame denominator without a reliable improvement.
- Inflating the denominator with extra UI refreshes or loosening the threshold only to make the run pass.
- More MPV testing: the remaining failed assertion concerns Android View deadlines in the Media3 path, not MPV playback.

## Primary sources

- [AndroidX Media3 `PlayerView`](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/libraries/ui/src/main/java/androidx/media3/ui/PlayerView.java)
- [AndroidX Media3 `VideoFrameReleaseHelper`](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/libraries/exoplayer/src/main/java/androidx/media3/exoplayer/video/VideoFrameReleaseHelper.java)
- [AndroidX Media3 demo `PlayerActivity`](https://github.com/androidx/media/blob/2bc207851df311340767e913931ca7b28cab1794/demos/main/src/main/java/androidx/media3/demo/main/PlayerActivity.java)
- [Android frame-rate guidance](https://developer.android.com/media/optimize/performance/frame-rate)
- [Android slow-rendering guidance](https://developer.android.com/topic/performance/vitals/render)
- [Android performance measurement overview](https://developer.android.com/topic/performance/measuring-performance)
- [Android Macrobenchmark overview](https://developer.android.com/topic/performance/benchmarking/macrobenchmark-overview)
- [Just (Video) Player source](https://github.com/moneytoo/Player/tree/fb436e14a5cc03998e69a166f00401ddbc71a138)
- [Jellyfin Android TV source](https://github.com/jellyfin/jellyfin-androidtv/tree/d45a6a19aba6075678354fb62effa1b2b872e651)
- [NextPlayer source](https://github.com/anilbeesetti/nextplayer/tree/5824581a828e9eb311ac1f8a2141f7825850ef74)
- [NewPipe source](https://github.com/TeamNewPipe/NewPipe/tree/bbbac9b223f21a1a5a714044353cf0412de57b98)
