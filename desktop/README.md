# desktop – Electron shell (B1)

Decision 7: Electron (see spikes/RESULTS.md). This folder is the desktop app
around the `ui/` build. B1-1: window and navigation security. B1-2: the core
running as a sidecar process (`mvs-core`, core/api) and the one IPC bridge.
B1-3: packaging (fuses, installers, `mvs://`).

Node and Electron are not needed on the host. Everything runs in Docker.

## Commands

Build the UI first (the shell refuses to start without its CSP):

```bash
cd ../ui && docker compose run --rm ui npm run build
```

Unit tests, then the real app under Xvfb with the security checks:

```bash
docker compose run --rm test
```

On a machine with Node: `npm ci && npm start`. `MVS_CORE_PATH=<absolute path to mvs-core>`
runs the real core (otherwise core commands answer `core-unavailable`);
`MVS_DEMO=1` opens the mock demo.

## Security settings and where they are tested

| Setting | Where | Test |
| --- | --- | --- |
| `sandbox`, `contextIsolation`, no `nodeIntegration` (also in workers and subframes), `webSecurity`, no `webviewTag`, no insecure content | `src/security.js` `WEB_PREFERENCES`, `app.enableSandbox()` | unit + e2e (`getLastWebPreferences`, no `require`/`process` in the page) |
| UI served only from `app://mvs` (privileged standard + secure scheme), never `file://` | `src/main.js` `serveUi` | e2e |
| No path traversal out of `ui/dist` | `resolveAppPath` | unit (encoded `..`, backslashes, NUL, other hosts) + e2e (404) |
| CSP from `ui/csp.ts` (via the built `index.html`) sent as a header, plus `frame-ancestors 'none'`; start refused without a CSP or with a loose one | `cspFromIndexHtml`, `responseHeaders` | unit + e2e (header present, inline script blocked) |
| `nosniff`, COOP/CORP same-origin, `no-referrer` | `responseHeaders` | unit |
| No network requests except `app://mvs` (and `data:`/`blob:`) | `webRequest.onBeforeRequest` | e2e |
| `window.open` denied; https links go to the OS browser only after a confirm dialog; everything else (http, file, other schemes, credentials in the URL) dropped | `setWindowOpenHandler`, `isExternalWebUrl` | unit + e2e |
| Navigation and redirects off `app://mvs` blocked; `<webview>` attachment blocked | `will-navigate`, `will-redirect`, `will-attach-webview` | e2e |
| No permissions (microphone, notifications, devices, screen capture) | `setPermissionRequestHandler` / `CheckHandler`, `setDevicePermissionHandler`, `setDisplayMediaRequestHandler` | unit + e2e |
| Crash reporter on with uploads off | `crashReporter.start({ uploadToServer: false })` | – |
| No default menu and `devTools: false` in packaged builds (and no `devtools://` requests); `MVS_UI_DIR` and `MVS_DEMO` only in a checkout; the link dialog shows the parsed URL (punycode) | `main.js` | – (B1-3, packaged build) |

## Core bridge (B1-2)

| Rule | Where | Test |
| --- | --- | --- |
| The page gets only `window.mvs.invoke(cmd, args)` and `window.mvs.onEvent(cb)`, never `ipcRenderer` | `src/preload.js` | e2e |
| Main answers only our top frame on `app://mvs` (`event.senderFrame`) | `main.js` `serveCoreIpc` | e2e (indirect) |
| Allow-list = `CoreCommands` in ui/src/core/api.ts, same parameter names; core-only commands (`grantRole`, `revokeRole`, `syncNow`) unreachable; prototype names refused | `src/commands.js` | unit (parses api.ts) + e2e |
| Positional args mapped to named args; extra args, non-arrays and cycles refused; requests over 1 MiB never sent | `toRequest` | unit + e2e |
| `mvs-core` spawned by absolute path, `shell: false`, minimal env (no `NODE_OPTIONS`, `LD_PRELOAD`, ...), stdio only, `--data-dir <userData>/core` | `src/sidecar.js` | unit (spawn spy, env seen by the fake core) |
| stdout never logged (it carries the words); stderr logged, lines truncated | `sidecar.js` | unit |
| A line from the core over 16 MiB kills and restarts it; a crash fails pending requests (`core-restarted`) and restarts with backoff | `sidecar.js` | unit |
| Timeout per request (30 s; 120 s for key generation and OS auth) answers `timeout`, for the UI's busy state | `sidecar.js` | unit |
| Errors reach the page as `{ code, positions? }` only | `sanitizeError` | unit |
| Single instance; packaged builds quit on `--inspect`, `--remote-debugging-*` or `--js-flags` and have `devTools: false` | `main.js` | – (packaged, B1-3) |

