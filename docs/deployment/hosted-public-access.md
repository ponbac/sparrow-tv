# Hosted public access

For the hosted service at `https://tv.ponbac.xyz/app`, explicitly set this in the
Sparrow service environment on the media box, then recreate/restart the service
through the authorized deployment process:

```dotenv
SPARROW_AUTH_MODE=public
```

**This is Internet-public access, not a passwordless private login.** Anyone who
can reach the service can load the SPA, browse/search the Channel Catalog and
guide, start Playback Sessions, subscribe to events, and request a refresh.
There is no login, Basic challenge, or `pw` query requirement. Provider bandwidth
and refresh activity are consequently accessible to unauthenticated clients.
TLS does not restrict who can watch. Do not enable this mode accidentally.

## Configuration and rollback

- An unset `SPARROW_AUTH_MODE` defaults to the existing `basic` mode, requiring
  `PASSWORD` and Basic username `sparrow`. Only exact `public` and `basic` values
  are accepted; empty, misspelled, or differently cased values fail startup.
- Public mode does not require or read `PASSWORD`. An old value may remain in
  the media box environment for rollback; even an empty or oversized retained
  value cannot make public mode require authentication.
- To restore Basic on this server, set `SPARROW_AUTH_MODE=basic` (or remove the
  setting), ensure a valid `PASSWORD` remains configured, and restart. Older
  rollback images retain their own authentication behavior.
- Source Configuration remains environment-owned on the media box: required
  `M3U_PATH` and optional `EPG_PATH`, supplied through the existing service
  environment or `.env.local`. Viewers cannot read or write those locations.
  There are no Source Configuration writable HTTP endpoints; the capabilities
  contract remains `deployment-readonly`.
- The API continues to project redacted catalog/status/events/errors. Playback
  accepts only a Channel Identifier and relays bytes through same-origin HTTP,
  not provider locations, credentials, or upstream response headers.

## Playback recovery and phone fullscreen

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
this custom header. This is not authentication: non-browser clients can supply
the header and refresh the public service.

Provider-free Rust contracts cover public SPA, catalog, playback, events,
refresh guards, and redaction while retaining Basic contracts:

```sh
cargo test -p sparrow-server --all-targets --locked
```

The existing [hard-cutover](hosted-hard-cutover.md) acceptance/endpoint tooling
expects Basic authentication; it is not public-mode deployment evidence. This
setting and its local tests do not authorize or perform a production cutover.
Do not publish Source Configuration, provider locations, passwords, or private
deployment evidence when verifying a deployment.
