# Agent Control

Opt-in local commands for a **running installed** Sparrow on Linux. Search the Channel Catalog, start a Playback Session, and read playback diagnostics without driving the desktop compositor.

This is not MCP, not WebMCP, and not a production HTTP API. The installed app still has no localhost catalog server (see [ADR 0003](../adr/0003-share-one-core-across-sibling-adapters.md)). The listener exists only when the process environment opts in.

The in-repo CLI is `scripts/agent-control/cli.ts`, invoked as `just agent …` from the repository root.

## When to use it

- Reproduce or watch in-app Linux playback (WebKit/MSE) on a real Channel, for example `Eurosport 1 FHD SE`.
- Confirm whether a freeze is a session restart loop (`phase` bouncing through `recovering`) or a stuck picture (`standstills`, `msSinceTimeAdvance`).
- Copy a diagnostics snapshot after a change, before more player-policy tweaks.

Do not use it to replace unit tests. Do not scrape the DOM or send Hyprland mouse events.

## Enable and start

The binary binds a Unix socket only when **both** variables are set in the **same** environment that launches Sparrow:

| Variable | Rules |
|---|---|
| `SPARROW_AGENT_SOCKET` | Absolute path, ≤100 bytes, no `..`. Must be under `$XDG_RUNTIME_DIR` or `/tmp`. Mode `0600`. |
| `SPARROW_AGENT_TOKEN` | 16–128 printable ASCII characters (no space). Compared in constant time. Never logged. |

If the socket is set but the token or path is invalid, the app prints `sparrow-agent-control: unavailable` and does not listen. On a successful bind it prints `sparrow-agent-control: listening`.

Linux desktop only. Ordinary AppImages and `tauri dev` include the code; it stays inert without the env vars. Rebuild after Agent Control, player, or diagnostics changes — an already-running AppImage will not pick them up.

```sh
export SPARROW_AGENT_SOCKET="${XDG_RUNTIME_DIR}/sparrow-agent.sock"
export SPARROW_AGENT_TOKEN="$(openssl rand -hex 16)"

just build-appimage
# Launch the AppImage from this same shell so it inherits the variables.
# Desktop-file launches will not, unless you wrap Exec.

just agent ping
```

`ping` with `ready: true` means the installed UI has bound catalog search. `unavailable` usually means the webview is not up yet, or the app was started without the env vars.

Pass CLI flags through just with `--`, for example `just -- agent --help`.

## Commands

| Command | What it does |
|---|---|
| `just agent ping` | Handshake. Does not tune. |
| `just agent search <term>` | Channel names and groups (no identifiers required for the next step). |
| `just agent tune <term>` | Search, pick one Channel, start in-app playback. |
| `just agent snapshot` | Current Channel name plus diagnostics JSON. |
| `just agent wait --phase playing --timeout-ms 45000` | Poll `snapshot` until `diagnostics.phase` matches. |
| `just agent wait --media-advancing --timeout-ms 45000` | Poll until media time is moving (see below). |
| `just agent stop` | Stop the Playback Session. Idempotent if nothing is playing. |

`tune` and `search` take the rest of the argv as the term, so this is valid:

```sh
just agent tune Eurosport 1 FHD SE
```

**Unique name required.** Exact canonical match wins within a complete search. A term that hits several Channels returns `ambiguous-channel` with names and does not start playback. If results exceed one bounded page, `tune` returns `search-incomplete`: use a more specific name instead of guessing from truncated results. `Eurosport` is typically ambiguous; `Eurosport 1 FHD SE` is the intended sample.

`stop` also cancels a pending tune before the player mounts. It reports `cleanup-failed` if release cannot be confirmed; it never acknowledges a successful stop while cleanup is blocked. Timed-out or disconnected requests cannot later commit a pending tune.

Wait flags can be combined (AND). `--media-advancing` requires **two consecutive snapshots** for the same Channel with `phase: "playing"`, unpaused media, an unchanged recovery count, and increases in **both** `currentTimeMs` and `presentedFrames`. A single old observation or a low `msSinceTimeAdvance` is not proof of motion. The wait deadline includes socket requests and polling delays.

Exit codes: `0` ok, `1` CLI/usage, `2` app error envelope, `3` wait timeout (includes `last` snapshot).

## Reading a snapshot

`snapshot` looks like:

