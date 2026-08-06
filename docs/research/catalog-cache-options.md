# Persistent catalog cache options

**Date:** 2026-08-06

**Issue:** [#3 — Compare persistent catalog cache options](https://github.com/ponbac/sparrow-tv/issues/3)

**Decision state:** Research complete; final storage selection intentionally deferred.

## Executive summary

Moka is not a persistent cache. Its own documentation describes its implementations as in-memory hash-map caches with eviction and expiration policies. A Moka cache disappears with the process, so it cannot be the source of truth for Sparrow TV's last-known-good catalog. Moka could later coalesce concurrent loads or cache many derived query results, but for two whole-catalog values it largely duplicates the locking and refresh coordination Sparrow TV already has.

Three durable approaches are credible:

1. **Raw files in versioned slots, activated by an atomic pointer swap** — the smallest dependency and maintenance surface. This is the leading baseline if measurements show that parsing the cached M3U and XMLTV payloads at startup is acceptable.
2. **SQLite BLOB rows through direct Rust (`rusqlite`)** — the strongest established transaction and tooling story. It becomes preferable if the catalog must be committed as one multi-record transaction, queried without loading whole feeds, or extended with more durable state.
3. **`redb` BLOB values** — a pure-Rust, transactional alternative to SQLite. It is credible when avoiding SQLite's bundled C build matters, but adds an embedded-database file format and lifecycle for a two-value use case.

Tauri's Store plugin is suitable for small preferences, not this snapshot. Its default representation serializes an in-memory JSON map and its current save path writes directly to the destination file; that does not supply the required atomic last-known-good replacement for large M3U/XMLTV payloads.

Whichever durable backend is selected, keep the validated **raw source payloads as the canonical persisted form**. Treat any serialized parsed model as a disposable, versioned acceleration artifact. Store durable wall-clock freshness metadata alongside each payload, retain stale validated data indefinitely, and replace it only after a newly fetched payload has parsed successfully.

## Context in the current code

Sparrow TV currently keeps one parsed playlist and one parsed EPG in `Arc<RwLock<Option<_>>>`, timestamps them with process-local `Instant`s, and considers each stale after six hours ([current cache state](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/main.rs#L29-L70)). Concurrent refreshes are already serialized with one mutex per feed, and a failed refresh serves the stale in-memory value ([playlist path](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/main.rs#L201-L251), [EPG path](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/main.rs#L278-L325)).

The fetched text is validated by parsing before it enters that cache ([M3U validation](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/main.rs#L254-L275), [EPG validation](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/main.rs#L328-L331)). The playlist model is not currently serializable, while the EPG model is Serde-enabled ([playlist types](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/playlist.rs#L11-L26), [EPG types](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/epg.rs#L7-L39)). The existing route can still produce a playlist-only EPG response when the EPG is unavailable, so the two feeds already have useful independent failure behavior ([route fallback](https://github.com/ponbac/sparrow-tv/blob/0c21d11a8293ebbdd09b407d13613ae61f061302/src/routes.rs#L54-L68)).

The repository does not contain representative provider payloads, so neither real disk size nor target-device parse time can be inferred here. Those measurements are a decision gate, not a fact to assume.

## Moka's actual persistence model

[Moka's crate documentation](https://docs.rs/moka/latest/moka/index.html) explicitly calls its implementations in-memory concurrent caches backed by hash maps. Entries are subject to capacity eviction and optional time-to-live/time-to-idle expiration. The published API exposes cache policies and map operations, not a disk store, serializer, restart recovery, or durable commit protocol ([complete public item list](https://docs.rs/moka/latest/moka/all.html)).

Relevant capabilities are:

- `future::Cache::try_get_with` coalesces concurrent initialization for the same absent key, which can replace some explicit single-flight locking ([Moka future cache](https://docs.rs/moka/latest/moka/future/struct.Cache.html#method.try_get_with)).
- Reads clone the stored value, so large parsed catalogs should be held behind `Arc` if Moka is used ([Moka value access](https://docs.rs/moka/latest/moka/future/struct.Cache.html#avoiding-to-clone-the-value-at-get)).
- TTL is measured from in-process insertion, not from a durable upstream fetch timestamp ([Moka TTL](https://docs.rs/moka/latest/moka/future/struct.CacheBuilder.html#method.time_to_live)). It therefore cannot implement six-hour freshness across restart by itself.

For Sparrow TV's two long-lived catalog objects, popularity-based admission/eviction is not useful and automatic expiry is actively at odds with “serve stale indefinitely.” The current per-feed locks already prevent duplicate refresh work. The initial architecture should therefore use an ordinary `Arc`-held parsed snapshot after loading the durable store. Add Moka only if later profiling identifies a distinct many-key cache—such as derived searches, artwork metadata, or other expensive results—where its eviction or single-flight behavior removes measured cost.

## Durable shortlist

### 1. Raw files with versioned slots and an atomic pointer

Persist each validated feed as source bytes plus metadata in the Tauri app data directory. Tauri exposes `app_data_dir` on both [desktop](https://docs.rs/tauri/latest/tauri/path/struct.PathResolver.html#method.app_data_dir) and [Android](https://docs.rs/tauri/latest/x86_64-linux-android/tauri/path/struct.PathResolver.html#method.app_data_dir). Use app data rather than app cache: Android documents separate persistent and cache locations and warns that cache files may be removed when storage is low ([Android app-specific storage](https://developer.android.com/training/data-storage/app-specific)).

A crash-safe layout can use two slots per atomicity unit:

```text
catalog/
  current                 # "a" or "b"
  a/manifest.json
  a/playlist.m3u
  a/epg.xml
  b/manifest.json
  b/playlist.m3u
  b/epg.xml
```

Write and validate the inactive slot completely, synchronize its files, then atomically replace the tiny `current` pointer with a temporary file created in the same directory. Rust's `rename` replaces an existing destination on the target Unix platforms and cannot cross mount points ([`std::fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html)); `tempfile::NamedTempFile::persist` documents atomic replacement but also notes that it does not synchronize content or the containing directory ([`NamedTempFile::persist`](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist)). Call `File::sync_all` before activation; it asks the OS to flush file content and metadata ([`File::sync_all`](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all)). For power-loss durability rather than process-death safety alone, also synchronize the containing directory after the pointer rename on supported platforms.

The two slots make recovery deterministic: if the pointer or active slot is corrupt, validate both slots and select the newest valid manifest. An interrupted write never touches the active slot. Tests can inject failure after every write/sync/rename boundary and assert that startup returns either the old validated snapshot or the complete new one, never a mixture.

**Strengths**

- No database engine, query language, migration framework, or native library.
- Stores provider bytes compactly and allows the current parser to be the integrity check.
- Schema changes affect only the small manifest; raw feeds can be reparsed by a new app version.
- Easy to inspect, replace, and delete during support or tests.

**Costs and cautions**

- Correct synchronization and slot activation are application code that must be tested.
- Parsed models must be rebuilt after process start unless a derived artifact is also stored.
- If playlist and EPG refresh independently, either use a slot pair per feed or define how a partially newer pair becomes a catalog. A single catalog-wide pointer provides coherent pairs but sacrifices the existing independent EPG failure behavior.
- Large source text is still read and parsed as a whole, matching the current parser design.

### 2. SQLite with raw payloads in BLOB rows

Use one private SQLite file with a small table such as `snapshot(kind PRIMARY KEY, schema_version, source_id, fetched_at_ms, payload BLOB, checksum)`. Validate outside the database, then update the desired rows and metadata in one transaction. SQLite documents that committed transactions are atomic and durable across program, OS, and power failures ([SQLite transactional guarantees](https://www.sqlite.org/transactional.html)); its atomic-commit documentation explains the rollback-journal mechanism ([atomic commit](https://www.sqlite.org/atomiccommit.html)).

For a Rust-owned catalog module, direct `rusqlite` is narrower than the Tauri SQL plugin. `rusqlite`'s `bundled` feature compiles and links an embedded SQLite using the `cc` crate, avoiding dependence on a device/system SQLite version ([rusqlite build guidance](https://github.com/rusqlite/rusqlite#usage)). Its transaction wrapper rolls back by default unless explicitly committed ([`rusqlite::Transaction`](https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html)). The official Tauri SQL plugin does support Linux and Android, SQLite, and transactional migrations, but its primary purpose is exposing SQL to the frontend through `sqlx` ([Tauri SQL plugin](https://v2.tauri.app/plugin/sql/)); that IPC and permission surface is unnecessary when only shared Rust code owns the data.

**Strengths**

- A single transaction can replace payloads and metadata together.
- Mature crash recovery, integrity tooling, migrations, and inspection tools.
- Natural path if requirements later include indexed programme queries, multiple source records, or more durable application state.
- Corruption handling can be “quarantine/delete and refetch,” because the cache is derived from the configured source.

**Costs and cautions**

- Adds SQLite, a Rust wrapper, schema/migration code, and Android cross-build coverage. Bundling avoids runtime-library variance but adds a C compilation step.
- Storing only two opaque BLOBs gains little query or startup advantage over files; the XML and M3U still need parsing.
- Normal journaling/WAL may temporarily require additional disk space during a large replacement.
- Turning programmes into relational rows is a separate, much larger design: it couples the durable schema to parser/domain changes and should be justified by measured query/startup needs.

### 3. `redb` with raw payloads in a transaction

`redb` is a pure-Rust embedded key-value database using copy-on-write B-trees. Its current documentation promises ACID transactions, concurrent readers, and crash safety ([`redb` overview](https://docs.rs/redb/latest/redb/)). A write transaction can replace the M3U bytes, EPG bytes, and manifest values as one commit. Its database API can check and sometimes repair integrity, while normal opens automatically recover from unclean shutdowns ([`Database::check_integrity`](https://docs.rs/redb/latest/redb/struct.Database.html#method.check_integrity)).

**Strengths**

- Transactional single-file persistence without SQLite's C build.
- Key/value shape fits a handful of raw blobs and metadata better than a relational schema.
- Built-in crash recovery and explicit integrity checking reduce custom atomic-file mechanics.

**Costs and cautions**

- Still introduces a database dependency, opaque file format, compaction/repair behavior, and upgrade handling. The API exposes `UpgradeRequired` for old on-disk formats, so app updates need an explicit “upgrade or discard and rebuild” policy ([`DatabaseError`](https://docs.rs/redb/latest/redb/enum.DatabaseError.html)).
- Fewer ubiquitous inspection and recovery tools than SQLite.
- For two values read as whole blobs, its transaction machinery may not earn its maintenance cost over two-slot files.

## Options not shortlisted

### Tauri Store plugin

The Store plugin is persistent, usable from Rust, and officially supports desktop and Android ([Tauri Store](https://v2.tauri.app/plugin/store/)). However, its default serializer converts the entire in-memory map of JSON values into a pretty-printed JSON byte vector ([serializer source](https://docs.rs/tauri-plugin-store/latest/src/tauri_plugin_store/lib.rs.html#327-336)). More importantly for this requirement, its current `save` implementation serializes the map and calls `fs::write` directly on the destination ([save source](https://docs.rs/tauri-plugin-store/latest/src/tauri_plugin_store/store.rs.html#291-299)); it does not stage, synchronize, and atomically activate a replacement.

It remains a reasonable home for small settings such as the single source configuration. Using it for large catalog payloads would require a custom serializer and atomic save protocol, at which point direct files are simpler and clearer.

### Platform-specific Android database or frontend storage

Room, WebView storage, IndexedDB, and browser local storage would split persistence away from the shared Rust catalog logic and require a second desktop path. They do not improve the target architecture's portability, so they are not credible defaults for this decision.

## Raw versus parsed persistence

Use raw validated M3U/XMLTV as the durable authority:

- It preserves exactly what passed the parser and avoids inventing a durable schema for `Playlist` and `Epg`.
- Filters can be reapplied from current settings rather than freezing their results into a snapshot.
- A parser or domain-model update can rebuild from source bytes without network access.
- Corruption detection is simple: verify manifest length/checksum, then parse. If parsing fails, try the alternate slot or rebuild from the source.

A parsed artifact is justified only if target-device measurements show unacceptable startup parsing time or peak memory. If added, it should include `artifact_schema_version`, parser/application version, and the hash of the raw payload. Any mismatch or decode failure discards only the parsed artifact; it must not invalidate the raw last-known-good snapshot. Parsed serialization should be benchmarked against the raw input because JSON may increase both disk size and allocation pressure, especially for XMLTV programme arrays.

This also avoids coupling the storage choice to the query model. SQLite or redb can initially hold raw BLOBs and gain a versioned parsed/indexed representation later without changing the last-known-good contract.

## Freshness and lifecycle contract

The storage adapter should persist, for each atomic snapshot unit:

```text
format_version
source_id                 # non-secret fingerprint/config generation
fetched_at_unix_ms        # time the payload was fetched and validated
payload length/checksum
raw payload(s)
optional parsed artifact metadata
```

Use a wall-clock UTC timestamp for durable freshness, because the current `Instant` is meaningful only inside one running process. Freshness is `now - fetched_at < 6 hours`; a timestamp implausibly in the future should be treated as stale so a backward clock change cannot suppress refresh indefinitely. A refresh attempt timestamp is operational state and must never advance `fetched_at` or replace the last-known-good payload.

Expected behavior:

1. **Launch:** Load and validate the persisted snapshot before requiring network access. Serve it even when stale. If absent, corrupt, wrong-version, or from an incompatible source configuration, fetch.
2. **Resume:** Re-evaluate the persisted wall-clock age. If stale, keep serving the loaded snapshot while one refresh is attempted.
3. **Manual refresh:** Bypass the six-hour freshness gate (and any ordinary retry delay), but keep the old snapshot until fetch and parse both succeed.
4. **Successful refresh:** Stage raw bytes and metadata, validate, durably commit/activate, then publish the parsed value to in-process readers.
5. **Failed refresh:** Report/log the failure and keep the previous snapshot indefinitely.
6. **Corruption or incompatible schema:** Try the alternate file slot or backend repair where applicable; otherwise quarantine/delete the derived cache and rebuild. Never crash-loop on corrupt cache data.

Whether the atomic unit is **each feed** or a **playlist+EPG pair** remains an architecture decision. Per-feed commits preserve the current ability to update the playlist while retaining an older EPG, and are the simpler fit with independent freshness/failure. A pair transaction is preferable only if mixed generations can produce user-visible catalog inconsistencies that validation cannot tolerate.

## Decision criteria and proposed validation

Before selecting the backend, collect one representative real-world snapshot on both target classes and measure:

| Criterion | Two-slot raw files | SQLite BLOBs | `redb` BLOBs |
| --- | --- | --- | --- |
| Dependencies/toolchain | Lowest; optional `tempfile` | `rusqlite` plus bundled C SQLite | Pure-Rust database crate |
| Atomic multi-value commit | Pointer protocol in app code | Native transaction | Native transaction |
| Crash/power-loss mechanics | Must implement and test sync/rename sequence | Established SQLite protocol | Built-in crash-safe transaction |
| Startup without parsed artifact | Read and parse whole feeds | Read BLOBs and parse whole feeds | Read values and parse whole feeds |
| Schema evolution for raw payloads | Versioned manifest; discard/rebuild | Migration plus discard/rebuild policy | Table/value version plus upgrade/rebuild policy |
| Inspection/support | Plain files | Excellent tools | Library-specific tools |
| Natural expansion path | More files/protocol code | Structured/indexed queries | More key/value state |
| Fit for only two raw feeds | Best | Credible but heavier | Credible but heavier |

Measure at least:

- M3U and XMLTV compressed/on-wire and decoded sizes;
- parse latency and peak memory on the Android device and Arch desktop;
- time to load a candidate parsed artifact versus reparsing raw input;
- commit latency and temporary disk overhead for a full replacement;
- recovery under injected process termination at every commit boundary;
- behavior after byte corruption, manifest/schema mismatch, source change, clock rollback, and disk-full errors;
- Android build and test coverage for any database dependency.

The decision rule can then stay simple:

- Select **two-slot raw files** if raw parsing meets the agreed startup budget and a small, well-tested activation protocol is acceptable.
- Select **SQLite** if native transactions, inspection tooling, or likely structured/indexed growth outweigh its build and schema cost.
- Select **`redb`** if a transactional key/value file is warranted and eliminating the SQLite C toolchain is a material advantage.
- Add a **parsed sidecar** only to solve measured startup cost.
- Add **Moka** only for a separately measured many-key or concurrent-initialization problem; never for durability.

## Result for the architecture map

Carry three candidates into the storage decision, with two-slot raw files as the complexity baseline. The architecture can already settle that durable raw payloads plus wall-clock metadata are the authority, stale validated data survives indefinitely, refresh never overwrites good data before validation, parsed artifacts are disposable, and Moka is not part of persistence.
