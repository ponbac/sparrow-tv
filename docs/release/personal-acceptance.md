# Personal release acceptance

This gate authorizes one immutable AppImage/APK candidate artifact after the exact downloaded
bytes pass the target Arch/Hyprland and physical Realme checks. It never rebuilds either binary.
A workflow rerun changes the attempt identity and invalidates all earlier evidence, even when the
binary hashes happen to be unchanged.

The candidate bundle is read-only. Do not check boxes in `CANDIDATE-ACCEPTANCE.md`, add evidence
to the bundle, or rename its files: bundle verification deliberately rejects every extra or changed
file. Keep local observations under the gitignored `release-acceptance/` directory. The tooling
creates private files with mode `0600` and directories with mode `0700`.

## Prerequisites

- The tagged workflow is waiting at its protected `release-publish` environment.
- The candidate job succeeded and its summary shows the candidate artifact ID, artifact SHA-256,
  and candidate-manifest SHA-256.
- `release/android-signing-identity.json` contains the restored offline certificate digest.
- `gh` is authenticated to the release repository. Preparation/sealing need Actions and
  attestations read access; the reviewer running the approval command also needs Deployments write.
- The pinned Android tools are installed and `ANDROID_HOME` points to them.
- The AppImage acceptance host is the target Arch/Hyprland installation.
- The Android target is the physical Realme GT8 Pro RMX5210 on API 36. An emulator is rejected.

## 1. Download and prepare the exact candidate

Download only the artifact named `release-candidate-RUN_ID-RUN_ATTEMPT` from the waiting run into
a new directory under `release-candidates/`. Keep the run ID, attempt, artifact ID, and artifact
digest from the candidate job summary together; do not take values from a superseded attempt.

For example, after assigning the values shown by GitHub:

```bash
gh run download "$run_id" \
  --repo "$repository" \
  --name "release-candidate-$run_id-$run_attempt" \
  --dir "$candidate_dir"

RELEASE_CANDIDATE="$candidate_dir" \
RELEASE_ACCEPTANCE_OUTPUT="$evidence_dir" \
  just release-acceptance-prepare
```

`prepare` fails unless the manifest, sidecars, candidate bytes, AppImage identity, signed universal
APK identity, offline certificate digest, and GitHub build-provenance attestations all agree. It
creates three attempt-bound files:

- `acceptance-session.json`, which must not be edited;
- `linux-observations.json`; and
- `android-observations.json`.

Every observation begins as `pending`; pending, missing, false, duplicated, reordered, or unknown
gates fail sealing. Fill `recordedAt` with an offset-bearing ISO timestamp only after the complete
target flow passes. Do not add notes, provider URLs, channel names, credentials, adb serials, or raw
logs: the schemas reject extra fields so detailed evidence cannot accidentally retain private
catalog data.

## 2. Exercise the exact AppImage on Arch/Hyprland

Restore the executable bit if the download removed it. That changes file mode, not candidate bytes.
Run with the validated native-Wayland workaround:

```bash
chmod u+x "$candidate_dir"/Sparrow_*_x86_64.AppImage
WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  "$candidate_dir"/Sparrow_*_x86_64.AppImage
```

Set the Linux target fields to `arch`, `wayland`, and `hyprland`, record the version displayed by
the application, and mark a gate `passed` only after observing all behavior in that row.

| Gate ID                                     | Required observation                                                                                                |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `startup-render-version`                    | Native Wayland startup renders correctly and displays the candidate version/status.                                 |
| `browse-search-guide`                       | Browse groups/channels, search channels/programmes, and inspect guide/schedule.                                     |
| `catalog-first-configuration`               | First source configuration loads the real on-device catalog without revealing source details.                       |
| `catalog-offline-restart`                   | Restart from saved snapshots with network unavailable and retain a usable catalog.                                  |
| `catalog-stale-manual-refresh`              | Stale status and a manual refresh are visible and behave correctly.                                                 |
| `primary-picture-audio`                     | Representative H.264/AAC produces visible picture and audible audio.                                                |
| `primary-controls-channel-changes`          | Pause/live-edge resume, stop/restart, fullscreen, volume/mute, and ordinary Channel changes work.                   |
| `audio-track-selection-preference-fallback` | Enumerate/select an Audio Track, remember preference, and visibly fall back when absent.                            |
| `bounded-recovery-resource-release`         | Failures are bounded/visible and stop, restart, and Channel change leave no overlapping request or unbounded retry. |
| `mpv-fallback-cleanup`                      | Invoke system mpv only after releasing primary playback; A/V/fullscreen work and stop reaps the child/socket.       |

One representative playback and a few ordinary Channel changes are sufficient. Do not repeat the
waived 90-minute soak or the completed 30-switch prototype without a new measured defect.

After exiting, confirm the AppImage SHA-256 still matches both `SHA256SUMS` and the candidate
manifest before marking the final Linux gate passed.

## 3. Prove key continuity on the physical Realme

Use an older non-debuggable universal APK signed by the restored release key. Before the first
public release this may be the unpublished predecessor candidate; later it is the preceding
accepted release. Its deterministic filename and adjacent `.sha256` sidecar must remain intact.

The continuity command performs two fixed `adb install -r --no-streaming` operations—older APK,
then accepted APK. It never uninstalls Sparrow or clears app data. It rejects an emulator, another
phone, another API/ABI, a different package or certificate, an equal/decreasing version, changed
APK bytes, or changed Android UID/first-install identity.

