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
                                     └─ manifest-body-{beta,alpha}.json      UNSIGNED, one per channel
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
     `manifest-body-beta.json` and `manifest-body-alpha.json`: the channel, the version, the CHANGELOG notes, one `{sha256, size}`
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
2. Name `MVS_RELAY_ADDR`, value the relay's **wss** address with its peer ID:
   `/dns4/<relay-domain>/tcp/443/wss/p2p/<relay peer id>`. The peer ID is in
   `docker compose logs relay` ("relay node started peer_id=..."); see
   `infra/README.md`. The address **must end in `/p2p/<relay peer id>`**: the build refuses it otherwise, because without the ID the app would trust whatever answers at that address.

   Why wss and not `/tcp/4001`: the domain's DNS record is proxied by
   Cloudflare, which forwards only HTTP(S) and WebSocket on 443, so port 4001
   isn't reachable through it. wss also keeps the home IP hidden behind
   Cloudflare. Never use an `/ip4/...` address here: it would put your home
   IP into a public download.

Or with `gh` (PowerShell):

```powershell
gh variable set MVS_RELAY_ADDR --repo moranmoa/MVP --body "/dns4/<relay-domain>/tcp/443/wss/p2p/<relay peer id>"
```

`mvs-core` dials wss through libp2p's `websocket` transport in `core/sync`
(#114).

### First pre-release: data may be reset

The record format is v2 from this release on, and it may still change
before the beta. The release notes and the quickstart tell testers that a
later build may reset servers and messages, and that their identity comes
back from the 12 words. If a later release does reset data, say so in its
release notes **before** testers install it.

Without the variable the build still succeeds, with a warning, but the app
runs offline and testers can't reach each other. Only tag builds read the
variable; pull-request dry runs always build with an empty config (the
repository is public, and so are their artifacts). The value does end up
inside the released app, so anyone with the download can read it; the owner
accepted that (decision 2026-11-03). The rule is that it never goes into the
repository or into pull-request CI artifacts.

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

0. **Write the release notes** first: a `## [X.Y.Z] - YYYY-MM-DD` section in
   [`CHANGELOG.md`](../../CHANGELOG.md), in Hebrew and English (Fixed / New
   / Known), merged to `main`. The draft's release notes are that section
   plus the install and verify text, and **a tag without a section fails**
   at "check tag", before anything builds.

1. **Tag and push** the reviewed commit on `main`:

   ```powershell
   git switch main; git pull
   git tag -a v0.1.0 -m "MVS 0.1.0 closed alpha"
   git push origin v0.1.0
   ```

2. **Wait for the Release workflow** (Actions tab). Its summary lists the
   checksums. The release is a draft: only repo collaborators can see it.

3. **Sign and publish, in one command** (steps 3–6 below, done for you):

   ```powershell
   # Dry run first: every check and the build, no key, nothing signed or uploaded.
   .\.github\scripts\publish-release.ps1 -Tag v0.1.0 -WhatIf
   # Then for real. Every release goes to beta; add -Channel alpha for stable.
   .\.github\scripts\publish-release.ps1 -Tag v0.1.0 -Key E:\mvs-keys\root.key
   ```

   [`publish-release.ps1`](../../.github/scripts/publish-release.ps1) does this:
   - It downloads the draft (or pre-release) and checks every file against
     `SHA256SUMS`, and the channel's manifest body against both: version,
     channel, and each payload's hash and size.
   - It restamps the body when it is older than 2 days.
   - It builds `updater` from the tag's own source. The tag must be on
     `origin/main`, and the source comes from `git archive` of the tag. It
     never uses a downloaded binary.
   - It shows a summary (version, channel, hashes, expiry, the first lines of
     the notes) and asks you to **type the version** to go on.
   - It signs, then runs `updater verify` against the tag's
     `core/updater/trust/root.json`, each payload included.
   - It publishes the release as a pre-release, then runs
     `publish-update.ps1` for the channel (step 6).
   - Any failed check stops it with a message, and nothing after that step
     runs.

   **The key.** It comes from `-Key` or `$env:MVS_ROOT_KEY`, and the script
   never prints, copies or uploads it. It refuses a key inside the
   repository, and refuses to run in CI. With this script the key is used on
   the online PC: plug the key drive in only for the run, and never keep the
   key on a synced folder. Never put the path in the repo, a script or
   `$PROFILE`. If you would rather keep the key on an offline machine, do
   steps 3–6 by hand as below.

   After a root rotation, add `-Roots E:\mvs-keys\root-2.json`.

   The manual steps, for reference or for offline signing:

   **Download and check** the draft's files:

   ```powershell
   gh release download v0.1.0 --repo moranmoa/MVP --dir release-v0.1.0
   cd release-v0.1.0
   Get-Content SHA256SUMS | ForEach-Object {
     $sum, $file = $_ -split '\s+', 2
     if ((Get-FileHash $file -Algorithm SHA256).Hash.ToLower() -ne $sum) { "MISMATCH $file" } else { "ok $file" }
   }
   Get-Content manifest-body-beta.json
   ```

   Read the body: `version` matches the tag, `channel` is `alpha`, all four
   platforms are listed, and each `sha256` matches its line in `SHA256SUMS`.

   If more than a day or two has passed since the build, renew the validity
   window (the 14 days count from `issued_at`; hashes are not touched):

   ```powershell
   node ..\.github\scripts\manifest-body.mjs --restamp manifest-body-beta.json --days 14
   ```

   **Do not re-run the workflow after this point.** A re-run replaces the
   draft's assets, and the hashes you are about to sign would no longer match.

4. **Sign offline, one manifest per channel.** The draft carries
   `manifest-body-beta.json` and `manifest-body-alpha.json`. Both have the
   same artifacts and the "what's new" notes from `CHANGELOG.md` (Hebrew +
   English). Every release goes to **beta**; sign **alpha** too only when the
   release is promoted to stable. Copy only the bodies to the signing machine
   (USB stick). The key never comes to the online PC. There:

   ```powershell
   updater sign-manifest --key E:\mvs-keys\root.key --body manifest-body-beta.json --out manifest-beta.json
   updater verify --manifest manifest-beta.json --root root.json
   # stable only:
   updater sign-manifest --key E:\mvs-keys\root.key --body manifest-body-alpha.json --out manifest-alpha.json
   ```

   `sign-manifest` prints the body before signing; check the version and the
   channel.

5. **Publish the release** (it stays a pre-release). This only publishes
   the installers for people downloading by hand. The app **fetches its
   update payload from `update-<channel>`, not from here**, so step 6 is
   what makes the update reach installed apps:

   ```powershell
   gh release edit v0.1.0 --draft=false --repo moranmoa/MVP
   ```

6. **Publish the update** to the channel releases the app polls
   (`https://github.com/moranmoa/MVP/releases/download/update-<channel>/manifest.json`).
   The script first runs `updater verify` (signature, root chain, expiry;
   build `updater` from a reviewed commit, or pass `-Updater <path>`). It then checks that the signed body's channel and version match, and
   that the update zip in the release matches its hash and size. It then
   uploads `manifest.json`, the zip and any root rotations to `update-<channel>`,
   which it creates once as a pre-release:

   ```powershell
   .\.github\scripts\publish-update.ps1 -Tag v0.1.0 -Channel beta -Manifest .\manifest-beta.json -Root .\root.json
   # stable only:
   .\.github\scripts\publish-update.ps1 -Tag v0.1.0 -Channel alpha -Manifest .\manifest-alpha.json -Root .\root.json
   # after a root rotation, add (to every channel, oldest first, never removed):
   #   -Roots E:\mvs-keys\root-2.json
   ```

   The payload (`mvs-X.Y.Z-<platform>.zip`) is the unpacked app folder,
   without the installer and without `mvs-launcher` (B8). In-app update is
   **Windows-only for now**: the updater fetches and unpacks only `.zip`, so
   the manifests list only `windows-*`. The Linux `.tar.gz` stays a plain
   release file.

## Keeping a release alive

Electron bumps follow [ELECTRON-UPGRADE.md](ELECTRON-UPGRADE.md): the Windows
desktop e2e must be re-run green on the bump itself.

A manifest expires (at most 30 days, 14 by default). Clients refuse it after
that, which stops an attacker freezing users on an old release. Before it
runs out, re-sign the same release without a new build: download
the channel's `manifest-body-<channel>.json`, `--restamp` it, sign it offline (step 4), then
`gh release upload v0.1.0 manifest.json --clobber`.

To pull a bad version, put it in the next body's `revoked` list (edit
`manifest-body-beta.json` before signing). A manifest cannot revoke its own
version.

## Later: 2 of 3 keys

The first key holder signs as above. Each further holder adds their
signature to the same file on their own offline machine:

```powershell
updater sign-manifest --key F:\mvs-keys\root2.key --manifest manifest.json --expect-version 0.1.0 --out manifest.json
```

(`--expect-version` comes with PR #8. On the older CLI, leave it out.)
