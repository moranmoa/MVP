# Electron upgrade checklist

Every Electron bump in `desktop/package.json` (patch, minor or major,
Dependabot's included) goes through this list before merge. Chromium's
permission and capture paths change between versions. The desktop e2e is
what proves our refusals still hold.

## Before merge

1. **Version age.** The `electron version age` CI job is green: we are no
   more than one major behind, and not on a patch that was superseded more
   than 8 weeks ago.
2. **Lockfile licences.** `licence policy` passes on `desktop/package-lock.json`.
   A new non-permissive package needs a pinned, reviewed entry in
   `.github/scripts/npm-licenses.mjs` EXCEPTIONS. Don't widen the allowed lists.
3. **Windows desktop e2e re-run.** `desktop e2e (Windows)` must be green on
   the bump's own head, not on an older commit. These cases must pass:
   - `permissions are refused, except the microphone` (only our own page gets the mic)
   - `legacy desktop capture never gives a stream`
   - window preferences (sandbox, context isolation, no node integration,
     no webview), CSP enforced, no network, path traversal, `window.open`
     denied, navigation blocked.

   When it fails, download the `desktop-e2e-windows-logs` artifact. It holds
   Electron's own output, the sidecar's `core:` lines and `mvs.log`.
4. **Linux desktop e2e.** `desktop e2e (Linux, xvfb)` is green: the same suite
   on Linux (Electron's sandbox under Xvfb).
5. **Unit tests.** `desktop unit tests` is green, including
   `security.test.js`'s permission-handler cases.
6. **Fuses.** `npm run test:fuses` passes against a packaged build (the
   release dry run on the PR builds one). A new Electron can add fuses or
   change defaults.

## After merge

7. **Release dry run.** The PR's `release` dry run built the Windows
   installer and the Linux AppImage/deb with the new version.
8. **Manual smoke on a real Windows machine** (QA, after merge): install
   over the previous build, start, send a message, join a voice channel,
   check that the mic prompt appears once and that screen share is refused.
9. **CHANGELOG.** Note the Electron version under "Changed" (security fixes
   are the usual reason).
