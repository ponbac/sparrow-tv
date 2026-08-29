# Sparrow Next architecture handoff

## Summary

Sparrow Next is a complete replacement built on `rewrite/sparrow-next`. One deep Rust `sparrow-core` module owns source parsing, Channel identity, the in-memory Channel Catalog, Source Snapshot policy, queries, refresh, and private Playback Source resolution. Sibling Axum and Tauri adapters use that core; one React application uses a transport-neutral `SparrowClient` with hosted-HTTP and Tauri-IPC adapters.

The installed application is fully on-device. It never calls the hosted Sparrow deployment or a localhost server. Linux and Android use `mpegts.js` over a Tauri-native byte stream as the Primary Playback Engine; Linux alone offers system mpv as an explicit Fallback Playback Engine. Raw Source Snapshots persist atomically on installed devices, while the hosted process uses memory-only snapshots. The old implementation is replaced rather than adapted.

This handoff reconciles [ADR 0001](../adr/0001-shared-native-http-playback.md), [ADR 0002](../adr/0002-persist-independent-raw-source-snapshots.md), [ADR 0003](../adr/0003-share-one-core-across-sibling-adapters.md), [ADR 0004](../adr/0004-rewrite-on-a-replacement-branch.md), and [ADR 0005](../adr/0005-build-candidates-on-tags-and-publish-after-device-acceptance.md). It is decision-complete and ready to split into implementation issues.

## Context and current state

The current Rust binary mixes environment loading, provider access, parsing, filtering, refresh, Axum routing, arbitrary-URL stream proxying, and process state. Its transport DTOs expose provider URLs, its parsers and catalog are cloned through request paths, and React selects a URL directly. The React player owns mutable timers and recovery without a typed state machine. The Docker build and `deploy.sh` use moving inputs and mutate a manifest while releasing.

Those files are evidence of the workflows to preserve—browse/search Channels and Programmes, inspect schedules, play and switch Channels—not interfaces to retain. No old endpoint, DTO, filter quirk, parser behavior, or URL-bearing client state survives by default.

## Goals

- Build one on-device Channel Catalog from one required M3U Source and one optional EPG Source.
- Parse once at startup and after successful refresh, then serve queries from one immutable in-memory catalog.
- Share catalog behavior and read models between hosted Axum and installed Tauri applications.
- Keep Source Configuration locations and resolved Playback Sources inside privileged Rust code.
- Preserve hosted Channel/Programme search, guide, browsing, and playback while rewriting client and server together.
- Provide reliable Linux and Android playback, selectable Audio Tracks, a per-Channel Audio Track Preference, bounded recovery, lifecycle ownership, and manual Linux mpv failover.
- Retain last-known-good installed Source Snapshots indefinitely and expose truthful redacted status.
- Produce and test exact AppImage and signed APK release candidates before publishing them.

## Non-goals

- Legacy HTTP or IPC compatibility, differential parity with the old code, or a runnable state after every commit.
- A hosted or localhost dependency for Tauri.
- VOD, catch-up, recording, casting, favourites, multiple Source Configurations, multiple users, or cloud sync.
- Android Media3 initially, bundled mpv, app stores, AABs, automatic updates, or ABI-split APKs.
- Windows, macOS, general Linux support, or Android support beyond the physical Realme target.
- A 30-switch provider stress run or a 90-minute soak as a release requirement.

## Invariants

1. React may hold Channel Identifiers and may transiently hold user-entered Source Configuration locations only inside the installed settings form. Routine client state never contains stored source locations, provider Playback Sources/headers, source fingerprints, or raw source payloads.
2. A Playback Session owns at most one provider connection. Stop, pause, restart, Audio Track change, Channel change, lifecycle suspension, and mpv failover release the old connection before another starts.
3. A Playback Session pins its resolved Playback Source until it ends. Catalog publication never changes an active session.
4. A Channel Identifier is opaque, scoped to one Source Configuration, stable across recognizable refreshes, and distinct for duplicate or backup entries.
5. One immutable `Arc<ChannelCatalog>` is published at a time. Queries clone its pointer; they neither parse source payloads nor deep-clone the catalog.
6. M3U and EPG refresh, validation, storage, status, and failure are independent. EPG failure cannot reject a valid M3U publication.
7. A Stale Source Snapshot stays eligible indefinitely. Refresh failure never destroys or advances the last-known-good snapshot.
8. A Source Configuration change makes snapshots from a different source fingerprint immediately ineligible. Stored locations are never returned by routine status queries.
9. Automatic refresh is single-flight, respects freshness/backoff, and waits for Playback Session inactivity. Manual refresh coalesces but bypasses freshness and retry delay.
10. Audio Track change recreates primary playback and visibly falls back when the saved preference cannot match a current rendition.
11. mpv starts only after explicit user action and complete primary-engine shutdown. It never runs on Android.
12. Logs, errors, metrics, copied diagnostics, HTTP DTOs, and IPC DTOs use safe projections. They cannot contain Source Configuration locations, Playback Sources, provider response bodies, identifying fingerprints, or signing material.

## Design constraints

