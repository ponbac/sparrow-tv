# Android catalog performance acceptance

This runbook measures issue #26 on the required physical Realme GT8 Pro
(RMX5210), Android 16 / API 36. The repository-owned harness rejects emulators,
other models, other API levels, non-arm64 candidates, online devices, and source
snapshots smaller than the agreed representative data set before collecting an
acceptance result.

## Prepare the candidate and device

Build one debuggable, API-36 APK containing
`lib/arm64-v8a/libsparrow_installed.so`. The debuggable candidate is deliberate:
it lets the harness read only a count-and-boolean readiness marker from the
WebView and app-owned process counters through `run-as`. It never exposes or
records rendered names, source locations, credentials, validators, or payload
contents; after baseline timing, payload bytes are consumed only by local hash
commands needed to prove corruption and exact restoration.

On the physical device:

1. Install and configure Sparrow, refresh the agreed large source while online,
   and perform enough content-changing refreshes to populate both atomic slots.
   Both M3U slots must be at least 64 MiB and both EPG slots at least 24 MiB.
2. Enable airplane mode, disable Wi-Fi, and leave the device awake and unlocked.
   The harness verifies all three conditions: airplane mode, Wi-Fi disabled, and
   no route to a fixed public IP.
3. Connect exactly the target device with adb and note its explicit serial from
   `adb devices -l`.

The host needs Bun, adb, and `apkanalyzer`. Run from the repository root with a
new evidence filename; the tool refuses to overwrite an existing file:

```sh
mise exec -- just android-catalog-accept \
  /absolute/path/to/app-universal-debug.apk \
  REALME_ADB_SERIAL \
  artifacts/android-catalog-acceptance.json
```

Use the actual debuggable candidate path. The serial selects the device but is
never written to evidence.

## What the harness proves

The tool copies the APK once into a private host directory, makes that staged
copy read-only, and uses only that copy for analysis, hashing, and installation.
It re-hashes the stage after installation, verifies the installed package's
version, target SDK, and debuggable flag, and compares the installed `base.apk`
digest when Android permits the shell to read it. It then runs three distinct
process-cold offline launches. The timer starts before the
activity launch and stops only when the installed UI exposes local IPC, a
retained catalog, its complete 24-Channel first page, group controls, and search.
It does not use Android's `am start -W` timing as the readiness result. It then
selects the first card without reading its content and waits for Channel details
to resolve, covering initial browse work.

During load and browse, the tool samples the main process's `VmHWM` and `VmRSS`
through its own UID. `VmHWM` is gated at 524,288 KiB; total PSS and cgroup memory
are supplemental because Android availability differs by build. All three
baseline runs must reach readiness in at most 3,000 ms and remain within the
memory gate.

Before every launch the tool revalidates the physical-device identity and proves
offline state again: no routed public IPv4 or IPv6 probe and bounded TCP failure
in addition to airplane mode and disabled Wi-Fi.

Finally, the tool force-stops Sparrow and freshly resolves the active M3U slot.
It makes a private exact backup, then revalidates the current pointer and the
active payload's canonical package-private path, regular-file type, link count,
mode, device, inode, and size immediately before replacing one nonzero byte with
a zero byte without changing length. Success requires the pointer to move from
that recorded slot to the other checksum-validated slot and remain queryable.
The original payload is restored and synced before the original pointer is
restored and synced. Backups are removed only after exact hashes prove both
restorations. If process quiescence or either restoration cannot be proven, no
unsafe follow-on restore is attempted and the private backup is retained.

The device-side checks deliberately depend on Android shell primitives `ip`,
`toybox nc` with IPv4/IPv6 and bounded-time options, `realpath`, `stat` with the
requested format fields, `sha256sum`, and `sync FILE`. OEM builds can differ;
the harness treats a missing primitive, unsupported option, unexpected exit, or
host-side timeout as rejection rather than weakening the check.

The JSON result is `accepted` only when every timing, memory, privacy,
repeatability, recovery, and restoration gate passes. Snapshot evidence records
only that the published representative thresholds and redundant-slot checks
passed; it omits exact payload sizes. Device evidence also omits the adb serial.
Safe failure tags remain in rejected evidence so a failed gate can drive
parsing/indexing/allocation measurement without disclosing provider data.
