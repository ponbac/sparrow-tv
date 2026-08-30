# Release maintainer setup

The release workflow is intentionally unusable until the repository owner completes these
one-time controls. Do not push a release tag while any item is incomplete.

1. Restore the long-lived Android release keystore from its encrypted offline backup. Record
   its lowercase, colon-free SHA-256 certificate digest in
   `release/android-signing-identity.json`. Never commit the keystore or its passwords.
2. Create the `release-signing` GitHub environment. Make the repository owner its sole required
   reviewer, allow that owner to review a run they initiated, disable administrator bypass, and
   select custom deployment policies consisting of exactly the `master` branch and stable `v*`
   tags. Create these environment-scoped secrets:
   `ANDROID_RELEASE_KEYSTORE_BASE64`, `ANDROID_RELEASE_KEYSTORE_PASSWORD`,
   `ANDROID_RELEASE_KEY_ALIAS`, and `ANDROID_RELEASE_KEY_PASSWORD`.
3. Create the `release-publish` GitHub environment. Use the same sole owner reviewer,
   self-review, and disabled-administrator-bypass settings, but select exactly one custom
   deployment policy: stable `v*` tags. The workflow queries each environment and its typed
   deployment-policy list, then fails closed on any missing, extra, or unreadable protection.
4. Protect stable `v*` tags from deletion or movement and enable immutable GitHub Releases.
   These repository controls close the race between the workflow's final remote-ref check and
   GitHub Release creation.
5. Before the first public APK, build two successively versioned candidates with the restored
   key and prove an in-place update on the physical Android device.

For a release, commit the version bump to `master`, push the matching
`vMAJOR.MINOR.PATCH` tag, and wait for the candidate bundle. Download that exact bundle and
complete every Arch/Wayland and physical-Android item in `CANDIDATE-ACCEPTANCE.md`. Approving
`release-publish` authorizes only the hashes and workflow attempt shown there. Rejecting or
rerunning a candidate invalidates earlier acceptance.

Manual workflow dispatch is rehearsal-only. It can build candidates but cannot publish, and it
must be dispatched from the current `master` commit.
