# Sparrow TV — notes for agents

Use the domain language in [`CONTEXT.md`](CONTEXT.md). Channel, Playback Session, Playback Source, and Agent Control are defined there.

## Installed playback on Linux

Do **not** drive the in-app player by clicking Hyprland windows, injecting compositor keys, or OCR. That path is scaled XWayland and will waste the session.

Use **Agent Control**: a local Unix-socket CLI that searches the Channel Catalog, starts a Playback Session, and reads diagnostics.

Full procedure, environment, commands, and how to read a snapshot: [`docs/debug/agent-control.md`](docs/debug/agent-control.md).

Screenshots (for example Omarchy capture) are evidence that a picture moved. They are not how you steer Sparrow.

## Privacy

Never copy Playback Sources, provider URLs, tokens, or `SPARROW_AGENT_TOKEN` into issues, chats, or commit messages. Agent Control `snapshot` is allowlisted diagnostics plus a Channel name. **Copy diagnostics** in the UI omits the Channel name.
