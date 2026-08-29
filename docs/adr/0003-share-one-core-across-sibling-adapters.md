# Share one deep Rust core across sibling Axum and Tauri adapters

Sparrow TV will place catalog behavior in one `sparrow-core` Rust library used by sibling Axum and Tauri shells. The core will be a deep module with a query-oriented interface; parsing, provider formats, source locations, snapshot mechanics, and playback resolution remain private implementation rather than leaking through HTTP, IPC, or React.

## Modules and seams

- `sparrow-core` owns Source Configuration validation and fingerprinting, M3U/XMLTV parsing, Channel Identifier assignment, EPG matching, catalog queries, refresh/backoff policy, Source Snapshot coordination, immutable catalog publication, and private Playback Source resolution.
- The existing `Playlist`, `Epg`, Axum-shaped results, hard-coded filtering, deep clones, and whole-input parser implementations are not the new interface. They may be replaced outright. New parsers are verified through observable core behavior and the startup/memory gates from ADR 0002.
- The Tauri shell supplies device-local Source Configuration persistence, the atomic file snapshot adapter, lifecycle and playback-activity signals, native stream delivery, and Linux mpv integration. It contains no Axum server and calls no hosted or localhost Sparrow API.
- The Axum shell supplies deployment-owned read-only Source Configuration, its process-memory snapshot adapter, authentication, HTTP transport, and hosted playback delivery. It remains independently deployable and shares core behavior without becoming part of the installed application.
- External source access and snapshot persistence live behind core-owned interfaces with production and deterministic test adapters. Platform playback stays outside the catalog interface and receives a resolved Playback Source only through a privileged Rust seam.
- One shared React application depends on a transport-neutral `SparrowClient` interface. Tauri IPC and hosted HTTP are its two production adapters. Typed capabilities express genuine platform differences in one place instead of scattering environment checks through features.

## Channel identity and source ownership

- Every Channel has an opaque, Source Configuration-scoped Channel Identifier. It remains stable across Source Snapshot refreshes while the same Channel remains recognizable, distinguishes duplicate or backup entries, and is neither `tvg-id` nor a playback URL. No stability is promised across Source Configuration changes.
- React stores and submits Channel Identifiers for ephemeral selection and navigation. Rust-owned preference persistence keys each per-Channel Audio Track Preference by the same identifier. The exact identity/matching algorithm is private to `sparrow-core`.
- The installed settings UI may submit a new Source Configuration, but stored source locations are not returned to routine client state. The hosted web configuration remains deployment-owned and read-only. Both clients receive redacted configuration and source status.
- TypeScript begins a Playback Session with a Channel Identifier and optional Audio Track selection. Rust resolves the private Playback Source, opens one native connection, and exposes only an opaque stream handle, bytes, and typed metadata/events to the Tauri `mpegts.js` loader. Linux mpv receives the resolved source inside Rust.
- Hosted playback resolves `/play/{channelId}` inside Axum. The rewritten web client and server carry no forward-compatibility obligation for today's URL-bearing endpoints or DTOs.
- A Playback Session pins its resolved Playback Source until the session ends. Catalog refresh therefore never interrupts viewing; new selections use the new catalog, while bounded recovery inside the current session may reuse its pinned source.

## Domain-facing interface

The client-facing query interface returns transport-neutral read models rather than parser types or the whole catalog:

- capabilities, redacted Source Configuration status, and per-source refresh status;
- Channel Groups, paginated Channels, Channel details, and schedules;
- paginated Channel and Programme search results; and
- a catalog generation on catalog-derived results so clients can invalidate stale queries after publication.

Commands cover changing the editable local Source Configuration, requesting refresh, and starting or controlling playback. Installed-only commands are guarded by capabilities rather than alternate domain models. Results and failures are typed and sanitized consistently across HTTP and IPC.

Events are reserved for unsolicited state changes: catalog generation/status changes and native playback or Audio Track updates. They carry identifiers, state, and safe metadata—not bulk catalogs, Source Configuration locations, Playback Sources, or provider response bodies. React responds to a catalog-generation event by invalidating and rerunning the affected queries.

## Lifecycle and testing

The core owns freshness, validation, retry, single-flight refresh, and publication decisions. Tauri reports startup, resume, and playback-active facts; Axum reports its always-active lifecycle. The shells do not duplicate cache policy.

Tests use the core interface as their primary surface. Committed fixtures cover small valid and malformed M3U/XMLTV cases, generated fixtures cover catalog scale, and excluded private-provider fixtures supply local acceptance checks. HTTP and IPC adapters receive contract tests against the same read models, commands, events, typed errors, and capability semantics.

## Rejected alternatives

- Reusing the current Axum state and parser types as the core is rejected because it preserves transport coupling, large clones, URL-shaped results, and mixed parsing/filtering/lifecycle responsibilities.
- Running or embedding Axum inside Tauri is rejected because the installed application must be fully on-device without a local or hosted server dependency.
- Separate web and installed catalog implementations are rejected because identity, parsing, refresh, and search behavior would drift.
- Returning provider URLs to shared React code is rejected because it makes transport data into identity and defeats Rust ownership of Playback Sources.
- Sending whole catalogs through events is rejected in favor of paginated queries plus generation invalidation.
- Preserving legacy HTTP endpoints and DTOs is rejected; issue #10 must preserve a usable hosted deployment through migration checkpoints, not preserve its current internal contracts.
- Splitting parser, domain, query, and policy into public micro-crates is rejected initially. Keep private internal seams inside one deep core until a demonstrated build or ownership constraint earns another external interface.

## Revisit gates

Reconsider the seam if the two client adapters cannot satisfy the same contract without platform flags leaking into shared features; if a core interface change routinely requires unrelated Axum, Tauri, and React edits; or if a new platform playback engine cannot consume private Playback Sources through the privileged Rust seam. A failing gate requires redesigning the interface, not exposing parser, storage, or provider details to callers.
