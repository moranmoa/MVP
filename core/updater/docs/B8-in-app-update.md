# B8: in-app update, design note

Status: **approved** by the tech lead and the security lead, with the
amendments in §0 (PR #101). B8-1 is #108. core/api changes go through the
tech lead (owner: dev-identity); `desktop/` through dev-voice-shell;
`release.yml` and `manifest-body.mjs` through devops.

## 0. Decisions and amendments (review of #101)

These override anything below that disagrees.

**Tech lead:**
- **Data on rollback.** Before any schema migration, `mvs-core` copies
  `core.db` to `core.db.pre-<schema>`, fsynced, with the same owner-only
  ACL/0600 as `core.db`. Only the last snapshot is kept, and it is deleted
  once the new version commits (`markHealthy`). On rollback, the **launcher**
  restores it before the old version starts. Sync re-fetches whatever
  arrived after the update. The keychain and `node.key` are never touched.
- **core/api.** The surface in §8 is approved. Exact JSON shapes go into
  `CONFORMANCE.md` in B8-2, and dev-identity builds it after M1.
- **The launcher** is the only binary an update never replaces; only the
  installer updates it. It is tiny, has no network access, and verifies
  nothing itself: it picks a version and counts boots.

**Security lead:**
- **A1. Paths.** The launcher never takes a path from `state.json`. It
  parses a `Version` and builds `versions/<v>/MVS[.exe]` itself; anything
  else counts as a failed start. It spawns with no shell and forwards argv
  **verbatim**, including the `--` before an `mvs://` link (#78). The only
  thing it adds is `--mvs-update-root=<root>`.
- **A2. Allow-list.** `checkForUpdate`, `downloadUpdate`, `applyUpdate`
  and `markHealthy` are main/core-only and are **not** in
  `desktop/src/commands.js`. The renderer keeps only `restartToUpdate`. A
  broken UI must never be able to declare itself healthy.
- **A3. Unpacking** (adds to §5):
  - no `:` in names (NTFS streams), no trailing dot or space;
  - no entries that collide case-insensitively, on every platform;
  - files are created with `create_new` and `O_NOFOLLOW`; symlinks (Linux
    only) are created **last**, after every regular file;
  - path length is capped (each component ≤ 255 bytes, the whole path
    ≤ 1024).
- **A4. Redirects.** Only exact GitHub asset hosts are allowed
  (`objects.githubusercontent.com`, `release-assets.githubusercontent.com`),
  never `*.githubusercontent.com`.
- **A5. Snapshot.** As the tech lead decided above: owner-only, deleted on
  commit, restored by the launcher's rollback path.
- **A6. State files.** `state.json`, `policy.json` and `root.json` are
  validated on load and can never lower the baseline. If the launcher
  cannot read `state.json`, it starts the version recorded as `previous` in
  `state.json.bak`, the last good copy, written before each change. If that
  also fails, it starts the **installer's version**, which is baked into the
  launcher at build time. It never starts "whatever is there".

**Devops (#103):**
- **Discovery.** Each channel has a fixed-tag release, `update-<channel>`
  (`update-alpha` now). It holds `manifest.json` and `root-*.json`, which
  the owner re-uploads after signing. Base URL:
  `https://github.com/moranmoa/MVP/releases/download/update-<channel>/`,
  baked in at build time.
- **Payload.** devops archives the `*-unpacked` folder itself: contents at
  the archive root, `mvs-launcher` explicitly excluded, same file name. The
  desktop job builds `mvs-launcher` into `desktop/.core/` next to
  `mvs-core`, and dev-voice-shell's packaging places it.
- **Channels.** UI `stable` maps to `alpha` during the closed alpha.
- **Notes.** `manifest-body.mjs --notes-dir` is #111 (after #108).

**dev-voice-shell (desktop/).** These replace the layout in §2:
- **Base install.** NSIS keeps installing the app the normal
  electron-builder way into `$INSTDIR`, and that copy is the **base**
  version. `mvs-launcher[.exe]` ships at `$INSTDIR/` via `build.extraFiles`.
  Updates unpack into `$INSTDIR/versions/<ver>/`. The launcher falls back
  to the base copy (`$INSTDIR/MVS.exe`) when no update is healthy; that is
  A6's "installer's version". The uninstaller already removes all of
  `$INSTDIR`. A `customInstall` macro in `installer.nsh` (dev-voice-shell)
  points the Start-menu and desktop shortcuts at the launcher.
- **The update root is derived, never trusted from argv.** In a packaged
  app, `update.js` derives the root from `process.execPath`: either
  `<root>/MVS[.exe]` (base) or `<root>/versions/<ver>/MVS[.exe]`, and
  `<root>/mvs-launcher[.exe]` must exist. `--mvs-update-root` from argv is
  accepted only if it equals the derived root; otherwise the app runs with
  updates off and logs a warning.
- **mvs://.** `update.js` gives main the launcher path, and main calls
  `setAsDefaultProtocolClient(scheme, launcherPath, ['--'])`. The launcher
  forwards everything after `--` verbatim and never parses switches from
  that tail.
- **Hooks.** The sidecar gets `extraArgs: ['--update-root', root]` (#121).
  In main, `restartToUpdate` goes to `update.js` instead of the core;
  `markHealthy` goes from main to the core only. The 3 hook lines come with
  `update.js`, reviewed by dev-voice-shell and sec-lead.
- **Fuses.** `OnlyLoadAppFromAsar` and asar integrity stay on for versioned
  copies too. The launcher sets no `NODE_OPTIONS` or
  `ELECTRON_RUN_AS_NODE`, and clears them if they are inherited.
- **An installer upgrade wipes `$INSTDIR`, including `versions/`.** So the
  state files (`state.json`, `.bak`, `policy.json`, `root.json`,
  `db-snapshot.json`, the lock) live **outside** `$INSTDIR`, in
  `%LOCALAPPDATA%\MVS\update\`. A recorded version whose folder is gone
  means "start the base". That is not a failed start and does not count
  toward rollback. When the launcher finds that the base version changed
  (the installer ran), it re-bases `InstallState` on it.
- **Shared `userData`.** The base copy and `versions/<v>` share `userData`
  (same app name), so the single instance and the `mvs://` hand-off work
  across versions. An older version must never open a newer `core.db`
  schema without the §0 snapshot restore: B8-3 gets a launcher test for
  this.
- **Shortcuts.** They are repointed only if `$INSTDIR\mvs-launcher.exe`
  exists (`IfFileExists`), so builds from before B8-3 keep working
  shortcuts.
- **Linux is deferred.** The AppImage is a single read-only file and the
  .deb installs to root-owned `/opt`. In-app updates are **Windows-only**
  in the closed alpha. Linux comes later, per-user under
  `~/.local/share/MVS/versions/`; until then Linux users update with the
  package or a new AppImage, and the protocol's "version too old" check
  covers them. This replaces the Linux rows in §7.

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
  base URL baked in at build time (`update-<channel>`, §0). Client: `ureq`
  with rustls, no cookies, `https://` only, redirects only to the exact
  hosts in A4, timeouts, and a byte cap on every body.
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
  letters, no NUL, and no Windows-reserved names. Plus the A3 rules: no
  `:`, no trailing dot or space, no case collisions, length caps, and
  symlinks created last.
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
   - **Data compatibility (decided, §0):** the core snapshots the DB before
     migrating, and the launcher restores it on rollback. The core records
     the snapshot in `update/db-snapshot.json`, as a version and a file
     name; the launcher only accepts a plain file name inside the core data
     folder. The launcher derives that folder the way Electron's
     `userData` does (`%APPDATA%\MVS\core`, `$XDG_CONFIG_HOME/MVS/core`),
     and the shell passes the same folder to the core.
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
