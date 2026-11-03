# B8: in-app update, design note

Status: **proposal**. File split pending agreement with dev-voice-shell
(`desktop/`) and devops (`release.yml`). core/api changes go through the
tech lead (owner: dev-identity). Security review: sec-lead.

Spec: `docs/spec.md` "עדכונים", which requires:
- signed updates with automatic rollback;
- no skipping and no switching updates off;
- install on the next start, or on "restart to update";
- never during a call;
- "what's new" after the update;
- stable/beta channels;
- a minimal update path that never changes between versions.

In this note, "verified" always means checked by the `updater` crate: the
signed chain, the policy and the hash, exactly as in `SECURITY.md`.

## 1. What already exists (core/updater, merged in #1/#8)

| Piece | Gives us |
| --- | --- |
| `TrustRoot` + `update_chain` | baked root, rotation, fork alarm |
| `SignedManifest::verify` | signatures, expiry, issued-in-future, size limits |
| `ClientPolicy` | no downgrade/replay/freeze, revocations, freshness |
| `store_artifact` / `open_verified` | hash while downloading, rename only on match, re-hash before use |
| `InstallState` | stage → trial boots → healthy/commit, rollback, `failed` list, platform binding |

B8 is integration: where files live, who runs what, and the transport.

## 2. Install layout: side-by-side versions plus a stable launcher

```
<root>/                         Windows: %LOCALAPPDATA%\Programs\MVS (per-user NSIS)
  mvs-launcher[.exe]            STABLE: shortcuts and mvs:// point here
  update/
    state.json                  InstallState      (launcher + core, under a lock)
    policy.json                 ClientPolicy      (core)
    root.json                   newest verified TrustRoot (core)
    lock                        exclusive-create lock file
    downloads/                  temp files from store_artifact
  versions/
    0.3.0/  MVS[.exe], resources/app.asar, resources/mvs-core[.exe], ...
    0.4.0/  ...
```

- **The launcher is the "minimal update path that does not change between
  versions".** It is a small Rust binary (`core/updater/src/bin/launcher.rs`)
  with no Electron, no network and no UI. Updates never replace it; only a
  full installer does. So a broken app version can never break the way back.
- **An update unpacks into a new folder. Nothing that is running is ever
  overwritten**, so Windows file locks don't matter, and rollback is just
  starting the other folder.
- Keep `current` and `previous` (the rollback target). The launcher deletes
  every other folder after a commit.

## 3. Who does what

```
 mvs-launcher (stable)          versions/<v>/MVS (Electron main)      mvs-core (sidecar)
 ─────────────────────          ────────────────────────────────      ──────────────────
 lock, load state.json
 begin_boot():
   Normal(v) / Trial(v, n)  ──► spawn versions/<v>/MVS
   RolledBack(prev)              --mvs-update-root=<root> argv...  ──► --update-root <root>
 exit (no waiting)                                                     every 6 h + at start:
                                 did-finish-load + core ping ok          checkForUpdate
                                 ──► markHealthy (core-only) ──────────► report_healthy()
                                                                         downloadUpdate: fetch,
                                                                         verify, unpack to
                                                                         versions/<v>.partial →
                                                                         rename, stage()
                                                          ◄──────────── event update.ready
 UI "restart to update" ──► restartToUpdate ──► applyUpdate (refused in a call)
                                 app.relaunch(<root>/mvs-launcher); quit
```

- **Rollback after 3 failed starts.** The launcher creates `InstallState`
  with `max_attempts = 3`. A trial version that starts 3 times without a
  `markHealthy` is marked `failed`, and the launcher starts `previous`. It
  never starts a revoked version. Nothing can mark it healthy but the
  running app.
- **Healthy** means the UI loaded *and* the core answered a request. A
  broken core binary, or UI bundle, therefore counts as a failed start.
- **Next start installs anyway.** A staged version is picked by
  `begin_boot` on the next launch, whether or not the user pressed the
  button (spec: no skipping).
- **Never during a call.** `applyUpdate` returns `in-call`, and
  `UpdateInfo.waitingForCall = true`, while a voice call or screen share is
  active. Voice is phase 2, so today this is always false. The hook exists
  now so voice cannot forget it.
- **Check cadence.** At core start, then every 6 h ± 30 min of jitter.
  Freshness is reported with `ClientPolicy::freshness`.

## 4. Transport: `UpdateSource` trait, HTTPS now, P2P later

```rust
pub trait UpdateSource {
    /// Signed manifest for a channel, at most MAX_MANIFEST_BYTES.
    fn manifest(&self, channel: &str) -> Result<Vec<u8>, Error>;
    /// Signed root rotations, newest last; capped in count and bytes (GAP-12).
    fn root_updates(&self) -> Result<Vec<Vec<u8>>, Error>;
    /// Streams the artifact into `out`, stopping after `max_len` bytes.
    fn artifact(&self, version: Version, platform: &str, max_len: u64, out: &mut dyn Write) -> Result<(), Error>;
}
```

- `HttpsSource`: GitHub release assets on a fixed per-channel URL, with the
  base URL baked in at build time (asked of devops). Client: `ureq` with
  rustls, no cookies, `https://` only, redirects only to
  `*.githubusercontent.com`, timeouts, and a byte cap on every body.
  - TLS is for privacy, not integrity: integrity is the signature and the hash.