- Installed source payload limits are 128 MiB decoded M3U and 64 MiB decoded EPG.
- Source freshness lasts six hours after complete validation or `304 Not Modified`.
- Automatic retry delays are 1, 5, 15, then 60 minutes, capped at one attempt per hour and extended by a longer `Retry-After`.
- Physical Android snapshot cold-start must be at most three seconds with at most 512 MiB peak process memory.
- Linux applies `WEBKIT_DISABLE_DMABUF_RENDERER=1` before WebKit starts on the target host.
- The target Linux system supplies mpv; the AppImage does not bundle it.
- Hosted production remains the old `master` deployment until the complete replacement passes web, Linux, and Android gates.

## Alternatives considered

### Shared deep core with sibling adapters — selected

Axum and Tauri call the same concrete `SparrowCore`; platform I/O enters through real seams. The shared React application calls one `SparrowClient` interface. This concentrates identity, parsing, refresh, query, and redaction invariants while letting platform playback and persistence differ.

### Embedded Axum inside Tauri — rejected

This would reuse HTTP shapes but make the installed application operate a local server, preserve transport coupling, and create a second runtime and security surface merely to cross one process. It conflicts with the fully on-device, no-localhost requirement.

### Separate hosted and installed implementations — rejected

This makes each shell locally simple but duplicates the hardest behavior: parsing, matching, Channel identity, refresh, search, and error projection. Behavior and tests would drift, and provider knowledge would spread through both applications.

### One generic request dispatcher — rejected for the client interface

A single `execute(Request) -> Response` method minimizes method count but moves exhaustive narrowing and request/response pairing into every caller. Task-oriented typed methods give React greater leverage while the adapters still centralize transport translation.

## Recommendation

Use the shared-core/sibling-adapter topology and task-oriented client interface below. The core is a concrete deep module, not a public trait. Its external dependencies have core-owned ports only where two real adapters exist: provider access has production and deterministic adapters; snapshots have atomic-file, process-memory, and deterministic adapters; time has system and controlled adapters.

Keep parser, matching, indexing, identity, refresh, and publication seams private inside `sparrow-core`. Keep platform playback out of the catalog interface. Keep HTTP and IPC DTO parsing in their adapters. The same read-model contract is verified through both adapters without forcing their wire protocols to be identical.

## Domain model and core types

The Rust sketches define core ownership; they are intentionally independent of Axum, Tauri, reqwest, and frontend libraries.

```rust
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ChannelId(Arc<str>); // opaque; redacted Debug; serialized only as an ID

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub struct CatalogGeneration(u64);

pub struct SourceConfigurationInput {
    pub m3u: String,
    pub epg: Option<String>,
}

pub struct SourceConfiguration {
    m3u: SecretSourceLocation,
    epg: Option<SecretSourceLocation>,
    fingerprint: SourceFingerprint,
}

pub enum SourceKind { M3u, Epg }

pub enum SourceState {
    Fresh { validated_at: DateTime<Utc> },
    Stale { validated_at: DateTime<Utc>, next_attempt_at: Option<DateTime<Utc>> },
    Refreshing { retained_snapshot: bool },
    Failed { retained_snapshot: bool, retry_at: Option<DateTime<Utc>>, failure: SafeFailure },
    Unavailable { failure: Option<SafeFailure> },
}

pub struct CatalogStatus {
    pub generation: Option<CatalogGeneration>,
    pub m3u: SourceState,
    pub epg: Option<SourceState>,
    pub configuration: RedactedSourceConfiguration,
}

pub struct ChannelGroupView { pub name: String, pub channel_count: u32 }

pub struct ChannelSummary {
    pub id: ChannelId,
    pub name: String,
    pub group: String,
    pub now: Option<ProgrammeSummary>,
}

pub struct ChannelDetails {
    pub id: ChannelId,
    pub name: String,
    pub group: String,
}

pub struct ProgrammeSummary {
    pub channel_id: ChannelId,
    pub title: String,
    pub description: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

pub struct PageRequest { pub cursor: Option<PageCursor>, pub limit: PageLimit }
pub struct Page<T> {
    pub generation: CatalogGeneration,
    pub items: Vec<T>,
    pub next: Option<PageCursor>,
}

pub struct SearchRequest {
    pub term: SearchTerm,
    pub channels: PageRequest,
    pub programmes: PageRequest,
}

pub struct SearchResults {
    pub generation: CatalogGeneration,
    pub channels: Page<ChannelSummary>,
    pub programmes: Page<ProgrammeSummary>,
}

pub struct ResolvedPlaybackSource {
    pub(crate) location: SecretPlaybackLocation,
    pub(crate) headers: SecretHeaders,
}

// Deliberately not Serialize; Debug is always redacted.
```

Page cursors are opaque, scoped to a catalog generation and query shape, and rejected after incompatible publication. Page limits are parsed and bounded at the core interface. Programme timestamps become UTC internally and are formatted in the UI locale.

