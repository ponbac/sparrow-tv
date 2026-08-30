# Hosted hard cutover

The repository tooling prepares and verifies evidence; it never changes production, Compose, Caddy, GitHub settings, tags, releases, or registry contents. An authorized operator performs the actual cutover outside these commands.

## Fixed contract

`deployment/hosted-contract.json` fixes the service as `sparrow`, container port `33733`, public origin `https://tv.ponbac.xyz`, unchanged Caddy upstream `sparrow:33733`, and the immutable 0.11.4 rollback manifest. The structured hosted acceptance binds the #23-tested image digest and revision. The deployable registry reference may have another name only when it resolves to those same bytes. Readiness queries GitHub to prove that revision is an ancestor of the final merged release commit.

Legacy 0.11.4 is checked only for its UI and search liveness. The replacement must pass health, authentication, refresh, browse, search, guide, playback, and privacy checks.

The local hosted acceptance is not authoritative by itself. Readiness queries GitHub and requires issue #23 to be closed plus exactly one owner-authored comment whose complete body is `hosted-acceptance-v1 sha256=<acceptance-file-sha256> revision=<40-character-revision> image=<sha256:digest>`. No such approval existed when this contract was implemented, so it remains an explicit live owner gate rather than fabricated evidence.

## Private evidence

Keep evidence under ignored `hosted-cutover-private/` and `hosted-cutover-evidence/` directories with mode 0700. Create a private 32-byte evidence key, for example with `umask 077` and `head -c 32 /dev/urandom > hosted-cutover-private/evidence.key`. The tooling stores keyed bindings, never raw Compose, environment, Caddy, response bodies, logs, passwords, or provider data. Do not publish the key or evidence directory.

The endpoint verifier accepts only loopback HTTP for disposable baseline/candidate/rollback checks and exactly the fixed HTTPS production origin for production evidence. Passwords enter curl through a private temporary config and Docker through the process environment. Docker can expose container environment values through inspection, so use only synthetic rehearsal credentials and data.

## Safe sequence

1. Produce structured #23 hosted acceptance for the exact immutable image with `just hosted-candidate-accept`. The contract pins the fixture manifest; evidence binds it and the committed fixture-script digest. The labelled fixture and candidate run on an internal network without a host gateway and delegate checks to the hardened verifier. Never use `.env.local` or production/provider credentials.
2. Prepare a plan while the pinned baseline is still running. Supply baseline and candidate Compose backups as fully rendered canonical JSON captured with `docker compose --project-name <live-project> ... config --format json`; unresolved includes, env files, builds, configs, secrets, and extends are not accepted. The exact dotenv backup must semantically reproduce the rendered Sparrow environment. Supply a private sanitized Caddy projection containing exactly `{"schemaVersion":1,"routes":[{"host":"tv.ponbac.xyz","upstream":"sparrow:33733"}]}`; this is an explicit owner-observed projection because production Caddy is not repository-accessible. Supply the immutable deployable image reference. `just hosted-cutover-prepare` only reads and verifies them.
3. Run `just hosted-cutover-rehearse`. It creates a uniquely labelled, internal/no-egress Docker network, synthetic M3U/EPG fixture, and executes baseline → candidate → rollback on random loopback ports. It cleans every owned container and network on exit. The Docker context must be local Unix-socket Docker.
4. After explicit merge authorization, fast-forward the reviewed stack. The tag must point at current `master`; build the release candidate and repeat #33 exact-byte Linux/Android/key-continuity acceptance and publication approval.
5. Run `just hosted-cutover-readiness`. It recomputes private file bindings, inspects the exact baseline runtime and immutable registry manifests, and queries GitHub for master/tag ancestry, successful workflows, and publication approval.
6. Run `just hosted-cutover-observe-start` immediately before the authorized phase. After the operator action, record fixed-origin endpoint evidence, run `just hosted-cutover-bind-route` to bind the inspected daemon/container, endpoint bytes, Caddy snapshot, and explicit owner acknowledgement, then run `just hosted-cutover-observe-finish` with that route proof. The keyed chain enforces start → route → finish on one kernel boot and derives conservative elapsed time from boot uptime. Do not change Caddy.
7. On recreate, health, topology, crash, or endpoint failure, restore the pinned legacy manifest and verify legacy liveness. A registry pull failure before mutation records the retained baseline without claiming a rollback. If restoration cannot be verified, seal an incident-open result; never seal acceptance.
8. Seal the bounded observation promptly with `just hosted-cutover-seal`. Downtime is derived only from the observed outage start and restoration timestamps.

All Just recipes are zero-argument and take explicit environment variables. Outputs must not already exist; the writer rejects symlinks and creates private files atomically. Treat input snapshots as immutable evidence and retain them privately.

## External gates

Repository completion cannot supply merge authorization, the final master/tag, successful hosted and release workflows, physical-device acceptance, publication approval, registry promotion, production Compose/environment/Caddy backups, production Docker access, the cutover itself, or rollback/endpoint observations. Until each live gate is supplied and verified, production is not ready.
