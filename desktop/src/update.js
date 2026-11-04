// In-app update, the shell's part (B8, core/updater/docs/B8-in-app-update.md
// §0 "dev-voice-shell"). No Electron imports: test/update.test.js drives it.
//
// Install layout (Windows):
//   <root>/mvs-launcher.exe           never replaced by an update
//   <root>/MVS.exe                    the base version (the installer's)
//   <root>/versions/<ver>/MVS.exe     versions the updater unpacked
//
// The update root is DERIVED from where this executable is; it is never taken
// from the command line, which anyone who can start the app controls. The
// launcher adds --mvs-update-root=<root>; it is only accepted if it names the
// same folder, otherwise updates stay off for this run.
//
// Updates are on only in an installed copy that has the launcher. Then:
//   - the core gets --update-root <root> (it stages updates there);
//   - mvs:// is registered to the launcher, so a link starts the healthy version;
//   - restartToUpdate relaunches through the launcher (it picks the new version);
//   - after a good start main tells the core `markHealthy`, which commits the
//     version. The renderer can never do this (not on the allow-list, GAP-17):
//     a broken UI must not declare itself healthy.

'use strict';

const fs = require('node:fs');
const path = require('node:path');

const ARG = '--mvs-update-root=';
/** A version folder name: semver, optionally with a pre-release tag. */
const VERSION_DIR = /^\d{1,5}\.\d{1,5}\.\d{1,5}(?:-[0-9A-Za-z.-]{1,64})?$/;

const launcherName = (platform) => (platform === 'win32' ? 'mvs-launcher.exe' : 'mvs-launcher');

/** <root> for an exe in <root>/ or <root>/versions/<ver>/, if the launcher is there. */
function deriveRoot({ execPath, platform = process.platform, exists = fs.existsSync }) {
  const dir = path.dirname(execPath);
  const candidates = [dir];
  const parent = path.dirname(dir);
  if (path.basename(parent) === 'versions' && VERSION_DIR.test(path.basename(dir))) candidates.unshift(path.dirname(parent));
  for (const root of candidates) {
    if (exists(path.join(root, launcherName(platform)))) return root;
  }
  return null;
}

function sameDir(a, b, platform) {
  const norm = (p) => path.resolve(p).replace(/[\\/]+$/, '');
  return platform === 'win32' ? norm(a).toLowerCase() === norm(b).toLowerCase() : norm(a) === norm(b);
}

/**
 * { root, launcher } when updates are on for this run, else null.
 * @param {{ packaged: boolean, execPath: string, argv: string[], platform?: string,
 *           exists?: (p: string) => boolean, warn?: (m: string) => void }} o
 */
function resolveUpdate({ packaged, execPath, argv, platform = process.platform, exists = fs.existsSync, warn = (m) => console.warn(m) }) {
  if (!packaged) return null;
  const root = deriveRoot({ execPath, platform, exists });
  if (!root) return null;
  const given = argv.find((a) => typeof a === 'string' && a.startsWith(ARG));
  if (given !== undefined && !sameDir(given.slice(ARG.length), root, platform)) {
    warn('--mvs-update-root does not match the install folder; updates are off for this run');
    return null;
  }
  return { root, launcher: path.join(root, launcherName(platform)) };
}

/** Switches for mvs-core. */
function coreArgs(upd) {
  return upd ? ['--update-root', upd.root] : [];
}

/**
 * After the window has loaded: ping the core, then commit this version.
 * `request` is Sidecar#request; it never rejects.
 */
async function reportHealthy(upd, request, log = (m) => console.log(m)) {
  if (!upd) return false;
  const ping = await request('getAppInfo', {});
  if (ping.error) {
    log(`not marking healthy: core ${ping.error.code}`);
    return false;
  }
  const res = await request('markHealthy', {});
  if (res.error) {
    log(`markHealthy: ${res.error.code}`);
    return false;
  }
  return true;
}

module.exports = { ARG, VERSION_DIR, launcherName, deriveRoot, resolveUpdate, coreArgs, reportHealthy };