Channel identity is deterministic and private. The source fingerprint namespaces every identifier. Within that namespace, the identity seed contains normalized `tvg-id` when present, normalized Channel name and Channel Group, and a zero-based occurrence among entries with the same seed in source order. A versioned cryptographic digest of namespace, seed, and occurrence becomes the opaque Channel Identifier. Playback location and credentials never become client identity. This keeps recognizable entries stable, distinguishes duplicate/backup entries, and makes truly indistinguishable reordered duplicates the only accepted ambiguous case.

EPG matching first uses exact trimmed M3U `tvg-id` to XMLTV channel ID. When that is absent, a normalized display-name fallback is allowed only when it is unique on both sides; ambiguous or unmatched Programmes remain unassociated rather than being guessed. Fixtures own the provider-specific normalization rules.

Expected core failures are values:

```rust
pub enum CoreError {
    InvalidInput { field: &'static str, reason: SafeReason },
    NotConfigured,
    CatalogUnavailable { status: CatalogStatus },
    ChannelNotFound { id: ChannelId },
    StaleCursor { current: CatalogGeneration },
    RefreshRejected { kind: SourceKind, failure: SafeFailure },
    SnapshotFailure { kind: SourceKind, operation: SnapshotOperation, failure: SafeFailure },
    Cancelled,
}
```

Defects such as violated internal invariants fail fast; provider, parse, storage, absence, cancellation, and stale-generation outcomes are typed and safely projected.

## Core interface and ports

```rust
pub struct SparrowCore { /* private runtime and Arc<ChannelCatalog> */ }

impl SparrowCore {
    pub fn parse_source_configuration(
        input: SourceConfigurationInput,
    ) -> Result<SourceConfiguration, CoreError>;

    pub async fn bootstrap(
        configuration: Option<SourceConfiguration>,
        adapters: CoreAdapters,
    ) -> Result<Self, CoreError>;

    pub fn status(&self) -> CatalogStatus;
    pub fn list_groups(&self, page: PageRequest) -> Result<Page<ChannelGroupView>, CoreError>;
    pub fn list_channels(&self, query: ChannelQuery) -> Result<Page<ChannelSummary>, CoreError>;
    pub fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError>;
    pub fn schedule(&self, query: ScheduleQuery) -> Result<Page<ProgrammeSummary>, CoreError>;
    pub fn search(&self, query: SearchRequest) -> Result<SearchResults, CoreError>;

    pub async fn replace_source_configuration(
        &self,
        configuration: SourceConfiguration,
    ) -> Result<CatalogStatus, CoreError>;

    pub async fn refresh(&self, trigger: RefreshTrigger) -> RefreshReport;
    pub fn report_lifecycle(&self, signal: LifecycleSignal);
    pub fn begin_playback_activity(&self) -> PlaybackActivityLease;
    pub fn resolve_playback(&self, id: &ChannelId) -> Result<ResolvedPlaybackSource, CoreError>;
    pub fn subscribe(&self) -> CoreEventStream;
}

pub enum RefreshTrigger { Startup, Resume, FreshnessDeadline, Manual }
pub enum LifecycleSignal { Started, Resumed, Suspended }

pub enum CoreEvent {
    CatalogStatusChanged { status: CatalogStatus },
    CatalogPublished { generation: CatalogGeneration },
}
```

`PlaybackActivityLease` is non-cloneable and reports inactivity when dropped, including cancellation and failure exits. This gives both shells a scoped resource instead of a race-prone active/inactive flag. The shell may call `resolve_playback`, but the resulting type cannot cross HTTP, IPC, or serialization. Source access and Source Snapshot persistence use real ports:

```rust
pub trait SourceAccess: Send + Sync {
    async fn open(&self, request: SourceRequest) -> Result<SourceResponse, SourceAccessError>;
}

pub struct SourceResponse {
    pub status: SourceResponseStatus,
    pub validators: PrivateValidators,
    pub decoded_body: SourceByteStream,
}

pub trait SnapshotStore: Send + Sync {
    async fn candidates(&self, source: SourceKey) -> Result<Vec<SnapshotCandidate>, StoreError>;
    async fn begin_stage(&self, metadata: PendingManifest) -> Result<SnapshotStage, StoreError>;
    async fn activate(&self, validated: ValidatedStage) -> Result<(), StoreError>;
    async fn discard(&self, stage: SnapshotStage) -> Result<(), StoreError>;
}

pub trait Clock: Send + Sync { fn now(&self) -> DateTime<Utc>; }
```

The core streams provider bytes into a stage while enforcing decoded limits and calculating length/checksum, reopens the staged payload for full parsing, then activates it only after validation. The atomic-file adapter synchronizes payload, manifest, pointer, and directory. The memory adapter implements the same observable contract without durable I/O. Temp-directory tests exercise the production file adapter rather than mocking filesystem semantics.

## Client types and interface

Every HTTP or IPC result begins as `unknown` and is parsed into the types below. Branding is created only by parsers, never by casts in feature code.