```bash
RELEASE_CANDIDATE="$candidate_dir" \
RELEASE_PREVIOUS_APK="$previous_apk" \
RELEASE_PREVIOUS_VERSION="$previous_version" \
RELEASE_DEVICE_SERIAL="$realme_adb_serial" \
RELEASE_ACCEPTANCE_OUTPUT="$evidence_dir/android-key-continuity.json" \
  just release-acceptance-prove-continuity
```

This command changes the installed Sparrow version on the connected target device. Run it only
when the phone is ready for the release acceptance flow. A failed predecessor install is not worked
around with an uninstall; resolve the device/version state and start a new continuity run.

## 4. Exercise the accepted APK on that same installation

Copy the accepted UID and first-install time from the machine-generated continuity record into the
installed-package section of `android-observations.json`. Fill the exact candidate application ID,
version name/code, APK digest, and certificate digest from the candidate-bound template. Mark each
gate passed only after observing its full behavior on the physical phone.

| Gate ID                                     | Required observation                                                                                                  |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `package-identity-install`                  | The installed universal release APK shows the expected package, version, certificate, min SDK, and ABI identity.      |
| `catalog-cold-start-bounds`                 | A retained-snapshot process cold start is at most 3 seconds and peak process memory at most 512 MiB.                  |
| `catalog-offline-restart`                   | The saved catalog remains usable offline after a process restart and across the replace install.                      |
| `catalog-refresh-stale-status`              | Missing data fetches in foreground; stale/manual refresh status is correct and resume refresh defers during playback. |
| `browse-search-guide`                       | Browse, search, guide, and schedule work on the real catalog.                                                         |
| `primary-picture-audio`                     | Representative H.264/AAC produces visible picture and audible audio.                                                  |
| `primary-controls-channel-changes`          | Start/stop, pause/live-edge resume, restart, fullscreen, volume/mute, and ordinary Channel changes work.              |
| `audio-track-selection-preference-fallback` | Enumerate/select, remember, and visibly fall back with the shared native selector.                                    |
| `rotation-session-preservation`             | Rotation preserves the UI/session without duplicate playback.                                                         |
| `background-foreground-release-resume`      | Background releases request/wake state; foreground restarts only previously active playback.                          |
| `manual-lock-wake-state`                    | Manual lock releases request/wake state and unlock resumes only prior active playback.                                |
| `bounded-recovery-resource-release`         | Failures stay bounded/visible and ordinary repeated use does not accumulate requests, descriptors, handles, or work.  |

After stopping playback, confirm the staged APK SHA-256 is unchanged. Fill `recordedAt` only after
all rows pass. Rejection, a device reset, an APK substitution, or a workflow rerun requires fresh
evidence.

## 5. Seal the evidence and approve the exact attempt

Use the artifact ID and artifact digest shown by the candidate job summary:

```bash
RELEASE_CANDIDATE="$candidate_dir" \
RELEASE_ACCEPTANCE_EVIDENCE="$evidence_dir" \
RELEASE_ARTIFACT_ID="$artifact_id" \
RELEASE_ARTIFACT_DIGEST="$artifact_digest" \
RELEASE_ACCEPTANCE_OUTPUT="$sealed_dir" \
  just release-acceptance-seal
```

`seal` reverifies candidate bytes, package identities, attestations, and the live GitHub artifact's
run, attempt, name, ID, archive digest, commit, and expiry. It requires every manual gate and the
machine continuity record, then writes `ACCEPTANCE-VERDICT.json` and `APPROVAL-COMMENT.txt` into a
new private directory. The approval receipt binds the run attempt, artifact ID/digest, candidate
manifest, AppImage, APK, certificate, and sealed local evidence digest.

Inspect those two files. The following command is the explicit publication authorization: it finds
the one pending `release-publish` deployment, submits the exact receipt as the authenticated review
comment, and resumes the waiting job.

```bash
RELEASE_CANDIDATE="$candidate_dir" \
RELEASE_ACCEPTANCE_EVIDENCE="$evidence_dir" \
RELEASE_ACCEPTANCE_SEALED="$sealed_dir" \
  just release-acceptance-approve
```

Do not use GitHub's ordinary blank approval button. The resumed publication command reads the
authenticated review history and fails unless exactly one approved `release-publish` review carries
the strict receipt for the current run attempt and candidate artifact. It then reverifies the same
bundle and attestations and publishes only the existing AppImage, APK, and `SHA256SUMS`; no build
command exists in the publication job.

## Rejection and reruns

- To reject a candidate, reject the pending environment deployment in GitHub. Do not run the
  approval command.
- A rerun retains the workflow run ID but increments the attempt. Old evidence and approval
  comments are therefore rejected mechanically.
- Start from a freshly downloaded `release-candidate-RUN_ID-RUN_ATTEMPT` artifact and a new evidence
  directory. Never edit or reuse a sealed verdict.
- If key continuity, physical-device playback, or exact-byte verification cannot be completed, do
  not publish the APK or approve the release.

The authenticated receipt uses GitHub's documented [workflow-run review
history](https://docs.github.com/en/rest/actions/workflow-runs#get-the-review-history-for-a-workflow-run),
[pending deployment review](https://docs.github.com/en/rest/actions/workflow-runs#review-pending-deployments-for-a-workflow-run),
and [Actions artifact](https://docs.github.com/en/rest/actions/artifacts) interfaces. Review history
is keyed by run ID rather than attempt, which is why the receipt carries and rechecks the attempt
explicitly.
