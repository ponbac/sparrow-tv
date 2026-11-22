# NVIDIA Shield TV performance

Measured 2026-11-22 on a physical arm64 SHIELD Android TV, Android 11 (API 30), WebView 157.0.8103.52, 1080p app surface, 59.94 Hz display. Baseline was the installed Sparrow 0.11.4 release, equivalent to repository commit `a9e1d39`. The same saved Channel Catalog exposed 155 Channel Group controls including All. Both builds used the installed signing certificate and in-place updates.

The [final installed results](#final-installed-results) cover startup, Down from the bottom visible entry, held-key response and the complete remote tour. The [second pass](#second-pass-on-the-shield) describes the experiments behind those measurements.

## Final installed results

The benchmark candidate was installed over the existing app. The measurements below precede the review fixes to long-page focus handoff and reuse of a completed startup status read. The installed APK matched the verified ordinary release. WebView inspection was disabled. Its APK SHA-256 was `a5f69b7df0c640ad654c710410dc9f609b82bf80144235e103cee3626195caea`.

| Startup milestone | Original median | First pass median | Installed median | Installed samples (ms) |
|---|---:|---:|---:|---|
| First contentful paint | 1472 | 2005 | 1696 | 1696, 1631, 1701, 1610, 1704 |
| TV shell in DOM | 1858 | 1547 | 1577 | 1577, 1530, 1600, 1497, 1605 |
| Real Channel rows in DOM | 2353 | 2006 | 1800 | 1800, 1751, 1817, 1714, 1825 |
| Focused Guide with stable geometry | 2547 | 2178 | 1926 | 1926, 1875, 1940, 1839, 1951 |
| Settled toolbar, recorded pixels | 1718 | 1451 | 1468 | 1485, 1468, 1468, 1416, 1451 |
| Settled visible focus | 2419 | 1918 | 1751 | 1785, 1752, 1751, 1700, 1735 |
| Settled visible Guide | 2786 | 2319 | 2085 | 2085, 2102, 2068, 2051, 2085 |
| Home to settled Guide, same running process | — | — | 268 | 268, 267, 268 |

The WebView time origin fell 743–824 ms after the launch request (median 785 ms). The layout-shift observer reported zero in all five launches. The visible Guide timings were measured from recordings of the ordinary release.

### Down across the guide

The comparison build rendered every loaded Channel. Both builds used the same Shield and saved catalog. Each run began with a newly loaded page. The installed build renders nearby Channels, retains the focused and playing Channels, and updates the programme preview after the new focus has had an opportunity to paint. Playback selection still follows the keypress directly.

| Physical Down action | Before | Installed |
|---|---:|---:|
| Now, Down from bottom visible entry, median | 180 ms | 90 ms |
| Now, Down from bottom visible entry, p95 | 241 ms | 101 ms |
| Now, cross first catalog page, both samples (ms) | 222, 179 | 86, 85 |
| Timeline, Down from bottom visible entry, median | 209 ms | 110 ms |
| Timeline, Down from bottom visible entry, p95 | 332 ms | 134 ms |
| Timeline, cross first catalog page, both samples (ms) | 219, 213 | 127, 131 |

Across the two runs, 14 Now presses and 32 Timeline presses began on the bottom visible entry in each build. The page-crossing samples use the catalog cursor and can occur on a different press.

The held-key test scheduled WebView input every 125 ms. Each focus change is counted separately from a highlight that stayed visible for a paint opportunity.

| Eight Down events per second, two runs | Before | Installed |
|---|---:|---:|
| Now, advanced focus | 59/72 | 72/72 |
| Now, highlight survived a paint opportunity | 8/72 | 60/72 |
| Now, median input to focus | 623 ms | 47 ms |
| Now, p95 input to focus | 1,230 ms | 77 ms |
| Now, last key to settled focus, both runs (ms) | 1,739, 1,550 | 130, 142 |
| Timeline, advanced focus | 132/160 | 160/160 |
| Timeline, highlight survived a paint opportunity | 5/160 | 138/160 |
| Timeline, median input to focus | 1,658 ms | 33 ms |
| Timeline, p95 input to focus | 5,035 ms | 102 ms |
| Timeline, last key to settled focus, both runs (ms) | 5,437, 5,421 | 182, 114 |

The Shield input dispatcher reported a 400 ms initial hold delay and 50 ms between repeats. A separate stress run scheduled trusted WebView events at 50 ms intervals from the first event, twice per view. This exercises steady-state cadence immediately and excludes Bluetooth and the initial hold delay. Every scheduled event reached the WebView. After 36 Now events from Channel 1, focus reached Channels 97 and 91 (Channel 109 at one three-column move per event) and settled 375 and 512 ms after the last key. After 80 Timeline events both runs reached Channel 81, settling in 384 and 345 ms. The Guide bounds outstanding movement to eight unseen steps when waiting for rows. At this cadence, groups of events can share an intermediate focus change.

The separate Android burst used 11 Down events in Now and 22 in Timeline from Channel Number 1. These commands sent press and release pairs through Android’s input system. Both runs landed on Channel 34 in Now and Channel 23 in Timeline. Bursts can coalesce intermediate paint opportunities.

On the return trip, individual Android Up presses retraced every Channel Number from the outward path. Now returned from 34 to 1 in 11 presses, with an 85 ms median to visible focus. Timeline returned from 23 to 1 in 22 presses, with a 129 ms median. Both crossed the catalog page boundary on the way back.

### Complete remote tour

Times are milliseconds. Each main tour used a new process with the same saved catalog. Search includes typing. Live playback includes the network response and first decoded frame observation. Channel Up and Quality Variant timings varied across live tunes. Group selection also includes the time to a populated, nonbusy board.

| Action | Original median | First pass median | Final median | Final samples (ms) |
|---|---:|---:|---:|---|
| Now grid, Right | 100 | 92 | 45 | 44, 46, 45 |
| Now grid, Down | 86 | 80 | 39 | 39, 39, 38 |
| Next Channel Group focus | 82 | 39 | 35 | 31, 43, 35 |
| Select group | 88 | 60 | 119 | 121, 119, 119 |
| Return to All | 83 | 52 | 116 | 118, 116, 114 |
| Open Timeline | 420 | 347 | 224 | 225, 224, 223 |
| Timeline, Down | 177 | 161 | 47 | 49, 47, 46 |
| Timeline, Right | 168 | 164 | 48 | 48, 49, 46 |
| Return to Now | 206 | 151 | 123 | 120, 125, 123 |
| Load next Channel page | 420 | 402 | 255 | 261, 251, 255 |
| Open group chooser | 451 | 439 | 411 | 415, 411, 408 |
| Close group chooser | 345 | 333 | 224 | 229, 224, 222 |
| Open Sources | 157 | 145 | 102 | 102, 105, 100 |
| Close Sources | 268 | 245 | 124 | 125, 124, 124 |
| Activate search input | 45 | 38 | 46 | 99, 46, 36 |
| Four letters to visible results | 873 | 862 | 465 | 456, 471, 465 |
| Open full Channel search | 307 | 290 | 195 | 201, 191, 195 |
| Close full Channel search | 284 | 269 | 168 | 168, 203, 165 |
| Clear search | 259 | 241 | 118 | 120, 117, 118 |
| Start docked playback | 2,545 | 2,772 | 2,640 | 2718, 2640, 2365 |
| Switch playing Channel | 2,434 | 2,948 | 3,038 | 2361, 3038, 3086 |
| Enter fullscreen | — | 1,535 | 177 | 177, 165, 206 |
| Focus fullscreen controls | — | 168 | 112 | 104, 187, 112, 111, 153, 109 |
| Back to guide | — | 753 | 311 | 311, 328, 308 |
| Pause | 350 | 351 | 226 | 228, 222, 226 |
| Resume to decoded video | 2,227 | 2,171 | 2,151 | 2151, 2218, 1870 |
| Channel Up to decoded video | 2,574 | 2,615 | 2,310 | 2310, 2268, 2427 |
| Channel Down to decoded video | 2,723 | 2,786 | 2,766 | 2259, 2766, 2848 |
| Re-enter fullscreen for Stop | — | 1,513 | 176 | 176, 174, 209 |
| Stop playback | — | 1,006 | 573 | 573, 572, 596 |
| Change Quality Variant | 2,384 | 2,221 | 1,996 | 1915, 1996, 2254 |
| Restore Quality Variant | 2,324 | 2,357 | 1,973 | 1973, 1933, 3613 |
| Toggle mute | — | 73 | 77 | 82, 74, 77 |
| Restore mute | — | 72 | 70 | 70, 68, 71 |

All 120 action observations succeeded across three fresh-process tours and the separate controls and Quality Variant runs. Selecting a Channel Group took 119 ms, up from 60 ms in the first pass. Returning to All took 116 ms, up from 52 ms. Live docked playback started in 2.64 s and switching a playing Channel through the Guide took 3.04 s at the median. The corresponding original medians were 2.55 s and 2.43 s. The three live samples per action include provider and keyframe variability.

The TV Guide remounts its rows when the Channel Group changes. This gives the new group a fresh scroll position and clears pending navigation from the previous group. Remount and layout measurement are plausible contributors to the extra group-selection time. The tour measures the entire action, so it cannot establish their share. We kept the measured improvement on repeated Down and page crossings and accepted the group-selection cost for this release. A focused comparison would keep row measurements across group changes and check both the timing and focus when returning to All.

## First-pass cold startup

| Cold start milestone | Baseline samples (ms) | Optimized samples (ms) | Baseline median | Optimized median |
|---|---|---|---:|---:|
| First contentful paint | 1472, 1477, 1689, 1392, 1439 | 2044, 2005, 2015, 2001, 1959 | 1472 | 2005 |
| TV shell present | 1858, 1881, 2066, 1786, 1804 | 1563, 1547, 1551, 1533, 1502 | 1858 | 1547 |
| Channel rows present | 2353, 2385, 2525, 2297, 2299 | 2045, 2006, 2021, 2002, 1958 | 2353 | 2006 |
| Focusable guide with stable geometry | 2547, 2585, 2721, 2494, 2494 | 2215, 2178, 2187, 2171, 2120 | 2547 | 2178 |
| Visible toolbar settled | 1933, 1718, 1735, 1701, 1718 | 1818, 1451, 1451, 1435, 1434 | 1718 | 1451 |
| Visible focus highlight settled | 2467, 2419, 2419, 2385, 2368 | 2002, 1918, 1935, 1918, 1901 | 2419 | 1918 |
| Visible guide settled | 2834, 2853, 2786, 2769, 2769 | 2402, 2319, 2335, 2302, 2302 | 2786 | 2319 |

The first four rows use the host launch request as zero. The page probe attaches before the shell appears, samples animation frames and translates page time into launch elapsed time using the reply timestamp. Stable means the board was populated, no longer busy, a Channel had focus, and the stage, board, first row and first Channel button held the same bounds for at least 600 ms. The timestamp is the beginning of that interval. First contentful paint includes loading content on the baseline. Each run force-stopped the process with the saved catalog and disk caches retained.

The last three rows use recorded pixels from the two ordinary release APKs. Their zero is the first sustained visual change away from Home. Settled means the toolbar text, lower grid and cyan focus highlight match their final positions and appearance within the thresholds in the script. Remote logos, video encoding and the changing live guide can affect this measure. The tools use different clocks and endpoints.


Android Activity draw times were measured separately without recording or inspection.

| Activity launch | Baseline samples (ms) | Optimized samples (ms) | Baseline median | Optimized median |
|---|---|---|---:|---:|
| Force-stop and start | 582, 392, 343, 312, 333 | 404, 343, 339, 314, 340 | 343 | 340 |
| Home and return | 104, 67, 75, 79, 87 | 138, 108, 111, 123, 102 | 79 | 111 |

## Remote actions

| Action | Baseline samples (ms) | Optimized samples (ms) | Baseline median | Optimized median |
|---|---|---|---:|---:|
| Now grid, Right | 108, 100, 98 | 90, 92, 92 | 100 | 92 |
| Now grid, Down | 87, 86, 85 | 81, 79, 80 | 86 | 80 |
| Move focus to next Channel Group | 82, 83, 80 | 39, 43, 37 | 82 | 39 |
| Select Channel Group | 88, 75, 91 | 61, 60, 54 | 88 | 60 |
| Return to All groups | 97, 83, 81 | 52, 51, 60 | 83 | 52 |
| Open Timeline | 420, 418, 422 | 348, 345, 347 | 420 | 347 |
| Timeline, Down | 172, 177, 177 | 169, 160, 161 | 177 | 161 |
| Timeline, Right | 171, 168, 168 | 171, 162, 164 | 168 | 164 |
| Return to Now | 199, 208, 206 | 151, 151, 151 | 206 | 151 |
| Load next Channel page | 420, 422, 411 | 403, 402, 397 | 420 | 402 |
| Open group chooser | 451, 454, 450 | 439, 436, 442 | 451 | 439 |
| Close group chooser | 347, 342, 345 | 334, 330, 333 | 345 | 333 |
| Open Sources | 159, 157, 156 | 145, 144, 146 | 157 | 145 |
| Close Sources | 268, 264, 272 | 245, 239, 252 | 268 | 245 |
| Activate search input | 34, 45, 49 | 46, 36, 38 | 45 | 38 |
| Type four letters to inline results | 871, 873, 893 | 880, 862, 853 | 873 | 862 |
| Open full Channel search | 311, 301, 307 | 368, 284, 290 | 307 | 290 |
| Close full Channel search | 302, 278, 284 | 269, 261, 342 | 284 | 269 |
| Clear search | 260, 259, 257 | 243, 240, 241 | 259 | 241 |
| Start docked playback | 2065, 2816, 2545 | 2821, 2772, 2469 | 2545 | 2772 |
| Switch playing Channel in guide | 1669, 2776, 2434 | 2948, 3206, 2154 | 2434 | 2948 |
| Enter fullscreen | timeout, timeout, timeout | 1535, 1453, 1697 | — | 1535 |
| Focus fullscreen controls | — | 167, 170, 251, 166, 173, 164 | — | 168 |
| Back from fullscreen | — | 753, 745, 756 | — | 753 |
| Pause playback | 371, 350, 338 | 354, 337, 351 | 350 | 351 |
| Resume to decoded video | 1910, 2227, 2704 | 2113, 2472, 2171 | 2227 | 2171 |
| Channel Up to decoded video | 2226, 3143, 2574 | 2615, 2591, 3036 | 2574 | 2615 |
| Channel Down to decoded video | 2032, 2723, 3128 | 2481, 3022, 2786 | 2723 | 2786 |
| Enter fullscreen again for Stop | timeout, timeout, timeout | 1525, 1470, 1513 | — | 1513 |
| Stop playback | target-unavailable, target-unavailable, target-unavailable | 1006, 1014, 992 | — | 1006 |
| Change Quality Variant | 2660, 2198, 2384 | 2356, 2221, 2074 | 2384 | 2221 |
| Restore Quality Variant | 2187, 2324, 2476 | 2357, 2306, 2754 | 2324 | 2357 |
| Toggle mute | — | 72, 73, 78 | — | 73 |
| Restore mute | — | 78, 72, 71 | — | 72 |

Rows report milliseconds from the first WebView input event or matching native bridge message through the observed state and one paint opportunity. Setup focus used CDP. Measured actions used Android remote key events, including native media and Channel keys. Input-to-observation excludes host command dispatch. Search includes typing all four characters. Group selection requires the checked state and populated, nonbusy board. Fullscreen timing observes the document entering fullscreen. It can include the WebView resize. Playback timing requires a new video element, Media3 playing and a positive decoded-frame counter. Native playback status is sampled at 250 ms intervals. Live network delivery and keyframe timing contribute to the results.

Each main tour began in a newly started process with the initial guide settled. The same two visible guide row indices, 1 and 2 (zero-based), were used in the same order. Quality was measured in a separate three-run pass that restored the original preference. A dash means there was no successful timed sample. Controls hidden by the baseline fullscreen failure could not be exercised with the remote. Muting was measured inside the restored fullscreen controls. The selected live Channel observed during playback exposed a single audio track.

## Changes on the Shield

The TV mounts the actual guide shell as the local catalog opens. It starts the status read before the first React commit and shares the configured generation’s first guide and group requests with the mounted UI. An unconfigured device still skips those catalog requests. The guide keeps the board busy until a Channel can take focus.

TV Channel Groups use buttons with radio semantics and the existing D-pad navigation. Moving focus checks the next candidate for visibility. The original registered all 155 controls with a second keyboard controller. The installed player now loads the WebView MPEG-TS decoder only for a session that uses it. The Shield uses Media3 and its first focus avoided about 195 kB of uncompressed JavaScript.

The baseline release had lost one of the two WebChromeClient callbacks Android inspects for fullscreen support. The release shrinker now preserves both. Fullscreen, controls, Back and Stop completed on the device in all three tours. On a shorter guide the separate controls tour entered fullscreen at a median of 398 ms. After loading the next page, the median was 1,535 ms. The larger guide is a distinct workload.

## Playback buffer experiment

The same optimized build was tested with Media3’s default 1,000 ms initial streaming threshold and a temporary 500 ms threshold. Four blocks were run in default, shorter, shorter, default order. Each started from a new process, tuned two Channels in alternating order, and sampled playback for six seconds after each tune. The rebuffer threshold was identical.

| Initial threshold | Remote event to decoded playback (ms), in order | Median | Median to Media3 marker | Decoded frames during follow-up | Dropped frames |
|---|---|---:|---:|---:|---:|
| 1,000 ms | 2313, 2191, 1936, 2183, 2330, 2402, 1894, 2218 | 2204.5 | 1623 | 2306 | 0 |
| 500 ms | 2318, 2584, 2177, 2251, 2603, 2158, 2004, 1876 | 2214 | 1759.5 | 2338 | 0 |

Both settings sustained playback throughout the sampled windows. The shorter buffer showed no end-to-end improvement at this resolution, so the installed optimized release uses the default threshold. The time before the Media3 marker includes the app and transport startup. This test does not isolate provider latency from local work.

The production APK SHA-256 values were `c026520c66b13e01c1aa9e8f7599296ede9aadc99fb76355c31b3447f893e580` for the baseline and `6e557e9b507ce98d37a3f4f156a2442841bf315a5f49140551b50e114b918daf` for the optimized build.

## Repeating the measurements

Set `ANDROID_SERIAL` to the intended Shield listed by `adb devices`. Configure the app with a catalog before benchmarking. The action and DOM probes need an inspection release build with WebView debugging enabled locally; the pixel benchmark works on a normal release. The inspection builds change the Kotlin WebView debugging guard. Their embedded frontend and native library match their corresponding production APK byte for byte. Leave the guard intact for distribution.

```sh
python3 scripts/debug/shield-startup.py --runs 5
node scripts/debug/shield-startup.mjs 5
adb shell am start -S -W -n xyz.ponbac.sparrow/.MainActivity
# Once the guide has loaded, use a fresh output file for each run.
node scripts/debug/shield-benchmark.mjs --scenario all --channels 1,2 --runs 1 --out tour.json
node scripts/debug/shield-benchmark.mjs --scenario quality --channels 1,2 --runs 3 --out quality.json
node scripts/debug/shield-benchmark.mjs --scenario controls --channels 1,2 --runs 3 --out controls.json
node scripts/debug/shield-benchmark.mjs --scenario tune --channels 1,2 --runs 1 --out tune.json
node scripts/debug/shield-benchmark.mjs --scenario catalog-search --runs 3 --out search.json
python3 scripts/debug/shield-startup.py --warm --runs 3
```

For a guide browsing comparison, let each installed build finish restoring its saved catalog. Open the populated Guide with All groups selected. Repeat these commands on the before and after builds with separate output paths and identical step counts.

```sh
node scripts/debug/shield-benchmark.mjs --scenario browse-now --browse-steps 36 --runs 1 --out now.json
node scripts/debug/shield-benchmark.mjs --scenario browse-timeline --browse-steps 80 --runs 1 --out timeline.json
node scripts/debug/shield-benchmark.mjs --scenario browse-timeline --browse-steps 80 --browse-input held --browse-repeat-ms 125 --runs 1 --out timeline-8hz.json
node scripts/debug/shield-benchmark.mjs --scenario browse-now --browse-steps 36 --browse-input held --browse-repeat-ms 50 --runs 2 --out now-20hz.json
node scripts/debug/shield-benchmark.mjs --scenario browse-timeline --browse-steps 80 --browse-input held --browse-repeat-ms 50 --runs 2 --out timeline-20hz.json
node scripts/debug/shield-benchmark.mjs --scenario browse --browse-input return --runs 1 --out guide-return.json
```

Each walk reloads the WebView and starts at the first Channel. It measures separate Android Down taps, then repeats on another fresh page using a single Android input command containing the same number of taps. The latter exercises queued input. Its recorded event timestamps show the actual cadence. Each step records input to focus, then input to a paint opportunity with the same focus visible inside the board. A later key can supersede an intermediate focus before it paints. The report also counts missing key events and observed keys that failed to move focus forward. It identifies scrolling steps and crossings of logical Channel Catalog pages using the actual page cursors after the timed walk. Compare runs marked `comparable` with the same page boundaries.

The optional held-key run uses trusted WebView input at a fixed cadence. At 125 ms, it sends eight Down events per second, marks all after the first as repeats and releases the key at the end. Host send times, WebView event receipt times and the clock alignment round trip are recorded. The Android taps and burst pass through the device input system. The return walk sends individual Android Down presses followed by as many Up presses and checks each Channel Number on the way back against the outward path.

## Second pass on the Shield

The installed catalog contained 262,543 Channels in 154 Channel Groups. The first 40 raw Channels folded into 17 visible guide rows. The populated guide included 218 Programmes in the three-hour window sampled. All comparisons used in-place installs with the saved configuration.

### Catalog startup

The processed catalog occupied 111,514,746 bytes. An update that changed its build key recovered Channels from the saved M3U Source Snapshot first. The initial read returned no Programmes. Saved EPG recovery then published a new generation. The complete processed catalog was ready to write about 10.36 s after the cache miss. On a subsequent cold launch the first page included Programmes and its geometry and focus stayed steady throughout a 36 s observation. Publishing that first-upgrade generation can return a paginated guide to its first page.

On a normal relaunch the app verifies the cached catalog checksum, validates its structure and serves it through a read-only mapping. In the baseline trace the checksum took 346 ms and structural validation took 623 ms. Reusing a previously parsed, identical HTTP(S) authority for ordinary paths reduced structural validation to 265 ms on the same catalog. Unfamiliar inputs still go through the complete URL parser. Checksum verification, structural checks and raw Source Snapshot recovery remain active.

### Search

Three interleaved rounds used five public search terms on the fully recovered catalog. The measure includes IPC, ranking both result lists and serializing the response. It ends when the WebView receives it. All 15 ordered responses, including paging, matched byte for byte across the two implementations. The faster search checks for a required word before ranking a field. It keeps the existing ordering and checks for cancellation during large scans.

| Query | Previous median (ms) | New median (ms) |
|---|---:|---:|
| news | 407 | 53 |
| sport | 439 | 61 |
| weather report | 501 | 46 |
| zzqxx | 418 | 45 |
| abc | 377 | 53 |

### Native playback

An inspection-only trace recorded fixed phase names on the Shield’s monotonic clock. Across eight alternating tunes on two Channels, remote Center to first native video frame took 1,982 ms at the median. Center to playback request took 164 ms. Request to response headers took 1,592 ms (1,378–2,044 ms). Stream discovery and transport handoff completed within 1 ms after headers. Transport handoff to the first frame took 210 ms. These are separate medians. The HTTP interval includes connection setup and waiting for the response.

Moving session selection ahead of the React commit increased median key-to-request time to 197 ms and was discarded. Channel Up and Down retain the 350 ms interval that coalesces repeated presses. The interval now starts at the press. Starting it after rendering added 20 and 26 ms in two measured taps.

### Startup and fullscreen experiments

| Isolated change | Shield observation | Decision |
|---|---|---|
| Start a combined catalog IPC from a small entry module | Returned valid rows, but the additional module waterfall delayed paint and did not improve stable focus | Discarded |
| Start Android WebView in the background | Activity draw improved by 31 ms. Median stable focus moved from 1,929 to 1,959 ms across five cold starts per build | Discarded |
| Use two checksum workers | The 111.5 MB checksum fell from 349 to 294 ms. Hashing and structural validation together saved 16 ms in that pair | Kept the serial reader |
| Split the closed Channel Group chooser and search desk out of the entry bundle | Startup medians differed by less than 10 ms. First search-desk opening took 778 ms in the trial versus 342 ms in the preceding tour | Discarded |
| Mount the TV Channel Group buttons in batches | Stable focus fell from 1,929 to 1,880 ms. A second warmed block measured 1,860 ms. A physical Right/Left walk passed the first batch boundary and returned through every group in order | Kept |
| Keep the WebView at one pixel density and expand playback inside the already immersive TV window | The native picture filled the screen and returned to its dock on Back. Coordinates were verified against screenshots and live Media3 playback | Kept, with browser fullscreen available as fallback |

### Guide navigation workload

The walk begins with the first Channel focused in All groups. In Now, Down preserves the grid column. In Timeline, Down advances one Channel at the same viewing time. Each press records when a real Channel or Programme first takes focus and when that focused control is visible through a paint opportunity. A viewport-edge press begins on the lowest visible band. A data-page crossing is determined from the catalog’s actual continuation cursor and Channel Numbers. It can happen before or after the viewport scrolls.

Before windowing, the median time to a visible focus change in Now rose from 90 ms with 17 mounted Channels to 181 ms with 67 and 279 ms with 143. Reading the next 100 raw Channels through the native bridge took 14 ms in Now and 17 ms in Timeline. The new TV Guide keeps the scroll extent of the loaded catalog, renders the visible rows plus a small margin, and retains the actual focused and playing Channels during a scroll. Desktop and phone retain their existing Guide renderers.

The sustained test sends trusted WebView Down events every 125 ms, including repeats, ending with a WebView keyup event. Channel positions and receipt times verify the distance traveled and distinguish a queued event from a missed move. A separate multi-key Android input command exercises a burst delivered much faster than this fixed cadence. During such a burst the Guide may paint the destination once after several presses. The number of intermediate highlights observed and the final Channel reached are reported separately.