```ts
declare const brand: unique symbol;
type Brand<T, Name extends string> = T & { readonly [brand]: Name };

export type ChannelId = Brand<string, "ChannelId">;
export type CatalogGeneration = Brand<number, "CatalogGeneration">;
export type PageCursor = Brand<string, "PageCursor">;
export type PlaybackSessionId = Brand<string, "PlaybackSessionId">;
export type AudioTrackId = Brand<string, "AudioTrackId">;
export type IsoInstant = Brand<string, "IsoInstant">;
export type NativeStreamHandle = Brand<string, "NativeStreamHandle">;
export type SameOriginPlaybackEndpoint = Brand<string, "SameOriginPlaybackEndpoint">;

export type Capabilities = Readonly<{
  sourceConfiguration: "editable" | "deployment-readonly";
  playbackTransport: "tauri-native-stream" | "same-origin-http";
  audioTrackSelection: boolean;
  mpvFailover: boolean;
}>;

export type PlaybackDescriptor =
  | Readonly<{
      _tag: "tauri-native-stream";
      sessionId: PlaybackSessionId;
      streamHandle: NativeStreamHandle;
    }>
  | Readonly<{
      _tag: "same-origin-http";
      sessionId: PlaybackSessionId;
      endpoint: SameOriginPlaybackEndpoint; // Sparrow route only; never a Playback Source
    }>;

export type AudioTrack = Readonly<{
  id: AudioTrackId;
  language?: string;
  label?: string;
  codec?: string;
  selected: boolean;
}>;

export type PlaybackState =
  | Readonly<{ _tag: "idle" }>
  | Readonly<{ _tag: "starting"; sessionId: PlaybackSessionId; channelId: ChannelId }>
  | Readonly<{ _tag: "playing"; sessionId: PlaybackSessionId; channelId: ChannelId; tracks: readonly AudioTrack[] }>
  | Readonly<{ _tag: "paused"; sessionId: PlaybackSessionId; channelId: ChannelId }>
  | Readonly<{ _tag: "recovering"; sessionId: PlaybackSessionId; attempt: number; reason: SafePlaybackFailure }>
  | Readonly<{ _tag: "fallback-playing"; sessionId: PlaybackSessionId; channelId: ChannelId }>
  | Readonly<{ _tag: "failed"; sessionId: PlaybackSessionId; failure: SafePlaybackFailure; canFailover: boolean }>
  | Readonly<{ _tag: "stopped"; sessionId: PlaybackSessionId }>;

export type ClientError =
  | Readonly<{ _tag: "invalid-input"; field: string; reason: string }>
  | Readonly<{ _tag: "not-configured" }>
  | Readonly<{ _tag: "catalog-unavailable"; status: CatalogStatus }>
  | Readonly<{ _tag: "not-found"; resource: "channel" }>
  | Readonly<{ _tag: "stale-cursor"; current: CatalogGeneration }>
  | Readonly<{ _tag: "unsupported"; capability: keyof Capabilities }>
  | Readonly<{ _tag: "transport"; retryable: boolean; message: string }>
  | Readonly<{ _tag: "cancelled" }>;

export type ClientResult<T> =
  | Readonly<{ _tag: "success"; value: T }>
  | Readonly<{ _tag: "failure"; error: ClientError }>;

export interface SparrowClient {
  capabilities(signal?: AbortSignal): Promise<ClientResult<Capabilities>>;
  status(signal?: AbortSignal): Promise<ClientResult<CatalogStatus>>;
  listGroups(input: PageInput, signal?: AbortSignal): Promise<ClientResult<Page<ChannelGroup>>>;
  listChannels(input: ChannelQuery, signal?: AbortSignal): Promise<ClientResult<Page<ChannelSummary>>>;
  channel(id: ChannelId, signal?: AbortSignal): Promise<ClientResult<ChannelDetails>>;
  schedule(input: ScheduleQuery, signal?: AbortSignal): Promise<ClientResult<Page<ProgrammeSummary>>>;
  search(input: SearchInput, signal?: AbortSignal): Promise<ClientResult<SearchResults>>;
  replaceSourceConfiguration(input: SourceConfigurationInput, signal?: AbortSignal): Promise<ClientResult<CatalogStatus>>;
  refresh(signal?: AbortSignal): Promise<ClientResult<RefreshReport>>;
  startPlayback(input: StartPlayback, signal?: AbortSignal): Promise<ClientResult<PlaybackDescriptor>>;
  controlPlayback(command: NativePlaybackCommand, signal?: AbortSignal): Promise<ClientResult<void>>;
  subscribe(listener: (event: SparrowEvent) => void): () => void;
}
```

`replaceSourceConfiguration` is supported only when capability says `editable`; hosted HTTP returns `unsupported` and provides no mutation route. Volume, mute, fullscreen, stall detection, recovery budget, and the exhaustive Playback Session reducer remain shared TypeScript behavior. Native commands cover effects requiring Rust: stop/release, restart, Audio Track selection, lifecycle suspension/resume, and Linux mpv failover.

Every exported client contract and method carries JSDoc for its invariants, cancellation, side effects, and expected failure values. Feature code exhaustively handles `ClientResult` and closed state/event variants; adapters are the only code allowed to translate a boundary-required exception into `ClientError`.

Pause releases the live provider request and retains session intent; resume restarts at the live edge using the pinned Playback Source. This avoids an unbounded paused buffer and hidden network use. Android keeps the screen awake only while primary playback is actively playing or recovering; background, manual lock, stop, and terminal failure release the request and the wake lock. Foreground resumes only a session that was playing before suspension.

