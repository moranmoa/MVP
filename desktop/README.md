# desktop – Electron shell (B1)

Decision 7: Electron (see spikes/RESULTS.md). This folder is the desktop app
around the `ui/` build. B1-1 (this step) shows the UI with the mock core; the
core sidecar and the IPC bridge come in B1-2, packaging in B1-3.

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

On a machine with Node: `npm ci && npm start` (set `MVS_DEMO=1` for the demo data).

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

The microphone permission is decided with voice (phase 2). There is no
preload yet; B1-2 adds one narrow bridge (`invoke` + `onEvent`) with an
allow-list of core commands checked in the main process.
