// Fails when the Electron in a lockfile is too old to still get security fixes.
//
//   node electron-age.mjs desktop/package-lock.json [--metadata electron.json] [--now ISO]
//
// Rules (Electron supports the latest three majors and ships one every ~8 weeks):
//   1. major: the installed major may be at most one behind the current
//      stable major (latest or latest - 1).
//   2. patch: if a newer stable release in the installed major has been out
//      for more than 8 weeks, the pin is stale (security fixes not taken).
//
// The only network access is a GET of the npm registry document for
// `electron`; --metadata reads a saved copy instead (tests, offline runs).
import { readFileSync } from 'node:fs';

const REGISTRY_URL = 'https://registry.npmjs.org/electron';
const MAX_STALE_MS = 8 * 7 * 24 * 3600 * 1000;

function arg(name) {
  const i = process.argv.indexOf(name);
  return i === -1 ? undefined : process.argv[i + 1];
}

function parse(v) {
  const m = /^(\d+)\.(\d+)\.(\d+)(-.+)?$/.exec(v ?? '');
  if (!m) return null;
  return { major: +m[1], minor: +m[2], patch: +m[3], pre: m[4] ?? null, raw: v };
}

function cmp(a, b) {
  return a.major - b.major || a.minor - b.minor || a.patch - b.patch;
}

function fail(msg) {
  console.error(`electron-age: ${msg}`);
  process.exit(1);
}

const lockPath = process.argv[2];
if (!lockPath || lockPath.startsWith('--')) fail('usage: electron-age.mjs <package-lock.json> [--metadata file] [--now ISO]');

const lock = JSON.parse(readFileSync(lockPath, 'utf8'));
const entry = lock.packages?.['node_modules/electron'];
if (!entry) fail(`${lockPath} has no node_modules/electron entry`);
const installed = parse(entry.version);
if (!installed || installed.pre) fail(`installed electron ${entry.version} is not a stable release`);

let meta;
const metaPath = arg('--metadata');
if (metaPath) {
  meta = JSON.parse(readFileSync(metaPath, 'utf8'));
} else {
  const res = await fetch(REGISTRY_URL, { headers: { accept: 'application/json' }, redirect: 'error' });
  if (!res.ok) fail(`registry answered ${res.status}`);
  meta = await res.json();
}
const now = arg('--now') ? Date.parse(arg('--now')) : Date.now();
if (Number.isNaN(now)) fail('bad --now');

const latest = parse(meta['dist-tags']?.latest);
if (!latest || latest.pre) fail('registry has no stable dist-tags.latest');

const problems = [];

// 1. major
if (installed.major < latest.major - 1) {
  problems.push(`electron ${installed.raw} is major ${installed.major}; current is ${latest.raw}, so at least ${latest.major - 1}.x is required`);
}

// 2. patch: the oldest newer stable release in the same major, and how long it has been out
const newer = Object.keys(meta.versions ?? {})
  .map(parse)
  .filter((v) => v && !v.pre && v.major === installed.major && cmp(v, installed) > 0)
  .sort(cmp);
if (newer.length > 0) {
  const published = Date.parse(meta.time?.[newer[0].raw] ?? '');
  if (Number.isNaN(published)) fail(`registry has no publish time for ${newer[0].raw}`);
  const ageDays = Math.floor((now - published) / 86400000);
  if (now - published > MAX_STALE_MS) {
    const newest = newer[newer.length - 1].raw;
    problems.push(`electron ${installed.raw} was superseded by ${newer[0].raw} ${ageDays} days ago (newest ${installed.major}.x is ${newest}); update within 8 weeks`);
  }
}

if (problems.length > 0) {
  for (const p of problems) console.error(`electron-age: ${p}`);
  process.exit(1);
}
console.log(`electron ${installed.raw} ok (current stable ${latest.raw})`);