## Seams, adapters, and implementations

| Seam | Interface owner | Production adapters | Test adapter/evidence |
|---|---|---|---|
| Catalog behavior | `sparrow-core::SparrowCore` concrete interface | Called by Axum and Tauri shells | Real core with controlled ports |
| Provider access | `sparrow-core::SourceAccess` | Shared reqwest implementation | Deterministic chunk/status/error adapter |
| Source Snapshots | `sparrow-core::SnapshotStore` | Tauri atomic files; Axum process memory | Temp-directory atomic store and deterministic memory store |
| Time | `sparrow-core::Clock` | System UTC clock | Controlled clock |
| Hosted client | `app` `SparrowClient` | HTTP adapter | Contract adapter against Axum router |
| Installed client | `app` `SparrowClient` | Tauri IPC/event adapter | Contract adapter against Tauri command harness |
| Installed native bytes | Tauri playback manager | Tauri native HTTP + MPEG-TS selector | Deterministic chunk/PMT fixture adapter |
| Linux failover | Tauri playback manager | mpv 0.41+ JSON IPC process | Fake child/IPC adapter plus target-host acceptance |

Axum serves the SPA and `/api/v1` from one origin; CORS is not enabled. A single deployment-configured HTTP Basic credential is checked by Axum middleware for the SPA and API, while `/health` exposes only liveness. `/api/v1/play/{channelId}` resolves an identifier inside Rust and cannot proxy an arbitrary URL. Hosted Source Configuration is read-only and deployment-owned.

Tauri exposes allowlisted commands and typed events only. Its capability set contains no general HTTP scope callable with an arbitrary URL from React. The native stream manager maps an opaque handle to one Rust-owned request and accepts bounded read/cancel operations from the custom `mpegts.js` loader.

## Call stacks and data flow

### Old hosted playback flow — deleted

```text
React search string
  -> GET /search
  -> Axum-shaped parser/catalog work
  -> DTO containing provider URL
  -> React selectedUrl
  -> GET /proxy/{arbitrary provider URL}
  -> provider bytes
  -> mpegts.js
```

### Installed startup

```text
Tauri setup
  -> ConfigStore.load() -> unknown local file
  -> parse optional SourceConfigurationInput
  -> SparrowCore::parse_source_configuration(input)
  -> SparrowCore.bootstrap(configuration, file/source/clock adapters)
  -> SnapshotStore.candidates(kind, source fingerprint)
  -> manifest/size/checksum parse
  -> M3U and optional EPG parse + Channel identity/matching
  -> publish Arc<ChannelCatalog>(generation)
  -> report stale/fresh status immediately
  -> schedule eligible background refresh
  -> Tauri IPC adapter projects safe DTO
  -> parse unknown in TypeScript
  -> React Query cache
```

With no Source Configuration, bootstrap returns a usable core in `NotConfigured` state so the settings UI can supply one. If a configuration exists but no valid M3U snapshot does, bootstrap performs a foreground M3U fetch. If it fails, the core remains `CatalogUnavailable` and the settings/status UI remains usable. Missing or failed EPG produces a Channel-only catalog.

### Catalog query and search

```text
React feature
  -> SparrowClient.listChannels/search(ChannelId-free query + cursor)
  -> HTTP or IPC adapter boundary parser
  -> SparrowCore query
  -> clone Arc<ChannelCatalog>
  -> index lookup/page projection with CatalogGeneration
  -> adapter safe DTO
  -> TypeScript runtime parser
  -> React Query result
```

No query fetches or reparses M3U/XMLTV. A publication event invalidates generation-keyed React queries. A stale cursor returns the current generation so the caller restarts pagination.

### Refresh and atomic publication

```text
manual command OR startup/resume/deadline signal
  -> SparrowCore refresh single-flight/coalescing
  -> automatic trigger checks freshness, backoff, and playback activity
  -> SourceAccess.open(private source + validators)
  -> stream decoded chunks into SnapshotStore stage
  -> enforce byte limit + checksum
  -> parse/validate staged payload
  -> fsync stage; atomically activate (installed) OR swap memory slot (hosted)
  -> rebuild candidate catalog from eligible M3U + EPG snapshots
  -> release raw parse buffers
  -> publish one new Arc and generation
  -> emit safe status/publication events
```

`304 Not Modified` advances validation time without replacing the raw payload. M3U and EPG each complete or fail independently. Cancellation or any validation/storage failure discards only the inactive stage and retains the current catalog.

### Source Configuration change

```text
settings form raw strings
  -> Tauri IPC parser
  -> SparrowCore::parse_source_configuration validates/refines input
  -> ConfigStore atomically persists the private plain-text configuration record
  -> SparrowCore.replace_source_configuration(configuration)
  -> old-fingerprint snapshots become ineligible
  -> foreground M3U refresh + optional EPG refresh
  -> publish new catalog or expose NotConfigured/CatalogUnavailable status
```

The old catalog is not served under the new Source Configuration. Hosted HTTP has no equivalent command.

### Installed primary playback

