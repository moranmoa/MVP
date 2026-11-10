# Third-party notices (T22)

Every installer carries the licences of the open-source code it ships.

## What is generated, and where it goes

`release.yml` (each `desktop` matrix job, tag builds and dry runs alike) runs
`.github/scripts/third-party-notices.mjs`. It writes
`desktop/build/THIRD-PARTY-NOTICES.txt`, which electron-builder copies to
`resources/THIRD-PARTY-NOTICES.txt` in the app. That copy is inside the
Windows installer, the updater zip, the AppImage, the .deb and the tar.gz.
The file contains:

- **Rust crates** in the core: the normal (non-dev, non-build) dependency
  closure of `mvs-core`, plus `mvs-launcher` on Windows, resolved for that
  platform (`cargo metadata --filter-platform`). Our own workspace crates are
  left out.
- **npm packages** bundled into the UI: the entries in `ui/package-lock.json`
  that are not marked `dev`.
- **Electron and Chromium:** electron-builder already ships
  `LICENSE.electron.txt` and `LICENSES.chromium.html` next to the app. The
  notices file points to them, and the Collect step fails if any of the three
  files is missing.

Each entry gives the name, version, SPDX licence, authors, source and the
licence files the package itself ships (LICENSE*, COPYING*, NOTICE*,
Cargo's `license-file`). Identical texts are printed once, and later entries
point back to the first one. A package that ships no licence file is still
listed with its SPDX licence; the build logs a warning with the count.

Clauses we must reproduce are checked, and the build fails without them:

- **ring 0.16:** its LICENSE, including the OpenSSL and SSLeay licences.
  This goes away when #256 (libp2p 0.56) drops ring 0.16.
- **webpki-roots under CDLA-Permissive-2.0** (1.x, from #256; the owner
  approved the exception): the CDLA text. webpki-roots 1.0.11 ships it as its
  own LICENSE, so it comes through like any other licence file. The check
  makes sure it never goes missing.

The licence *policy* (what may be used at all) stays in `npm-licenses.mjs`
and `core/deny.toml`; this file is only about attribution.

## Settings → About (spec for dev-ui)

The app shows the notices, so users don't have to find a file in the
install folder.

- **Main process (desktop):** a new IPC `notices:read`, available to our own
  page only (top frame, `app://mvs`, like `core:invoke`). It returns the
  UTF-8 text of `path.join(process.resourcesPath, 'THIRD-PARTY-NOTICES.txt')`
  when packaged. In a checkout, or when the file is missing, it returns
  `null`. Read it on request, not at start-up (it is ~0.6 MB). No path comes
  from the renderer.
- **Preload:** expose it as `notices()` next to `copyText` / `invoke` /
  `onEvent`. Update the e2e test "the page sees only copyText, invoke and
  onEvent" accordingly.
- **UI:** Settings → About gets a "Third-party notices" row that opens a
  full-height, scrollable, monospace view of the text, with "Copy all". When
  `notices()` gives `null` (web build, dev run), show: "Notices are included
  with the installed app." Plain text only: never render it as HTML.
- **Electron/Chromium line:** below the view, show "Electron and Chromium
  licences: LICENSE.electron.txt and LICENSES.chromium.html in the app's
  folder."
- **Tests:** unit-test the IPC (own page only, `null` without the file). The
  QA smoke opens About → Third-party notices against the mock.
