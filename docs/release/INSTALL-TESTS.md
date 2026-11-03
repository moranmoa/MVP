# Install and uninstall tests (T12)

Owner requirement: installing, updating and removing MVS works on every OS,
and **the identity is never deleted without the user's explicit consent**.

This file has the detailed steps and the automated checks behind QA's cases
INS-01..16 (`qa/TEST_PLAN.md` §14). It covers the two shell spike
packages (PR #59). **Electron is the chosen shell (decision 7)**, so its
results are the main path. The Tauri rows stay as measured, for the
record, and the Tauri jobs can be dropped from CI once the real Electron
packages (B1-3 / B9) replace the spike ones, with the names below changed.

## Packages

Built by `spikes/shell/build-packages.sh [out dir]` (Docker only).

| OS | File | Install | Uninstall | Admin? |
| --- | --- | --- | --- | --- |
| Windows x64 | `shell-tauri-windows-x64-setup.exe` (NSIS) | `setup.exe /S` | `%LOCALAPPDATA%\ShellSpikeTauri\uninstall.exe /S` | no (per-user) |
| Windows x64 | `shell-electron-windows-x64-setup.exe` (NSIS) | `setup.exe /S` | `"%LOCALAPPDATA%\Programs\shell-spike-electron\Uninstall ShellSpikeElectron.exe" /currentuser /S` | no (per-user) |
| Linux x64 | `shell-{tauri,electron}-linux-amd64.deb` | `sudo apt install ./file.deb` | `sudo apt remove <pkg>` (+ `purge`) | yes |
| Linux x64 | `shell-{tauri,electron}-linux-amd64.AppImage` | `chmod +x`, run it | delete the file | no |
| macOS | none yet | – | – | – |

**Per-user install without admin rights (INS-04):** supported on Windows
(both NSIS installers are per-user) and on Linux through the AppImage. The
`.deb` needs root by design. macOS isn't tested until there's a package.

## What each package creates

This is what to diff before and after (INS-01, 02, 08). CI checks exactly
these lists. Paths are per user unless they start with `/` or `HKLM`.

| | Tauri | Electron |
| --- | --- | --- |
| Windows program folder | `%LOCALAPPDATA%\ShellSpikeTauri` | `%LOCALAPPDATA%\Programs\shell-spike-electron` |
| Windows uninstall key | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ShellSpikeTauri` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\169df8e2-50f9-5fea-b18c-9805ad553a53` |
| Other registry | none | `HKCU\Software\169df8e2-50f9-5fea-b18c-9805ad553a53` |
| Windows shortcuts | Start menu + desktop `ShellSpikeTauri.lnk` | Start menu + desktop `ShellSpikeElectron.lnk` |
| Linux `.deb` files | `/usr/bin/shell-spike-tauri`, `/usr/share/applications/ShellSpikeTauri.desktop` | `/opt/ShellSpikeElectron/`, `/usr/bin/shell-spike-electron`, `/usr/share/applications/shell-spike-electron.desktop` |
| AppImage | nothing outside the file itself | same |
| `mvs://` handler | **none** (`HKCU\Software\Classes\mvs`, `x-scheme-handler/mvs`) | **none** |
| Autostart | none (`...\CurrentVersion\Run`, Startup folder, `~/.config/autostart`, `/etc/xdg/autostart`) | none |
| File associations, services, scheduled tasks, firewall rules | none | none |
| Data, Windows (created on first launch) | `%LOCALAPPDATA%\dev.mvp.shellspike.tauri` (WebView2 profile) | `%APPDATA%\shell-spike-electron` |
| Data, Linux | `~/.local/share/dev.mvp.shellspike.tauri` | `~/.config/shell-spike-electron` (+ `~/.local/share/pki`, Chromium's certificate store) |
| Identity | none: the spike's mock core keeps it in `localStorage`, inside the data folder | same |

The real package must add an `mvs://` handler (INS-01/02 expect one) and
must keep the identity in the OS keychain. Both are gaps of the spike
packages, not passes.

## What each uninstall route does to data and identity (INS-08, 10)

| Route | Program files, shortcuts, registry, `.desktop` | Data folder | Identity (keychain) |
| --- | --- | --- | --- |
| Windows: Settings → Apps → Uninstall | removed | Tauri: kept unless the user ticks "delete application data" (built-in checkbox). Electron: kept (no prompt) | not touched |
| Windows: silent `/S` (admin removal) | removed | kept | not touched |
| Linux: `apt remove` | removed | kept | not touched |
| Linux: `apt purge` / `dpkg --purge` | removed | kept (`$HOME` is never touched by dpkg) | not touched |
| Linux: delete the AppImage | the file only | kept | not touched |

No route deletes the identity, so the spike passes INS-10 trivially. The
real package needs two things:

- **The keep/delete prompt (INS-09).** Tauri has the checkbox built in, but
  it still needs the "without the 12 words you can't recover" warning text.
  Electron's `deleteAppDataOnUninstall` deletes without asking, so it needs
  a custom NSIS page.
- **Identity handling on uninstall.** The keychain entry is never removed
  by the uninstaller; only the app's own "delete my identity" flow may
  remove it, after the warning.

## Automated checks (CI)

Workflow: [`.github/workflows/install-tests.yml`](../../.github/workflows/install-tests.yml)
runs on pull requests that touch the shells or these tests, and by hand
(Actions → Install tests → Run workflow). It builds the six packages once,
then runs:

- **Windows** (`windows-latest`, [`packaging/install-tests/windows.ps1`](../../packaging/install-tests/windows.ps1)):
  1. Snapshot: HKCU/HKLM uninstall keys, `HKCU\Software`, `HKCU\Software\Classes`
     (`mvs://`, file types), Run keys, Start menu, Startup, desktop, `%LOCALAPPDATA%`,
     `%LOCALAPPDATA%\Programs`, `%APPDATA%`, Program Files.
  2. `/S` install: exactly the expected entries appear (table above), and
     nothing in HKLM, Program Files or all-users locations (per-user). No
     autostart (INS-01, 04).
  3. First launch: still running after 10 s, and only the data folder
     appeared (INS-03, smoke test only).
  4. Install again over the existing install: no new or duplicate entries
     (INS-05, same version).
  5. Silent uninstall: everything removed except the data folder; nothing
     that existed before is gone; no process left running (INS-08, 10).
  6. Reinstall: same entries as the first install (INS-11, files only).
  7. A newly created **standard (non-admin) user** installs and uninstalls
     silently into its own profile (INS-04).
- **Linux** (Debian 13 container, [`packaging/install-tests/linux.sh`](../../packaging/install-tests/linux.sh)),
  per shell:
  1. `.deb`: `apt install`, the expected files and `.desktop` appear, no
     autostart or service; first launch as a normal user (Xvfb) is still
     running after 10 s and creates the data folder; `--reinstall` changes
     nothing; `apt remove` removes the files and keeps the data; purge keeps
     the data; reinstall starts with the old data (INS-02, 05, 08, 10, 11).
  2. AppImage as a normal user with no root: runs, creates only the data
     folder (no `.desktop`, no `mvs://`), and deleting the file leaves the
     data (INS-04, 10).

Each step prints its before/after diff, so a run's log is the record QA
attaches.

CI can't cover these, and they stay manual:

- the uninstall UI and the keep/delete prompt (INS-09);
- a real N-1 → N update (INS-05, 06; there is one version of each spike);
- interrupted installs (INS-07);
- restoring from the 12 words (INS-12; needs the real core);
- two users on one machine (INS-13);
- non-ASCII paths (INS-14);
- the app open during uninstall (INS-15);
- SmartScreen and Defender on a downloaded file (INS-16);
- macOS.

Limitations of CI as a substitute:

- The Windows runner's own user is an administrator; step 7 is the real
  no-admin check.
- The Linux checks run in a container with Xvfb. The Electron launch there
  needs `--no-sandbox`, because the container has no user namespaces for
  Chromium's sandbox; on a desktop the sandbox works.
- The packages are built on Debian 13 and need its glibc (2.41).
  **Ubuntu 24.04 (glibc 2.39) can't run them.** Real packages must be built
  on the oldest supported distribution (B9).

## Manual checklist (QA's pass)

On a clean VM with a snapshot. Note "before" and "after" for every item in
"What each package creates".

### Windows 10 and 11 (both packages)

- [ ] INS-01: download from a browser, run. Record the SmartScreen
      "unknown publisher" warning (unsigned alpha). The Start menu shortcut
      works. `mvs://` is not registered (a spike gap).
- [ ] INS-03: first launch: UI language and RTL, no firewall prompt (none is
      expected yet), usable in under 2 s.
- [ ] INS-04: as a standard user, install with no UAC prompt.
- [ ] INS-05: install over it again; Settings → Apps lists one entry and
      there's one shortcut.
- [ ] INS-07: cancel mid-install, run it again: it completes and the app
      starts.
- [ ] INS-08: uninstall from Settings → Apps: the program folder, uninstall
      key and shortcuts are gone, and the data folder is kept.
- [ ] INS-09 (Tauri): tick "delete application data": the data folder is
      gone. (Electron has no prompt: record it as a gap.)
- [ ] INS-11: reinstall: the old data is used.
- [ ] INS-13: user A and user B each install; uninstalling at A leaves B
      working.
- [ ] INS-14: user name or folder with Hebrew and spaces.
- [ ] INS-15: uninstall while the app runs: the uninstaller asks to close
      it or closes it, and no files are left locked.
- [ ] INS-16: Defender doesn't quarantine the installer or the app.

### Linux: Ubuntu LTS and Fedora

- [ ] INS-02: Ubuntu 24.04: the spike `.deb`/AppImage won't run (glibc;
      expected, see limitations). Run the steps on Debian 13 until B9
      builds on an older base. Fedora: AppImage only (no `.rpm`).
- [ ] INS-03, 08, 10, 11: as on Windows, with `apt remove`, `apt purge`
      and deleting the AppImage. `~/.local/share/...` / `~/.config/...` must
      survive all three.
- [ ] INS-04: AppImage as a user without sudo.

### macOS

- [ ] Not packaged yet. Add it when B9 produces a `.dmg`.

### With the real core (after B1/B9)

- [ ] INS-05: a real update N-1 → N, both by hand and through the updater:
      identity, servers, messages and settings are kept.
- [ ] INS-06: installing N-1 over N is blocked or warned about, and the
      data still opens.
- [ ] INS-09: the delete prompt shows the 12-words warning with a link to
      show the words.
- [ ] INS-10: the keychain entry survives every uninstall route.
- [ ] INS-12: after uninstalling with data deleted, reinstall, choose
      "I have an account", enter the 12 words: same identity code.

## Results

| Date | Where | Tauri | Electron | Notes |
| --- | --- | --- | --- | --- |
| 2026-11-03 | Linux checks, local (Docker Desktop, Debian 13 container), packages from dev-voice-shell's local build of PR #59 | all passed | all passed | Tauri's `.deb` has no conffiles, so after `apt remove` dpkg forgets it and `apt purge <pkg>` reports "unable to locate"; the test purges with `dpkg --purge` |
| – | CI, Windows + Linux | pending first run | pending | – |

What this told the shell decision (Electron was chosen, decision 7):

- Both behave the same on install and remove. Neither leaves anything but
  the data folder.
- Tauri's uninstaller already has the keep/delete data checkbox; Electron
  needs a custom NSIS page for INS-09.
- Package size: Tauri's `.deb` is 1.6 MB against Electron's 99 MB.
- Neither registers `mvs://` yet; both will need it.