Tests use `test/fake-core.js`, which speaks the sidecar's line protocol
(core/api README) and misbehaves on cue. The keychain stays in the core
process; the shell never touches keys (no `safeStorage`).

The microphone permission is decided with voice (phase 2).

## With the real core

Build `mvs-core` from `core/` into `desktop/.core/` (git-ignored), then run the
app against it with a real keychain (a session D-Bus and an unlocked
gnome-keyring in the container, test only):

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/../core":/w -v "$PWD/.core":/out -w /w rust:1.90-bookworm sh -c 'cargo build --release --locked --bin mvs-core && cp target/release/mvs-core /out/'
```

```bash
docker compose run --rm real-core
```

`test/real-core.test.mjs`: createIdentity → getRecoveryWords (same words) →
checkRecoveryWords → getRecoveryWords refused → createServer → listServers,
and the recovery words never appear in the main process's stdout or stderr.

## Packaging (B1-3)

`electron-builder` 26.17.0 with `mvs-core` (from `.core/`) and `ui/dist` in
`resources/`, the app code only in `app.asar`, and `build/fuses.js` as the
`afterPack` hook. The fuses:

| Fuse | Value |
| --- | --- |
| RunAsNode | off |
| EnableNodeOptionsEnvironmentVariable | off |
| EnableNodeCliInspectArguments | off |
| OnlyLoadAppFromAsar | on |
| EnableEmbeddedAsarIntegrityValidation | on (enforced on Windows and macOS) |
| EnableCookieEncryption | on |
| GrantFileProtocolExtraPrivileges | off |

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/..":/repo -v desktop_node_modules:/repo/desktop/node_modules -w /repo/desktop node:22.23.4-bookworm sh -c "npm ci && npx electron-builder --linux dir && node --test test/fuses.test.mjs"
```

`publish: null`: electron-builder never uploads anything by itself; releases go
through our own release workflow and signed updater.

ASAR integrity is enforced only on Windows and macOS. On Linux the fuse is set
but Electron does not check the archive, so there it protects nothing.

`test/fuses.test.mjs` reads the fuses back from the packaged binary, checks
the asar-only layout, and runs the binary with `ELECTRON_RUN_AS_NODE=1` and a
hostile `NODE_OPTIONS` to see both ignored. The core's data folder
(`<userData>/core`) is created `0700`.

## Invite links (mvs://)

The installed app registers `mvs://` for the current user (never in a
checkout), with `--` before the link on Windows (`"mvs.exe" -- "%1"`) and
Linux (`Exec=mvs -- %U`), so a link can never become a switch. A link arrives
on the first launch's command line, from a second launch
(`requestSingleInstanceLock` + `second-instance`; the second process quits),
or as macOS `open-url`. `src/deeplink.js` accepts only the exact core format
`mvs://join/<64 hex>/<64 hex>` (canonicalised).

**Main never joins by itself** (a click on a link must not reveal who we are
to someone's server). It sends the page a `link.open` event, `{ link }` with
the canonical link only, through `window.mvs.onEvent`; if the page has not
subscribed yet, the preload keeps the latest one for the first subscriber.
The UI shows the join screen pre-filled and calls `joinServer` only when the
user presses Join.

Tests: `test/deeplink.test.js` (unit, hostile links) and
`test/deeplink.e2e.test.mjs` (first launch with a late subscriber, second
instance, hostile links reach nothing, no `joinServer` without the UI).

## Windows uninstall

`build/installer.nsh`: the uninstaller asks whether to delete the local data
(`%APPDATA%\MVS`); the default is keep. `/S` keeps it, `/S /DELETEDATA` deletes
it, an update never deletes it. The identity key in Windows Credential
Manager is never touched. The `mvs://` registration is always removed.

```powershell
powershell -ExecutionPolicy Bypass -File test\nsis-uninstall.ps1 -Installer "dist\MVS Setup 0.1.0.exe"
```

Build the installer with `electronuserland/builder:wine` (see
spikes/RESULTS.md) after cross-compiling `mvs-core.exe` into `.core/`
(`cargo build --release --locked --bin mvs-core --target x86_64-pc-windows-gnu`).

Still to come: the release.yml plug-in point with devops.

## In-app update hooks (B8)

`mvs-launcher.exe` (core/updater, built into `.core/` by the release job)
ships at the install root via `build.win.extraFiles`. When it is present,
`customInstall` in `build/installer.nsh` points the Start-menu and desktop
shortcuts at it. A build without it keeps shortcuts to `MVS.exe`. Design:
core/updater/docs/B8-in-app-update.md §0 (`update.js` and the main.js hooks
come with dev-updater's B8-6).