```json
{
  "ok": true,
  "result": {
    "_tag": "snapshot",
    "channelName": "Eurosport 1 FHD SE",
    "diagnostics": {
      "version": 2,
      "phase": "playing",
      "recoveryCount": 0,
      "media": {
        "readyState": 4,
        "paused": false,
        "currentTimeMs": 12000,
        "bufferAheadMs": 8000,
        "waiting": 1,
        "standstills": 0,
        "msSinceTimeAdvance": 80,
        "presentedFrames": 240,
        "totalVideoFrames": 240,
        "droppedVideoFrames": 0
      },
      "transitions": []
    }
  }
}
```

`diagnostics` is the same allowlist as in-app **Copy diagnostics** (no Playback Source, no URLs, no handles). `channelName` is extra and exists only on this socket.

| Field | How to read it |
|---|---|
| `phase` | Session machine: `starting`, `playing`, `recovering`, `failed`, … |
| `recoveryCount` / `transitions` | Restart loop if these keep climbing while you watch. |
| `media.standstills` | Frozen picture with `currentTime` not advancing. |
| `media.msSinceTimeAdvance` | Age of last media-time tick. Large + `standstills` ≥ 1 = stuck. |
| `media.waiting` / `stalledEvents` | Browser buffering. |
| `media.bufferAheadMs` / `readyState` | Data present vs starved. |
| `media.presentedFrames` vs `totalVideoFrames` | Decode vs display. |

A first picture that never moves: ON AIR in the UI with `phase: "playing"`, rising `standstills`, and `msSinceTimeAdvance` in the seconds. A watchdog restart loop: `phase` flipping through `recovering` and growing `recoveryCount`.

If `diagnostics` is `null`, nothing is mounted yet — wait after `tune`, or the tune failed. If `media` is `null`, there is no current observable in-app transport; counters from released transports are not reused.

### Audio requires separate evidence

Video-frame and media-time progress do **not** prove audio output. Check the selected Audio Track, mute and volume controls, then listen or capture Sparrow's own audio output. A recording of the whole desktop can accidentally prove another application's audio instead. Neither an available Audio Track nor `audio.selection` alone proves that samples reached the speakers.

The native transport preserves the selected audio PID, including MPEG-1/2 audio and AC-3. Do not restore video by silently dropping audio packets while leaving a track marked selected. Codec incompatibility must surface as a playback failure or be handled by a tested engine adaptation; **Open in MPV** remains an explicit user choice.

## Suggested loop for a live Channel

```sh
just agent ping
just agent search "Eurosport 1 FHD SE"
just agent tune "Eurosport 1 FHD SE"
just agent wait --phase playing --timeout-ms 45000
just agent wait --media-advancing --timeout-ms 45000
just agent snapshot
# watch 20–60s
just agent snapshot
```

If the second snapshot shows new `standstills` or `msSinceTimeAdvance` stuck above a few seconds while `presentedFrames` is unchanged, the picture is frozen. Paste that JSON (not provider URLs) when asking for a player change.

Omarchy `omarchy capture screenshot fullscreen save` can sit beside this as visual proof. It does not replace `snapshot`.

## Privacy

- Do not print `SPARROW_AGENT_TOKEN` or the socket path in tickets.
- Do not put Playback Sources or playlist URLs in snapshots, logs, or commits.
- Search terms are not written to the app log. Treat Channel names in `snapshot` as the user’s catalog, not as something to publish.
- Keep screenshots, audio captures, recordings, and raw debug logs outside the repository. Share only reviewed, anonymized evidence.

## Verification

`just check` includes frontend command/lifecycle tests, actual mpegts.js IO/demuxer pause/resume regressions, Rust socket framing/deadline tests, and CLI tests against real Unix sockets (including EOF, oversized responses, shell metacharacters, and motion/deadline semantics). These checks do not replace a rebuilt AppImage test with moving video **and** verified audio output.

## Implementation map

| Piece | Location |
|---|---|
| Socket, token, eval dispatch | `app/src-tauri/src/agent_control.rs` |
| Command execution | `app/src/features/agent-control/agent-control.ts` |
| Catalog / player bindings | `app/src/features/agent-control/agent-control-binding.ts` |
| Webview reply bridge | `app/src/features/agent-control/installed-agent-bridge.tsx` |
| CLI | `scripts/agent-control/cli.ts` |
| Diagnostics allowlist | `app/src/features/playback/installed-playback-diagnostics.ts` |

The synthetic C harness in `scripts/debug/linux-playback-lab/` is a different tool: fixture streams and renderer variants, not the installed catalog.
