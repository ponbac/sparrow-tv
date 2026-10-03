# Hosted authenticated access

The hosted SPA at `https://tv.ponbac.xyz/app` and all `/api/v1` endpoints require
HTTP Basic authentication with the deployment `SPARROW_AUTH_USERNAME` (default
`sparrow`) and `PASSWORD`.
The browser prompts for credentials; passwords must not be placed in a `pw`
query parameter. Use HTTPS. Only `/health` and the root redirect remain public.

## Configuration

- Set `SPARROW_AUTH_USERNAME` to the desired Basic username. It defaults to
  `sparrow` when unset; empty values, colons, control characters, and values over
  128 UTF-8 bytes prevent startup. Changing it invalidates the old username.
- Set a nonempty `PASSWORD` (at most 1024 bytes) in the Sparrow service environment
  on the media box. Missing, empty, or oversized passwords prevent startup.
- `SPARROW_AUTH_MODE` may be unset or exactly `basic`. Public access has been
  removed: `public` and all other values fail startup rather than expose viewers
  or silently change access policy.
- If a deployment previously used `SPARROW_AUTH_MODE=public`, change it to
  `basic` (or remove it), restore a valid `PASSWORD`, and recreate through
  the authorized deployment process. This repository change does not update
  an already-running service; Compose `restart` does not apply changed service
  environment or image settings. Older public-capable rollback images can restore
  unauthenticated access if given `SPARROW_AUTH_MODE=public`; keep `basic` on rollback.
- Source Configuration remains environment-owned on the media box: required
  `M3U_PATH` and optional `EPG_PATH`, supplied through the existing service
  environment or `.env.local`. Viewers cannot read or write those locations.
  There are no Source Configuration writable HTTP endpoints; the capabilities
  contract remains `deployment-readonly`.
- The API continues to project redacted catalog/status/events/errors. Playback
  accepts only a Channel Identifier and relays bytes through same-origin HTTP,
  not provider locations, credentials, or upstream response headers.

## Playback recovery and phone fullscreen

Hosted playback catches up only when its forward buffer exceeds four seconds,
then retains two seconds for delivery jitter. This avoids repeated small seeks
into a nearly empty buffer while still bounding delay added by the player. The
buffer duration is not a measurement of broadcast-to-screen latency.

Hosted playback automatically retries transient interruption, timeout, or Source
unavailability after 1, 5, then 15 seconds. It stops the old engine before each
retry, cancels pending recovery on Stop or a Channel change, and resets the retry
budget after 60 seconds of stable playback. A picture that makes no progress for
15 seconds also triggers recovery. After the budget is exhausted, manual retry
remains available. Pause, blocked autoplay, and permanent failures do not trigger
automatic retries.

Phones fullscreen the player and its controls without forcing Theater. Roomy
desktop windows retain root-fullscreen Theater and guide. This does not add
MediaSource support to iPhone Safari; its existing playback limitation remains.

## Refresh and verification

Manual refresh still requires `POST /api/v1/refresh` with exactly one
`X-Sparrow-Request: refresh` header, no query, and an empty body. It refreshes the
existing Source Configuration; it cannot replace it. The API enables no CORS
and rejects cross-origin preflights, preserving the browser CSRF boundary for
this custom header. The header is additional to Basic authentication, not a
replacement for it.

Provider-free Rust contracts cover authenticated SPA, catalog, playback, events,
refresh guards, redaction, and rejection of public-mode configuration:

```sh
cargo test -p sparrow-server --all-targets --locked
```

The existing [hard-cutover](hosted-hard-cutover.md) acceptance/endpoint tooling
expects Basic authentication with the default username `sparrow`; deployments
with a custom username need corresponding operator verification. Configuration
changes and local tests do not authorize or perform a production cutover.
Do not publish Source Configuration, provider locations, passwords, or private
deployment evidence when verifying a deployment.
