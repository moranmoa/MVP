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
| No default menu (so no DevTools) in packaged builds; `MVS_UI_DIR` only in a checkout | `main.js` | – (B1-3, packaged build) |

## Core bridge (B1-2)

| Rule | Where | Test |
| --- | --- | --- |
| The page gets only  and , never  |  | e2e |
| Main answers only our top frame on  () |   | e2e (indirect) |
| Allow-list =  in ui/src/core/api.ts, same parameter names; core-only commands (, , ) unreachable; prototype names refused |  | unit (parses api.ts) + e2e |
| Positional args mapped to named args; extra args, non-arrays, cycles refused; requests over 1 MiB never sent |  | unit + e2e |
|  spawned by absolute path, , minimal env (no , , ...), stdio only,  |  | unit (spawn spy, env seen by the fake core) |
| stdout never logged (words); stderr logged, lines truncated |  | unit |
| Lines from the core over 16 MiB kill and restart it; crashes fail pending requests () and restart with backoff |  | unit |
| Timeout per request (30 s; 120 s for key generation and OS auth) answers , for the UI's busy state |  | unit |
| Errors reach the page as  only |  | unit |
| Single instance; packaged builds quit on  /  /  and have  |  | – (packaged, B1-3) |

Tests use , which speaks the sidecar's line protocol
(core/api README) and misbehaves on cue. The keychain stays in the core
process; the shell never touches keys (no ).

The microphone permission is decided with voice (phase 2).
