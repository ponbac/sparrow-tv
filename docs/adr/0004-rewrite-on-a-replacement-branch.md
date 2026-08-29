# Rewrite on a replacement branch and cut over once

Sparrow TV will be rewritten on a long-lived `rewrite/sparrow-next` branch while `master` and the deployed `ponbac/sparrow:0.11.4` web application remain untouched. Because the owner is the only user and no legacy interface must survive, the simplest route is a dependency-first replacement followed by one hard Compose-service recreation after the web, Linux, and physical Android targets are complete.

## Branch and checkpoint policy

- `master` remains the production source and receives only emergency fixes during the rewrite. Every such fix is evaluated and forward-ported to the replacement branch immediately so the branches do not drift silently.
- Old implementation is removed as its replacement becomes useful. The branch carries no dual parser, compatibility adapter, legacy HTTP/DTO layer, or long-lived feature flag merely to keep intermediate states deployable.
- Individual commits and intermediate states may fail to build or run when that makes the rewrite easier. Coherent Jujutsu commits preserve understandable work, but only named checkpoints must be green.
- The green checkpoints are: the complete core interface and tests; the complete hosted web replacement; and the complete web/Linux/Android replacement ready for merge and deployment.
- `master` is not replaced when only the hosted web checkpoint passes. The final merge waits for the rewritten hosted web client, Linux AppImage, and physical-device Android APK to pass their applicable architecture and packaging gates.

## Dependency-first order

1. Convert the repository to the intended workspace shape and complete `sparrow-core`: new M3U/XMLTV parsers, Channel Catalog, Channel Identifiers, EPG matching, query/search interface, refresh policy, source access, snapshot interfaces, and deterministic test adapters.
2. Rewrite the transport-neutral `SparrowClient` contract and shared React application against the new read models, commands, events, errors, and capabilities. It need not run before an adapter exists.
3. Add the Axum/HTTP adapter, hosted ID-based playback, server configuration, and container build. This creates the first complete replacement application and the hosted-web checkpoint.
4. Add the atomic device snapshot adapter, Tauri IPC adapter, native playback stream, Audio Track selection and preference, app lifecycle handling, Linux mpv failover, and platform packaging.
5. Run the final web, Arch/Wayland, physical Android, corruption/recovery, startup/memory, and artifact checks from the accumulated ADRs and release decision. Only then merge the branch to `master`.

## Verification strategy

- The old application is a list of user workflows, not a behavioral oracle. Preserve channel browsing/search, Programme search/schedules, and playback/switching; do not assert byte-for-byte responses, legacy DTOs, filtering quirks, URL leakage, or old parser behavior.
- Core-interface tests define expected domain results directly. Small sanitized M3U/XMLTV fixtures cover normal input, duplicate/missing identifiers, provider quirks, malformed responses, and EPG matching. Deterministically generated large feeds cover pagination, search, startup, and memory behavior.
- Private provider payloads and credentials remain excluded from Git. Local acceptance commands use the real M3U and optional EPG sources on the target machine and devices; ordinary CI never contacts the provider, avoiding secret exposure and rate-limit surprises.
- The core checkpoint requires the new parser, identity, query, refresh, error, and event contracts to pass through deterministic source, clock, and snapshot adapters. Superseded implementation-level tests are deleted rather than layered under the new interface tests.
- The hosted-web checkpoint requires a production container built from the branch to start privately with the real server configuration and pass its search, guide, Channel Identifier, ID-based playback, privacy, and performance smoke checks.
- The final checkpoint requires the same shared features through Tauri IPC, cached/offline startup, lifecycle and refresh behavior, primary playback and Audio Track selection on Linux and the physical Android device, manual mpv failover on Linux, and the cold-start/memory and packaging gates already recorded.

## Cutover and rollback

- Build and identify the accepted Docker image immutably, merge the accepted replacement branch to `master`, update the existing `sparrow` Compose service to that image, and recreate that service on its existing port. Brief downtime is accepted.
- Caddy remains unchanged and continues routing `tv.ponbac.xyz` to `sparrow:33733`. There is no blue/green service, alternate hostname, or proxy switch.
- Preserve the previous image tag/digest, the old Compose configuration, and its environment until the replacement is explicitly accepted in production. A failed cutover rolls back by restoring `ponbac/sparrow:0.11.4` and recreating the same service.
- The hosted application has no durable data migration. Installed Source Snapshots are derived, versioned data and may be discarded/rebuilt under ADR 0002, so they cannot prevent application rollback.

## Rejected alternatives

- Incrementally rewriting `master` is rejected because the current hosted application is the only production fallback during development.
- A compatibility/strangler layer inside the new code is rejected because no external contract or multi-user rollout earns its cost.
- Blue/green Caddy switching is rejected because a single-user hard replacement plus retained image rollback is simpler and sufficient.
- Requiring every commit or partial slice to remain runnable is rejected because the isolated replacement branch removes that delivery constraint.
- Differential tests against the old implementation are rejected because they would preserve defects and transport-shaped behavior that the rewrite exists to remove.

## Revisit gates

Reconsider this route if another user or external client begins depending on the hosted application, if `master` requires sustained feature development during the rewrite, or if a hard replacement can no longer tolerate its expected downtime. Any such change may earn green intermediate commits, a compatibility seam, or blue/green deployment; none is justified now.
