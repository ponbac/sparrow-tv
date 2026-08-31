# Android MPEG-TS playback performance acceptance

This runbook validates the installed Android native playback path on the same
physical target owned by
[`android-catalog-performance.md`](android-catalog-performance.md). It is a
candidate test, not release authorization. It does not create signing material,
publish an artifact, or approve a tag.

## Prepare the representative source

Use the private catalog configured for the catalog acceptance, but keep the
device online for playback. The first two browseable entries have a deliberate
role that the harness can exercise without reading or recording their content:

1. The first entry is the representative MPEG-TS stream that previously showed
   choppy playback. It must expose at least two compatible audio tracks.
2. The second entry is another representative MPEG-TS stream used to prove a
   deterministic Channel switch.

Neither entry's name, identifier, provider location, media metadata, or track
labels is returned by the probes. If the private catalog does not put suitable
entries in these positions, prepare a private two-entry acceptance catalog on
the device; do not add it to the repository or evidence.

Build a debuggable API-36 APK containing the arm64 Sparrow runtime. The harness
uses the debuggable WebView only for strict aggregate DOM markers. It stages a
private read-only copy, hashes it, installs it with `adb install -r`, and checks
the installed package identity. Reinstallation retains an existing compatible
app data directory, but the operator should confirm the configured catalog is
still present before starting.

Run from the repository root with an unused evidence path:

```sh
ANDROID_SERIAL=EXPLICIT_ADB_SERIAL \
ANDROID_PLAYBACK_ACCEPTANCE_APK=/absolute/path/to/app-universal-debug.apk \
ANDROID_PLAYBACK_ACCEPTANCE_OUTPUT=artifacts/android-playback-acceptance.json \
  mise exec -- just android-playback-accept
```

The standard `ANDROID_SERIAL` environment selector chooses the target while
keeping the private identifier out of process arguments and evidence. The
harness rejects emulators, the wrong physical model, the wrong API level,
non-arm64 devices, non-debuggable candidates, and evidence paths that already
exist.

## Silent playback

The harness launches the debuggable candidate with the fixed
`xyz.ponbac.sparrow.ACCEPTANCE_SILENT` boolean Activity extra. Debug builds use
that extra to clamp the Media3 instance to mute and zero volume before its media
item is prepared; release builds ignore it, so normal product audio defaults are
unchanged. The harness waits for the aggregate native `silent=true` status,
then also sets the player UI's own mute and volume controls. This is a
per-process silent sink. The harness never changes Android's global media
volume and never mutes the user's whole device.

Every sustained sample must continue to report the effective native silent
state together with `muted=true` and `volume=0` in the WebView controls. A
candidate that can emit audio before the debug sink is applied is rejected
rather than worked around with a system-wide setting.

## What the harness exercises

After resetting `dumpsys gfxinfo`, the harness launches Sparrow process-cold,
waits for at least two private Channel cards, selects the first without reading
its text, and requires `android-media3` to reach a buffered playing state. It
then samples aggregate native status once per second for at least 120 seconds.

The sustained gate requires uninterrupted playing state, a nonzero Media3
buffer at every sample, and monotonic counters. When the candidate exposes the
aggregate decoded-frame count, it must sustain at least 20 decoded frames per
second and keep the dropped-frame ratio at or below 1%. A legacy candidate with
no decoded counter is limited to 12 dropped frames instead. A missing counter is
recorded as `null`, never invented from UI timing.

The same process then proves:

- pause releases the native presentation and resume returns to buffered play;
- sending the activity to the background releases presentation ownership and
  foregrounding it resumes at the live edge;
- a second audio option is selected without reading its value or label, followed
  by confirmed presentation replacement and successful playback recovery;
- selecting the second Channel returns to buffered play; and
- Stop removes both the native presentation marker and the installed player UI.

The harness reads and resets Android's modern `Janky frames` deadline metric at
each journey boundary. It deliberately ignores `Janky frames (legacy)` and
records separate counters for cold startup, sustained playback, pause/resume,
background/foreground, audio selection, Channel switching, and Stop. These UI
deadline counters complement Media3's dropped-frame and buffer counters; they
are not treated as decoded-video FPS.

The strict UI deadline gate covers repeatable warm media replacement. The
harness changes audio five times on the first representative stream and then
switches between the two representative Channels five times. Their counters
are combined from frame counts, never by averaging percentages. This scoped
sample must contain at least 100 UI frames and at most 2% modern jank. All five
iterations of every action must also pass their functional ownership and
playback-recovery checks.

Cold startup, steady playback, pause/resume, background/foreground, and Stop
remain explicit per-journey diagnostics, but they are not folded into that
percentage. A process-cold debug launch, an OS task transition, four Stop
frames, and warm in-app media replacement are not interchangeable performance
samples. For a release-performance claim, follow the Macrobenchmark split in
[`android-media3-ui-jank.md`](../research/android-media3-ui-jank.md) with a
profileable release-like candidate and repeated startup/frame timing traces.

## Privacy and cleanup

Evidence contains the candidate digest and version, fixed gate configuration,
aggregate duration/frame/buffer results, per-journey and gated warm-replacement
UI counters, booleans for each interaction, and a safe failure code. It contains
no provider URL, credentials, Channel or track names, catalog values, adb
serial, signing identity, socket endpoint, screenshots, logcat, or raw WebView
and `dumpsys` output. Strict marker parsing rejects extra fields so an
accidental private value cannot be projected into evidence.

The harness force-stops Sparrow before and after the observation, closes its
debug session, removes its exact adb forward, and deletes its private staged APK.
On rejection, check that no player remains before investigating. Preserve the
JSON rejection record because its aggregate failure tag identifies which gate
needs a new measured differential without disclosing the source.