```text
React selects ChannelId
  -> shared Playback Session reducer: starting
  -> Tauri SparrowClient.startPlayback(ChannelId, saved preference)
  -> Tauri command parser
  -> PlaybackManager stops/releases any old engine and request
  -> PlaybackManager acquires PlaybackActivityLease
  -> SparrowCore.resolve_playback(ChannelId) -> non-serializable secret
  -> native HTTP request opens once
  -> MPEG-TS PMT parser enumerates Audio Tracks
  -> preference matcher selects audio PID or visible fallback
  -> byte selector forwards video + selected audio into opaque stream handle
  -> Tauri adapter returns handle and emits safe track/telemetry events
  -> custom mpegts.js loader reads handle
  -> MSE/video element
  -> reducer: playing
```

Changing Audio Track, pausing/resuming, restarting, suspending/resuming, or bounded recovery first cancels the current handle and confirms release, then creates the replacement request. Recovery is finite and test-configurable; exhausting it yields a visible `failed` state rather than looping. Exact delay tuning remains internal and cannot weaken the no-overlap invariant.

### Linux mpv failover

```text
user presses “Open in mpv” from failed/stopped primary
  -> capability check
  -> reducer requests failover
  -> PlaybackManager proves primary handle released
  -> reuse pinned ResolvedPlaybackSource inside Rust
  -> spawn system mpv with private IPC socket and no URL logging
  -> observe startup/process/IPC state
  -> reducer: fallback-playing OR typed failure
  -> stop/session end closes IPC, terminates/reaps child, removes socket
```

### Hosted playback

```text
React selects ChannelId
  -> HTTP SparrowClient returns same-origin /api/v1/play/{encoded ChannelId}
  -> mpegts.js fetch includes browser-managed same-origin authentication
  -> Axum auth + ChannelId parser
  -> acquire PlaybackActivityLease for the response lifetime
  -> SparrowCore.resolve_playback
  -> Axum opens provider request and streams bytes
  -> disconnect cancellation drops provider request and activity lease
```

The hosted capability disables native Audio Track selection and mpv failover. The shared player presents only controls supported by capabilities.

### Observability and diagnostics

```text
internal error/cause/private values
  -> classify at owning module
  -> SafeFailure { tag, operation, retryable, safe context }
  -> tracing/DTO/diagnostic projection
```

Allowed fields include operation, source kind, state transition, safe error tag, retry count, byte count, duration, memory, catalog generation, playback engine, codec, resolution, and frame/stall counts. Arbitrary thrown values, URLs, headers, payload snippets, fingerprints, and environment dumps are forbidden.

## Runtime and transport routes

Hosted Axum exposes only versioned identifier-based routes plus static content:

```text
GET  /health
GET  /app/*
GET  /api/v1/capabilities
GET  /api/v1/status
GET  /api/v1/groups
GET  /api/v1/channels
GET  /api/v1/channels/{channelId}
GET  /api/v1/channels/{channelId}/schedule
GET  /api/v1/search
POST /api/v1/refresh
GET  /api/v1/events
GET  /api/v1/play/{channelId}
```

The root redirects to `/app/`. Raw M3U/EPG download routes and the arbitrary `/proxy/*` route are deleted. HTTP errors use a versioned safe error DTO. Tauri exposes equivalent catalog queries/status/refresh plus installed-only configuration and playback commands; unsolicited changes use Tauri events rather than polling a local server.

## Files and modules

### Add

| Path | Ownership |
|---|---|
| `rust-toolchain.toml`, `mise.toml`, `justfile` | Exact toolchain and shared local/CI command interface |
| `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.github/dependabot.yml` | Deterministic checks, candidate build/sign/attest/publish, reviewed action updates |
| `crates/sparrow-core/Cargo.toml` | Deep catalog module package |
| `crates/sparrow-core/src/domain/` | Domain values, refined inputs, views, error algebra |
| `crates/sparrow-core/src/m3u/`, `xmltv/`, `catalog/` | Private parsing, matching, identity, indexes, projections |
| `crates/sparrow-core/src/source/`, `snapshot/`, `refresh/` | Core-owned ports and refresh/publication policy |
| `crates/sparrow-core/tests/fixtures/` | Sanitized small fixtures and deterministic scale generators |
| `crates/sparrow-server/Cargo.toml`, `src/` | Axum composition root, auth, DTO parsers/projections, SSE, hosted playback |
| `app/src/client/contracts.ts`, `parsers.ts` | Transport-neutral branded client models and runtime parsing |
| `app/src/client/http.ts`, `tauri.ts` | Two `SparrowClient` adapters |
| `app/src/features/catalog/`, `search/`, `settings/` | Shared UI workflows through `SparrowClient` |
| `app/src/features/playback/` | Exhaustive Playback Session reducer, mpegts.js loader, controls, telemetry |
| `app/src-tauri/` | Tauri crate/config/capabilities plus composition root |
| `app/src-tauri/src/config_store.rs`, `snapshot_store.rs` | Atomic installed configuration and Source Snapshot adapters |
| `app/src-tauri/src/commands.rs`, `events.rs`, `lifecycle.rs` | IPC/event translation and lifecycle reporting |
| `app/src-tauri/src/playback/` | Native request/handle ownership, TS selection, Audio Track preference, mpv |
| `tests/contract/` | Shared JSON contract cases exercised through HTTP and IPC adapters |
| `tests/private/` (gitignored) | Local provider acceptance configuration/scripts with redacted output |

