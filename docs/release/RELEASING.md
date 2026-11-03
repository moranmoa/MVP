# Releasing (closed alpha)

From a tag to a published, signed pre-release. CI builds and hashes, but it
never sees the update root key. **Signing is always a manual, offline step
by the owner.**

```
tag vX.Y.Z ──> Release workflow ──> draft pre-release (unsigned)
                                     ├─ mvs-X.Y.Z-<platform>.zip|.tar.gz   updater artifacts
                                     ├─ mvs-ui-X.Y.Z.tar.gz                built UI
                                     ├─ SHA256SUMS
                                     └─ manifest-body.json                 UNSIGNED
owner, offline: updater sign-manifest ──> manifest.json ──> upload ──> publish
```

## What the workflow does

[`.github/workflows/release.yml`](../../.github/workflows/release.yml), on a
`v*` tag:

1. **check**: the tag must be `vMAJOR.MINOR.PATCH`. core/updater accepts
   no `-alpha.1` style suffixes; the channel says "alpha" instead.
2. **ui**: licence check, `npm ci --ignore-scripts`, `npm run build` →
   `mvs-ui-X.Y.Z.tar.gz`.
3. **core** (matrix): `cargo build --release --locked` for
   `windows-x86_64`, `macos-x86_64`, `macos-aarch64` and `linux-x86_64`. The
   platform archive `mvs-X.Y.Z-<platform>` holds `bin/` (core binaries),
   `ui/` and a README. Everything is **unsigned** (no code-signing certs yet;
   Windows SmartScreen and macOS Gatekeeper will warn testers).
4. **publish**: `SHA256SUMS` over every file, then
   [`manifest-body.mjs`](../../.github/scripts/manifest-body.mjs) writes
   `manifest-body.json`: channel `alpha`, the version, one
   `{sha256, size}` entry per platform, valid for 14 days from the build.
   Then it creates the draft pre-release and uploads everything. If a
   release for the tag is already published, it stops, so a published
   release's assets are never replaced.

**Who can write (GAP-20).** The build jobs (ui, core) run third-party code:
crates, npm packages and the toolchain action. So they get a read-only token
and **no caches** (no rust-cache, no npm cache), which means a cache poisoned
by a PR build can't end up in a release binary. They hand their files to
`publish` as workflow artifacts (kept 7 days). `publish` is the only job with
`contents: write`. It runs only GitHub's own actions, the runner's
preinstalled `gh` and `node`, and our own dependency-free script. Release
builds are slower without the caches (cold `cargo build` on every platform);
that's the price.

The desktop shell is not chosen yet. The **SHELL PACKAGING PLUG-IN POINT** in
the core job has sketched Tauri and Electron steps behind the repository
variable `MVS_SHELL` (unset = plain archives). Whichever shell wins, its
updater payload must be named `mvs-X.Y.Z-<platform>.zip|.tar.gz`, because
that file name is how the manifest body finds each platform's artifact.

## One-time setup (owner)

Covered in `core/updater/SECURITY.md` (PR #8). In short:

- On the **offline signing machine**, build `updater` yourself from a
  reviewed commit (`cargo build --release -p updater` in `core/`). Don't use
  a binary downloaded from a release: the tool that signs should not come
  from the pipeline it vouches for.
- `updater keygen --out E:\mvs-keys` (removable or encrypted drive, never a
  synced folder), then `updater make-root --threshold 1 --pub E:\mvs-keys\root.pub --out root.json`.
  `root.json` (public) is what the app embeds.

## Cutting a release (owner)

PowerShell on your normal (online) PC, in the repo:

1. **Tag and push** the reviewed commit on `main`:

   ```powershell
   git switch main; git pull
   git tag -a v0.1.0 -m "MVS 0.1.0 closed alpha"
   git push origin v0.1.0
   ```

2. **Wait for the Release workflow** (Actions tab). Its summary lists the
   checksums. The release is a draft: only repo collaborators can see it.

3. **Download and check** the draft's files:

   ```powershell
   gh release download v0.1.0 --repo moranmoa/MVP --dir release-v0.1.0
   cd release-v0.1.0
   Get-Content SHA256SUMS | ForEach-Object {
     $sum, $file = $_ -split '\s+', 2
     if ((Get-FileHash $file -Algorithm SHA256).Hash.ToLower() -ne $sum) { "MISMATCH $file" } else { "ok $file" }
   }
   Get-Content manifest-body.json
   ```

   Read the body: `version` matches the tag, `channel` is `alpha`, all four
   platforms are listed, and each `sha256` matches its line in `SHA256SUMS`.

   If more than a day or two has passed since the build, renew the validity
   window (the 14 days count from `issued_at`; hashes are not touched):

   ```powershell
   node ..\.github\scripts\manifest-body.mjs --restamp manifest-body.json --days 14
   ```

   **Do not re-run the workflow after this point.** A re-run replaces the
   draft's assets, and the hashes you are about to sign would no longer match.

4. **Sign offline.** Copy only `manifest-body.json` to the signing machine
   (USB stick). The key never comes to the online PC. There:

   ```powershell
   updater sign-manifest --key E:\mvs-keys\root.key --body manifest-body.json --out manifest.json
   updater verify --manifest manifest.json --root root.json
   ```

   `sign-manifest` prints the body before signing; check it again.
   Optionally also check one artifact:
   `updater verify --manifest manifest.json --root root.json --artifact mvs-0.1.0-windows-x86_64.zip --platform windows-x86_64`.

5. **Attach and publish.** Copy `manifest.json` back, then:

   ```powershell
   gh release upload v0.1.0 manifest.json --repo moranmoa/MVP
   gh release edit v0.1.0 --draft=false --repo moranmoa/MVP
   ```

   It stays marked as a pre-release. Send the link to testers.

## Keeping a release alive

A manifest expires (at most 30 days, 14 by default). Clients refuse it after
that, which stops an attacker freezing users on an old release. Before it
runs out, re-sign the same release without a new build: download
`manifest-body.json`, `--restamp` it, sign it offline (step 4), then
`gh release upload v0.1.0 manifest.json --clobber`.

To pull a bad version, put it in the next body's `revoked` list (edit
`manifest-body.json` before signing). A manifest cannot revoke its own
version.

## Later: 2 of 3 keys

The first key holder signs as above. Each further holder adds their
signature to the same file on their own offline machine:

```powershell
updater sign-manifest --key F:\mvs-keys\root2.key --manifest manifest.json --expect-version 0.1.0 --out manifest.json
```

(`--expect-version` comes with PR #8. On the older CLI, leave it out.)
