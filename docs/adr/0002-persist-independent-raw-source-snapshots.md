# Persist independent raw Source Snapshots in atomic file slots

Sparrow TV will persist the validated raw payload for each configured M3U Source and EPG Source independently in two crash-safe slots under the platform app-data directory. The current representative payload is about 75 MB of M3U plus 31 MB of XMLTV; reparsing both on the target Arch system took 863 ms and peaked at about 305 MB (298 MiB), so SQLite or redb would add a database lifecycle without avoiding the measured read-and-parse cost.

## Boundaries

- Each source owns an active and inactive slot containing its raw payload and a versioned manifest. The manifest records a non-secret source fingerprint, UTC `validated_at`, decoded byte length and checksum, and any private `ETag` or `Last-Modified` validator supplied by the source. The app-data directory and files are private to the user.
- A refresh streams into the inactive slot, enforces decoded limits of 128 MiB for M3U and 64 MiB for EPG, and fully validates the result. M3U must parse into at least one playable Channel; EPG must parse into at least one channel record. The implementation synchronizes staged files, atomically replaces a same-directory active-slot pointer, and synchronizes the directory. An invalid, oversized, interrupted, or disk-full refresh cannot replace the last-known-good snapshot.
- M3U and EPG advance independently. A valid M3U update is not blocked by optional EPG failure; catalog enrichment uses whatever matching EPG snapshot is valid for its configured source.
- A source-configuration change immediately makes snapshots with another source fingerprint ineligible. An unchanged source retains its snapshot. URLs, credentials, raw payloads, and identifying fingerprints stay out of routine logs and copied diagnostics.
- On launch, a valid snapshot is loaded even when stale. Missing M3U requires a foreground fetch; missing EPG produces a channel-only catalog. A stale snapshot remains valid indefinitely while refresh is attempted in the background.
- Freshness lasts six hours from the last complete validation or successful HTTP `304 Not Modified`; an implausibly future timestamp is stale. Startup, resume, and the six-hour boundary trigger automatic refresh, but automatic work waits for an active Playback Session to end. Manual refresh bypasses freshness and retry delay, coalesces duplicate requests, and reports M3U and EPG outcomes separately.
- Automatic failures back off for 1, 5, 15, then 60 minutes and remain capped at one attempt per hour, honoring a longer provider `Retry-After`. Failure never advances `validated_at`.
- Startup validates the active slot's manifest, source fingerprint, size, checksum, and parseability. If it is damaged, the newest valid matching alternate is used and the bad slot is replaced by a later refresh. If neither slot is valid, the derived files are discarded and rebuilt without crash-looping.
- Catalog status reports each source's last successful update and whether it is fresh, updating, using saved data, failed, or unavailable. Copied diagnostics include timings, sizes, and recovery state while excluding source locations, credentials, raw payloads, and identifying fingerprints.

## In-memory model

The durable Source Snapshots are not the request-time catalog. Rust parses them once at process start and after successful refresh, then publishes one immutable `Arc<ChannelCatalog>`; requests clone only the pointer. Only one refresh candidate may exist, raw parse buffers are released before publication, and the prior catalog remains available until readers release it.

No parsed sidecar or Moka cache is part of the initial design. The physical Android acceptance gate is at most three seconds for snapshot cold-start and at most 512 MiB peak process memory. If either is missed, first compact or stream the parsers; add a versioned parsed artifact only when a target-device benchmark proves that it helps. Moka remains eligible only for a separately measured many-key cache and never for durability.

## Rejected alternatives

- SQLite BLOB rows and redb values are rejected for the initial two-payload store because their transactions do not remove whole-payload parsing, while adding schema, recovery, build, and inspection obligations.
- Tauri Store is rejected for bulk snapshots because its JSON-oriented direct-save path does not provide the required last-known-good activation protocol.
- Persisting only a parsed model is rejected because parser and domain changes must remain rebuildable offline from provider input.
- Pairing M3U and EPG in one transaction is rejected because it would make optional guide failure block a valid channel update.

## Revisit gates

Reconsider the backend if the app needs indexed durable programme queries, multiple source configurations, or other transactional records; if the two-slot protocol fails crash-injection or corruption tests; or if target-device measurements cross the startup or memory gates after parser optimization. Crossing a query-growth gate favors SQLite; crossing only a measured startup gate favors a disposable parsed artifact while retaining raw Source Snapshots as authority.
