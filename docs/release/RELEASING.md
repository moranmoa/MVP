# Releasing (closed alpha)

From a tag to a published, signed pre-release. CI builds and hashes, but it
never sees the update root key. **Signing is always a manual, offline step
by the owner.**

```
tag vX.Y.Z ──> Release workflow ──> draft pre-release (unsigned)
                                     ├─ mvs-X.Y.Z-windows-x86_64-setup.exe   installer (NSIS, per user)
                                     ├─ mvs-X.Y.Z-linux-x86_64.AppImage|.deb installers
                                     ├─ mvs-X.Y.Z-<platform>.zip|.tar.gz     updater payloads
                                     ├─ mvs-ui-X.Y.Z.tar.gz                  built UI
                                     ├─ SHA256SUMS
                                     └─ manifest-body.json                   UNSIGNED
owner, offline: updater sign-manifest ──> manifest.json ──> upload ──> publish
```

Testers start from [ALPHA-QUICKSTART.md](ALPHA-QUICKSTART.md) (Hebrew), which
the release notes link to.

## What the workflow does

[`.github/workflows/release.yml`](../../.github/workflows/release.yml), on a
`v*` tag:

1. **check**: the tag must be `vMAJOR.MINOR.PATCH`. core/updater accepts
   no `-alpha.1` style suffixes; the channel says "alpha" instead.
2. **ui**: licence check, `npm ci --ignore-scripts`, `npm run build` →
   `mvs-ui-X.Y.Z.tar.gz`.
3. **desktop** (matrix: `windows-x86_64` on `windows-latest`, `linux-x86_64`
   on `ubuntu-22.04` for an old glibc):
   - builds the core sidecar `mvs-core` (`cargo build --release --locked -p api`);
   - unpacks the ui build;
   - writes `desktop/build/mvs-config.json` with the relay address from the
     repository variable `MVS_RELAY_ADDR` (below);
   - runs electron-builder in `desktop/` (version from the tag):
     - Windows: the NSIS installer plus a `.zip`;
     - Linux: AppImage, `.deb`, plus a `.tar.gz`.

   The `.zip`/`.tar.gz` are the updater payloads, named
   `mvs-X.Y.Z-<platform>.zip|.tar.gz`, which is how the manifest body finds
   them. Windows is built natively, because electron-builder's NSIS step
   needs wine anywhere else. Everything is **unsigned** (no code-signing
   certificate yet; Windows SmartScreen warns, and the quickstart explains it).
4. **publish**, with
   [`collect-release.sh`](../../.github/scripts/collect-release.sh):
   - checks that the run produced exactly the artifacts `ui` and
     `desktop-<platform>` (one per entry in `PLATFORMS`, which must match the
     desktop matrix), each holding exactly its expected files, and stops on
     anything missing or extra;
   - writes `SHA256SUMS` over those files;
   - [`manifest-body.mjs`](../../.github/scripts/manifest-body.mjs) writes
     `manifest-body.json`: channel `alpha`, the version, one `{sha256, size}`
     entry per platform in `PLATFORMS` (any other platform is an error),
     valid for 14 days from the build;
   - creates the draft pre-release and uploads that explicit list.

   If a release for the tag is already published, it stops, so a published
   release's assets are never replaced.

**Dry run on pull requests.** A PR that touches the release workflow, its
scripts or `desktop/`'s package and build config runs the same build jobs as
version `0.0.0`. A read-only `dry-run` job then runs `collect-release.sh` and
lists what a tag would publish. Nothing is uploaded. Check that it's green
before pushing a tag.

**Who can write (GAP-20).** The build jobs (ui, desktop) run third-party
code: crates, npm packages, Electron and electron-builder, and the toolchain
action. So they get a read-only token and **no caches** (no rust-cache, no
npm cache), which means a cache poisoned by a PR build can't end up in a
release binary. They hand their files to `publish` as workflow artifacts
(kept 7 days). `publish` is the only job with `contents: write`, and it runs
only on a tag. It runs only GitHub's own actions, the runner's preinstalled
`gh` and `node`, and our own dependency-free scripts.