- `P2pSource` plus the project seeder come after the relay is live (T4),
  behind the same trait. It verifies the hash before forwarding, which is
  already a rule.
- **GAP-12** lands here: `root_updates` is capped at 16 files × 64 KiB per
  fetch, before `update_chain` sees them.

## 5. Unpacking safely

The payload is `mvs-<ver>-<platform>.zip` (Windows) or `.tar.gz` (Linux). It
is verified by hash first, then unpacked by a new `unpack` module into
`versions/<v>.partial`.

- Entries must be relative paths with no `..`, no absolute paths, no drive
  letters, no NUL, and no Windows-reserved names.
- No symlinks, hardlinks or device files. Exception: on Linux, symlinks that
  stay inside the folder (Electron needs `.so` symlinks); they are resolved
  and checked.
- Caps: total unpacked size ≤ 4 × the artifact size and ≤ 2 GiB, and
  ≤ 20,000 entries. Executable bits come from the archive on Unix only.
- `.partial` is renamed to `<v>` only after the whole tree is written and
  fsynced. Then `InstallState::stage` runs.
- The required entry (`MVS.exe` / `mvs`) must exist, or the stage fails.

## 6. "What's new" from signed notes

- `ManifestBody` gains `#[serde(default)] notes: BTreeMap<String, String>`:
  a language tag (`he`, `en`, `[a-z]{2,3}(-[A-Z]{2})?`) mapped to plain
  text, at most 4 KiB per language and 8 languages.
  - It is part of the signed body, so it carries the same trust as the
    version.
  - Because of `deny_unknown_fields`, this must land **before** the first
    release that ships an updater. Today nothing does.
- The core keeps the notes of the staged version. After the first healthy
  start of a new version, it emits `update.installed { version, notes }`
  once, in the user's language, falling back to `en` and then `he`.
- **The UI renders the notes as text only**: no HTML, no Markdown links.
  This is a dev-ui task, and it needs a new event in `ui/src/core/api.ts`.
- `manifest-body.mjs` gets `--notes-dir` (one `<lang>.txt` per language),
  sent to devops as a patch.

## 7. Platforms

| Platform | Mechanism |
| --- | --- |
| Windows (NSIS per-user) | this design; first |
| Linux AppImage / tar.gz | this design; the launcher sits next to `versions/` |
| Linux deb/rpm | updater **off** (spec exception); "version too old" comes from the protocol |
| macOS | later: a bundle with the launcher as `CFBundleExecutable` and the app in `Contents/Versions/`; unsigned for the alpha, re-check with D-sign |

## 8. Things other owners must decide or do

1. **dev-voice-shell (`desktop/`):**
   - Agree on the new file `desktop/src/update.js` (mine).
   - Three hooks in `main.js`.
   - An extra-args option in `sidecar.js`.
   - electron-builder/NSIS: the launcher at the root and the app under
     `versions/<v>/`; shortcuts and `mvs://` go to the launcher.
2. **dev-identity via the tech lead (`core/api`):**
   - Core-only commands `checkForUpdate`, `downloadUpdate`, `applyUpdate`,
     `markHealthy`.
   - Events `update.ready` and `update.installed`.
   - The `--update-root` argument and the 6-hour timer.
   - I'll provide an `updater::client::Updater` facade so the core/api part
     is thin glue.
   - **Data compatibility:** a rollback runs the previous version on data
     the new one may have migrated. Rule: schema changes must be readable by
     the previous version, or the core snapshots the DB before the first
     migration and restores it on rollback. Needs dev-identity's decision.
3. **devops (`release.yml`):** the fixed manifest URL per channel;
   `mvs-launcher` in `CORE_BINS`; the payload is the unpacked app folder;
   `--notes-dir`.
4. **dev-ui:** render `update.installed` as plain text; the existing "update
   ready, restart" button uses `update.ready` / `restartToUpdate`.
5. **Channel names:** the manifest uses `alpha`, the UI uses stable/beta.
   During the closed alpha, UI `stable` maps to `alpha`.

## 9. PR plan (small, each reviewed by the tech lead and sec-lead)

| PR | Content | Files |
| --- | --- | --- |
| B8-0 | this note | `core/updater/docs/` |
| B8-1 | `notes` in the manifest (+ CLI `--notes`), GAP-12 caps | core/updater |
| B8-2 | `unpack` (zip/tar.gz) + layout/state paths with lock | core/updater |
| B8-3 | `mvs-launcher` bin (begin_boot, spawn, argv forwarding, gc) | core/updater |
| B8-4 | `UpdateSource` + `HttpsSource` + `Updater` facade | core/updater (+ `deny.toml` if needed) |
| B8-5 | core/api glue | core/api (dev-identity reviews) |
| B8-6 | `desktop/src/update.js` + hooks | desktop (dev-voice-shell reviews) |
| B8-7 | packaging + release wiring | devops |
| B8-8 | e2e: install N, update to N+1, break N+1 → 3 starts → back to N | qa + me |

## 10. Risks

- **The launcher is a new unsigned exe.** Until D-sign, SmartScreen warns
  on first install only; updates don't run through SmartScreen.
- **Same-user malware can edit `state.json` or `versions/`.** This is out of
  scope, as in `SECURITY.md`.
- **Antivirus may hold a lock on a fresh `versions/<v>`.** The rename is
  retried with backoff, and the stage simply waits for the next check.