### Change or replace wholesale

| Path | Change |
|---|---|
| `Cargo.toml`, `Cargo.lock` | Convert to workspace and pin replacement dependencies |
| `app/package.json`, lockfile, Vite/TS config | Own product version, Tauri/client/test dependencies, strict checks |
| `app/src/App.tsx` | Replace URL/search-only root with capability-driven catalog application |
| `app/src/components/tv-player.tsx` | Replace mutable URL player with typed Playback Session modules |
| `app/src/lib/api.ts` | Replace hosted singleton with injected `SparrowClient` composition |
| `Dockerfile` | Reproducible workspace/frontend build for `sparrow-server` on port 33733 |
| `.gitignore` | Protect local configuration, snapshots, signing properties, private fixtures |

### Delete when replacement modules take ownership

- `src/main.rs`, `src/routes.rs`, `src/playlist.rs`, and `src/epg.rs`.
- `deploy.sh` and `fetch_examples.sh`.
- Legacy URL DTOs, arbitrary proxy route, hard-coded group/snippet filtering, parser tests tied to old structs, and player probe/export code not used by production diagnostics.

## RGR TDD implementation slices

Intermediate branch states may be broken under ADR 0004. Within each slice, add one caller-visible failing behavior, implement only enough through the real seam, then refactor behind that interface. Superseded tests are deleted rather than layered underneath replacements.

| Slice | Red behavior | Green result and refactor boundary |
|---|---|---|
| 1. Workspace and domain shell | A core caller cannot construct invalid Channel IDs/page limits/source input | Workspace builds; refined domain values and safe errors exist with no framework types |
| 2. M3U to Channel Catalog | Fixture cannot produce grouped playable Channels with stable duplicate-aware IDs | New M3U parser and identity implementation pass through core query interface |
| 3. Optional XMLTV enrichment | EPG cannot enrich matching Channels or degrade independently | Streaming/compact XMLTV parser, matching, schedules, channel-only fallback |
| 4. Query surface | Groups, details, schedules, search, generation, and stale pagination are absent | Immutable indexed catalog serves bounded query projections without parsing/cloning |
| 5. Atomic Source Snapshots | Crash/corruption/disk-full cases can replace last-known-good data | Temp-directory production file adapter proves stage/validate/fsync/activate/recovery |
| 6. Refresh policy | Controlled clock/source cannot prove six-hour, `304`, backoff, single-flight, playback deferral, and manual override | Core refresh state machine and safe status/events pass with deterministic adapters |
| 7. Shared client contracts | HTTP and IPC shapes can drift or trust decoded unknown values | Runtime parsers plus shared contract corpus define `SparrowClient`, errors, capabilities, events |
| 8. Hosted tracer bullet | Browser cannot browse/search/guide/play by Channel ID | Axum auth/routes/playback and HTTP client complete hosted checkpoint; arbitrary proxy deleted |
| 9. Installed catalog tracer bullet | Tauri cannot configure, persist, restart offline, refresh, or query without Axum | IPC client, atomic config/snapshots, lifecycle signals complete installed catalog path |
| 10. Native primary playback | Channel ID cannot reach visible A/V without exposing Playback Source | Rust handle manager + custom mpegts.js loader pass deterministic and target smoke checks |
| 11. Audio Tracks and recovery | PMT fixtures cannot enumerate/select/reselect/fallback without overlap | PID selector, preference matcher, exhaustive reducer, bounded recovery and diagnostics |
| 12. Lifecycle and mpv | Pause/background/stop/failover can leak requests or children | Wake-lock/resource ownership and Linux JSON-IPC failover pass contract and target checks |
| 13. Packaging and release | Exact artifacts cannot prove version/signature/provenance or target acceptance | AppImage/APK candidate workflow and manual publication gate satisfy ADR 0005 |

Property tests cover Channel Identifier stability/distinction, cursor round-trips, parser chunk boundaries, and manifest/checksum invariants. Cancellation tests assert resource release rather than call order. Snapshot crash claims use the production file adapter. HTTP/IPC tests assert parsed responses and safe errors, not private functions.

## Acceptance gates

### Core checkpoint

| Gate | Required evidence |
|---|---|
| Parsing and identity | Sanitized normal/malformed/provider-quirk fixtures; duplicate/missing `tvg-id`; stable recognizable IDs; distinct backups |
| Queries | Groups, bounded pagination, details, schedules, Channel/Programme search, generation invalidation, stale cursor behavior |
| Snapshot safety | Interrupted write, truncated payload, wrong length/checksum/fingerprint, corrupt active pointer, alternate recovery, disk-full simulation |
| Refresh | Six-hour freshness, independent M3U/EPG outcomes, `304`, 1/5/15/60 backoff, `Retry-After`, single-flight, manual override, playback deferral |
| Privacy | No URL/header/body/fingerprint in safe errors, logs, events, read models, or copied diagnostics |
| Scale | Generated representative-size catalog parses once; queries do not parse or deep-clone; raw buffers release before publication |
| Target bound | Physical Android snapshot cold-start ≤3 seconds and peak process memory ≤512 MiB; compact/stream parsers before considering a parsed sidecar |