macOS isn't built yet; it returns to `PLATFORMS` and the matrix when there's
a signed, notarisable package.

## One-time setup (owner)

Covered in `core/updater/SECURITY.md` (PR #8). In short:

- On the **offline signing machine**, build `updater` yourself from a
  reviewed commit (`cargo build --release -p updater` in `core/`). Don't use
  a binary downloaded from a release: the tool that signs should not come
  from the pipeline it vouches for.
- `updater keygen --out E:\mvs-keys` (removable or encrypted drive, never a
  synced folder), then `updater make-root --threshold 1 --pub E:\mvs-keys\root.pub --out root.json`.
  `root.json` (public) is what the app embeds.

### Relay address for the build (`MVS_RELAY_ADDR`)

The app ships with the relay it connects to. The address contains your
domain, so it lives in a **repository variable**, never in git:

1. Settings → Secrets and variables → Actions → **Variables** → New
   repository variable.
2. Name `MVS_RELAY_ADDR`, value the relay's native address with its peer ID:
   `/dns4/<relay-domain>/tcp/4001/p2p/<relay peer id>`. The peer ID is in
   `docker compose logs relay` ("relay node started peer_id=..."); see
   `infra/README.md`.

Or with `gh` (PowerShell):

```powershell
gh variable set MVS_RELAY_ADDR --repo moranmoa/MVP --body "/dns4/<relay-domain>/tcp/4001/p2p/<relay peer id>"
```

Without the variable the build still succeeds, with a warning, but the app
runs offline and testers can't reach each other. The value ends up inside the
released app, so anyone with the download can read it. That's expected: the
rule only keeps it out of the repository's history.

### Protect release tags

Pushing a `v*` tag starts the release workflow, so only the owner should be
able to create, move or delete one. The repo has no ruleset yet. Create one
(Settings → Rules → Rulesets → New tag ruleset):

- Name `release tags`, enforcement **Active**.
- Target tags: include by pattern `v*`.
- Rules: **Restrict creations**, **Restrict updates**, **Restrict deletions**.
- Bypass list: **Repository admin** (that's you) → Always.

Or in one command (PowerShell, logged in to `gh` as the owner):

```powershell
@'
{"name":"release tags","target":"tag","enforcement":"active",
 "conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},
 "rules":[{"type":"creation"},{"type":"update"},{"type":"deletion"}],
 "bypass_actors":[{"actor_id":5,"actor_type":"RepositoryRole","bypass_mode":"always"}]}
'@ | gh api repos/moranmoa/MVP/rulesets --method POST --input -
```

Check: `gh api repos/moranmoa/MVP/rulesets` lists `release tags`.

What this does and doesn't cover:

- It blocks anyone without the admin role, which matters as soon as there
  are other collaborators.
- The agent sessions currently push with **your** GitHub login, which is an
  admin, so the ruleset does not stop them. Their instructions say not to tag.
  The technical control is a separate account or fine-grained token for
  the agents, without admin and without permission to bypass this ruleset.
- Either way, a rogue tag only produces an unsigned **draft**. Nothing can be
  installed through the updater until you sign its manifest offline, and the
  signing step shows you the version and hashes first.

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

6. **Point the updater at it.** Installed apps look for the signed manifest
   at a fixed address per channel: the release with tag `update-alpha`
   (`https://github.com/moranmoa/MVP/releases/download/update-alpha/manifest.json`,
   with the root-rotation files `root-*.json` next to it). Create it once,
   then replace its manifest on every release:

   ```powershell
   # once:
   gh release create update-alpha --repo moranmoa/MVP --prerelease --title "Update channel: alpha" --notes "Signed update manifest for the alpha channel. Not an installer."
   # every release (and every keep-alive re-sign):
   gh release upload update-alpha manifest.json --clobber --repo moranmoa/MVP
   ```

   The updater payload (`mvs-X.Y.Z-<platform>.zip|.tar.gz`) is the unpacked
   app folder, without the installer and without `mvs-launcher` (B8).

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
