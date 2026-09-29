# Installed catalog startup

The installed app reuses a private processed Channel Catalog, including parsed
contributions and search/schedule indexes. Raw Source Snapshots remain the
fallback. The cache is disposable and never bundled into the app or exported.

- The key binds the Source Configuration, both source checksums, storage format,
  and a build fingerprint of the core Rust sources and locked dependencies.
  An app update that changes these semantics rebuilds automatically.
- A cache hit checks the current snapshot manifests and the cache checksum, then
  restores the prepared catalog without reading/parsing raw source payloads.
  It does not revalidate raw payload bytes: the independently checked derived
  copy remains usable if those bytes are damaged later.
- A cache miss opens saved Channels first. Saved EPG recovery runs on a blocking
  worker after initial status, publishes a new generation when ready, and then
  starts normal network revalidation. Source changes invalidate delayed recovery.
- Cache writes run off the startup/refresh critical path. One worker coalesces
  pending writes to the latest view; closing the core stops pending work.
- `catalog-cache-v1` lives beside private snapshots. It is bounded to 256 MiB,
  created with mode 0600, checksummed and atomically replaced. Missing, damaged,
  incompatible, or unwritable caches do not prevent snapshot recovery.
- Installed queries and mutations run over local IPC even when Android reports
  no network. Hosted HTTP queries retain their normal network policy.

Use `just build-android-debug` for an optimized, debug-signed test APK. The debug
signature supports installing over earlier test builds; Rust optimization is
explicitly enabled. A first launch after an incompatible cache update may show
Channels before Programmes while rebuilding the processed catalog.

## Reproducible validation

Use only synthetic Channels and Programmes. Exercise a large guide (the current
fixture has 1,000 Channels and 96,000 Programmes, about 23 MB of XML). Measure a
force-stop/relaunch with saved sources on the old APK, then a cache miss and warm
cache launch on the new APK. Check offline startup, channel/programme search,
Playback Source resolution through playback, and refresh after source changes.
Never copy provider URLs, cache contents, or raw snapshots into reports.

## Emulator result (2026-09-29)

With the synthetic fixture above, single force-stop/relaunch runs measured from
`am start` to rendered Channel rows:

| Build / state | Launch to rows | Programmes at first render |
| --- | ---: | --- |
| Previous unoptimized APK | 7.064 s | Present |
| Optimized APK, cache miss | 4.363 s | Restored in background |
| Optimized APK, processed cache hit | 3.481 s | Present |
| Processed cache, airplane mode, Wi-Fi/data off | 4.053 s | Present |

The offline run reported `navigator.onLine === false`. Programme search worked
offline, and a cached Channel played through native Media3 after connectivity
was restored. The processed file was 53,662,939 bytes with mode 0600. These are
single-run Android 16 emulator results, including Activity/WebView startup and
instrumentation overhead; they are not a physical-phone benchmark or an
isolated measure of caching versus compiler optimization.

## App icon

The orange sparrow asset is `app/src-tauri/icons/sparrow-foreground.png`.
Generate launcher assets from the committed manifest:

```sh
cd app
bun run tauri icon src-tauri/icons/sparrow-icon.json --output /tmp/sparrow-icons
for density in mdpi hdpi xhdpi xxhdpi xxxhdpi; do
  cp /tmp/sparrow-icons/android/mipmap-$density/*.png src-tauri/gen/android/app/src/main/res/mipmap-$density/
done
cp /tmp/sparrow-icons/icon.png src-tauri/icons/icon.png
```

Keep the hand-maintained adaptive XML files: the pinned Tauri icon generator
applies `android_fg_scale` only to legacy images. The XML insets both adaptive
foreground and monochrome by 17% to fit the safe circle.

Android uses separate dark background, foreground and monochrome layers, with
legacy and round assets for older launchers. Verify circle and rounded-square
masks and themed mode at launcher size.
