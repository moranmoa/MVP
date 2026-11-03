// Writes the UNSIGNED manifest body for core/updater from a folder of
// release artifacts. The owner signs it offline with `updater sign-manifest`
// (docs/release/RELEASING.md). No dependencies: runs on any Node >= 18.
//
//   node manifest-body.mjs --version 0.1.0 --platforms "linux-x86_64 ..." --dir dist [--channel alpha] [--days 14] [--notes-dir notes] --out manifest-body.json
//   node manifest-body.mjs --restamp manifest-body.json [--days 14]
//
// Updater artifacts are named mvs-<version>-<platform>.zip|.tar.gz. Every
// platform in --platforms (space- or comma-separated) must have exactly one,
// and an artifact for any other platform is an error. Other files in --dir
// are ignored. --restamp only renews issued_at/expires_at (for signing days
// after the build); the hashes are left untouched.
//
// --notes-dir holds one <lang>.txt per language ("what's new", B8), e.g.
// he.txt, en.txt. They go into the signed body as `notes`, with the limits
// core/updater enforces (MAX_NOTES_LANGUAGES, MAX_NOTES_BYTES).

import { createHash } from "node:crypto";
import { createReadStream, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Mirrors core/updater: MAX_VALIDITY_SECS and is_valid_name.
const MAX_DAYS = 30;
const NAME = /^[a-z0-9_-]{1,32}$/;
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
// Mirrors core/updater notes limits (manifest.rs).
const NOTES_LANG = /^[a-z]{2,3}(-[A-Z]{2})?$/;
const MAX_NOTES_LANGUAGES = 8;
const MAX_NOTES_BYTES = 4096;

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined) fail(`bad argument ${argv[i]}`);
    out[argv[i].slice(2)] = argv[i + 1];
  }
  return out;
}

function fail(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

function stamp(body, days) {
  if (!(days >= 1 && days <= MAX_DAYS)) fail(`--days must be 1..${MAX_DAYS}`);
  body.issued_at = Math.floor(Date.now() / 1000);
  body.expires_at = body.issued_at + days * 86400;
}

function readNotes(dir) {
  const notes = {};
  for (const file of readdirSync(dir).sort()) {
    const m = file.match(/^(.+)\.txt$/);
    if (!m) continue;
    const lang = m[1];
    if (!NOTES_LANG.test(lang)) fail(`notes: ${file}: language must be ll, lll or ll-CC`);
    // Windows line endings and trailing blank lines are editing noise.
    const text = readFileSync(join(dir, file), "utf8").replace(/\r\n/g, "\n").replace(/\s+$/, "");
    if (!text) fail(`notes: ${file} is empty`);
    if (Buffer.byteLength(text) > MAX_NOTES_BYTES) fail(`notes: ${file} is over ${MAX_NOTES_BYTES} bytes`);
    // Same rule as core/updater: no control characters except newline.
    if (/[^\P{Cc}\n]/u.test(text)) fail(`notes: ${file} has control characters`);
    // Nor bidi overrides/isolates, which can reorder what the user reads
    // (LRM/RLM marks are fine).
    if (/[‪-‮⁦-⁩]/u.test(text)) fail(`notes: ${file} has bidi override characters`);
    notes[lang] = text;
  }
  if (Object.keys(notes).length === 0) fail(`notes: no <lang>.txt in ${dir}`);
  if (Object.keys(notes).length > MAX_NOTES_LANGUAGES) fail(`notes: more than ${MAX_NOTES_LANGUAGES} languages`);
  return notes;
}

function sha256(path) {
  return new Promise((resolve, reject) => {
    const h = createHash("sha256");
    createReadStream(path)
      .on("data", (d) => h.update(d))
      .on("end", () => resolve(h.digest("hex")))
      .on("error", reject);
  });
}

function write(path, body) {
  writeFileSync(path, JSON.stringify(body, null, 2) + "\n");
  const until = new Date(body.expires_at * 1000).toISOString();
  console.log(`${path}: ${body.channel} ${body.version}, ${Object.keys(body.artifacts).join(", ")}; expires ${until}`);
}

const a = args(process.argv.slice(2));
const days = Number(a.days ?? 14);

if (a.restamp) {
  const body = JSON.parse(readFileSync(a.restamp, "utf8"));
  stamp(body, days);
  write(a.restamp, body);
} else {
  const { version, dir, out } = a;
  const channel = a.channel ?? "alpha";
  if (!version || !dir || !out || !a.platforms) fail("need --version, --platforms, --dir and --out");
  if (!VERSION.test(version)) fail(`version ${version} is not MAJOR.MINOR.PATCH`);
  if (!NAME.test(channel)) fail(`bad channel ${channel}`);
  const platforms = new Set(a.platforms.split(/[\s,]+/).filter(Boolean));
  for (const p of platforms) if (!NAME.test(p)) fail(`bad platform name ${p}`);

  const prefix = `mvs-${version}-`;
  const artifacts = {};
  for (const file of readdirSync(dir).sort()) {
    const m = file.startsWith(prefix) && file.slice(prefix.length).match(/^(.+?)\.(zip|tar\.gz)$/);
    if (!m) continue;
    const platform = m[1];
    if (!platforms.has(platform)) fail(`${file}: platform ${platform} is not in --platforms`);
    if (artifacts[platform]) fail(`two artifacts for ${platform}`);
    const path = join(dir, file);
    artifacts[platform] = { sha256: await sha256(path), size: statSync(path).size };
  }
  for (const p of platforms) if (!artifacts[p]) fail(`no ${prefix}${p} artifact in ${dir}`);

  const body = { format: 1, channel, version, issued_at: 0, expires_at: 0, artifacts, revoked: [] };
  if (a["notes-dir"]) body.notes = readNotes(a["notes-dir"]);
  stamp(body, days);
  write(out, body);
}