### Hosted-web checkpoint

| Gate | Required evidence |
|---|---|
| Container | Replacement image builds from locked inputs, starts privately, reports health, and serves on 33733 |
| Authentication | Same-origin SPA/API/playback reject missing or wrong deployment credential; `/health` reveals no private state |
| Experience | Browse Channel Groups/Channels, search Channels/Programmes, inspect guide/schedule, select and switch playback |
| Identity/privacy | Network/UI state contains Channel IDs and Sparrow endpoints only; no arbitrary proxy, provider URL, config location, or raw provider body |
| Failure | Missing EPG remains usable; M3U failure exposes safe unavailable/stale status; playback disconnect cancels upstream request |
| Deployment rehearsal | Image is identified by immutable tag/digest and tested with real deployment configuration without changing Caddy or production |

### Final Linux gate

| Gate | Required evidence on the exact AppImage candidate |
|---|---|
| Startup/render | Launches on target Arch/Hyprland native Wayland with the validated DMA-BUF workaround and displays version/status |
| Catalog | First configuration, restart from saved snapshots without network, stale status, manual refresh, source change invalidation |
| Primary playback | Representative H.264/AAC picture and audible audio; start, stop, pause/live-edge resume, ordinary Channel changes, restart, fullscreen, volume/mute |
| Audio Tracks | Enumerates actual available tracks, changes track with brief restart, remembers preference, visibly falls back when absent |
| Recovery/resources | Bounded failure is visible; every stop/restart/change releases old handle; no overlap or unbounded retry |
| mpv | User invokes installed mpv only after primary release; A/V/fullscreen works; stop reaps child and removes IPC socket |

### Final Android gate

| Gate | Required evidence on the exact signed APK candidate and Realme device |
|---|---|
| Package | `apksigner` verification, expected certificate digest/application ID/version/min SDK/ABIs, clean install and upgrade from preceding release |
| Startup/catalog | ≤3-second/≤512-MiB snapshot cold start, offline saved catalog, foreground fetch when absent, stale/manual refresh status |
| Primary playback | Representative picture and audible audio; start, stop, pause/live-edge resume, ordinary Channel changes, restart, fullscreen, volume/mute |
| Audio Tracks | Same enumerate/select/preference/fallback behavior as Linux through the native selector |
| Lifecycle | Rotation preserves UI/session; background/manual lock releases request and wake lock; foreground restarts only prior active playback; resume refresh remains deferred while playback is active |
| Resources | Repeated ordinary use shows no accumulating requests, file descriptors, handles, or unreaped work; do not repeat the 30-switch stress run solely for evidence |

### Release and cutover gate

| Gate | Required evidence |
|---|---|
| Build ownership | Repository pins, frozen lockfiles, full-SHA actions, unprivileged provider-free CI |
| Candidate integrity | Exactly one versioned AppImage and universal APK, checksums, APK signature checks, GitHub provenance attestations |
| Exact-byte acceptance | Candidate SHA values tested above match the waiting workflow artifacts; any rerun invalidates prior acceptance |
| Key continuity | Encrypted offline keystore restore tested; two successively versioned signed candidates prove Android update before first public release |
| Publication | The release tag points to the current `master` commit; manual `release-publish` environment approval publishes it without rebuilding; immutable release contains AppImage, APK, `SHA256SUMS` only |
| Hard cutover | Merge complete replacement to `master`, recreate existing `sparrow` service with accepted image, leave Caddy unchanged, verify production workflows |
| Rollback | Retain old Compose/env and `ponbac/sparrow:0.11.4`; failed production acceptance recreates the same service with the old image |

One ordinary representative playback and a small number of natural Channel changes are enough for candidate acceptance. The waived 90-minute soak and completed 30-switch prototype are not repeated unless a later measured reliability defect specifically earns them.

## Risks and open questions

No unresolved architectural decision blocks implementation issue generation.

The remaining risks are already routed by evidence gates:

- If the native MPEG-TS selector cannot enumerate or select a required Audio Track, fail the audio slice and reopen ADR 0001 rather than forking `mpegts.js` silently.
- If primary Android playback fails the ADR 0001 threshold while a native engine succeeds, run the focused Media3 prototype.
- If packaged Linux WebKit cannot render with the validated workaround, use mpv immediately and reconsider Linux primary ownership.
- If Android startup/memory misses the gate, optimize parser allocation/streaming first; add a disposable parsed artifact only after a benchmark proves it helps.
- If atomic-file crash tests fail, redesign the SnapshotStore implementation before considering a database.
- If Android release-key recovery or update continuity fails, do not publish the APK.

Page-size defaults, visual styling, recovery delay tuning within a finite budget, exact private filesystem paths, and internal parser decomposition are reversible implementation choices behind settled interfaces. They are not Wayfinder fog.
