// Third-party notices for the desktop installers (T22); dependency-free.
//
//   node third-party-notices.mjs npm <package-lock.json> <dir with node_modules> <out.txt>
//     The packages a production build ships (lockfile entries not marked dev),
//     with each package's own licence files. For ui/: Vite bundles them.
//
//   node third-party-notices.mjs cargo <metadata.json> <root crate,...> <out.txt>
//     The normal (non-dev, non-build) dependency closure of the root crates,
//     from `cargo metadata --filter-platform <target>` after `cargo fetch`,
//     with each crate's own licence files. Workspace members are ours: skipped.
//
//   node third-party-notices.mjs merge <out.txt> <part.txt>...
//     One file, with a header, and checks that clauses we must carry are in
//     it (ring 0.16's OpenSSL licence, webpki-roots' CDLA-Permissive-2.0).
//
// Licence texts come from the packages themselves (LICENSE*, LICENCE*,
// COPYING*, NOTICE*, UNLICENSE, plus Cargo's license-file). A package
// without one is listed with its SPDX expression and authors, and counted
// in a warning, so it can be looked at.
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";

const LICENCE_FILE = /^(licen[cs]e|copying|notice|unlicense)([-._].*)?$/i;
const MAX_FILE = 256 * 1024;
const RULE = "=".repeat(78);

function licenceTexts(dir, extra = []) {
  if (!dir || !existsSync(dir)) return [];
  const names = new Set(extra.filter(Boolean).map((f) => path.basename(f)));
  for (const name of readdirSync(dir)) if (LICENCE_FILE.test(name)) names.add(name);
  const out = [];
  for (const name of [...names].sort()) {
    const file = path.join(dir, name);
    if (!existsSync(file) || !statSync(file).isFile() || statSync(file).size > MAX_FILE) continue;
    out.push({ name, text: readFileSync(file, "utf8").replace(/\r\n/g, "\n").trimEnd() });
  }
  return out;
}

// The same licence text (Apache-2.0, MIT with the same holder, ...) is printed
// once; later packages point back to the first one that carried it.
const firstWithText = new Map();
const norm = (t) => t.replace(/\s+/g, " ").trim();

function entry({ name, version, licence, authors, repository, texts }) {
  const lines = [RULE, `${name} ${version}`, `Licence: ${licence || "(not declared)"}`];
  if (authors?.length) lines.push(`Authors: ${authors.join(", ")}`);
  if (repository) lines.push(`Source: ${repository}`);
  if (!texts.length) lines.push("", "(This package ships no licence file; the licence above applies.)");
  for (const t of texts) {
    const key = norm(t.text);
    const first = firstWithText.get(key);
    if (first) {
      lines.push("", `--- ${t.name}: same text as ${first} above ---`);
    } else {
      firstWithText.set(key, `${t.name} of ${name} ${version}`);
      lines.push("", `--- ${t.name} ---`, "", t.text);
    }
  }
  return lines.join("\n");
}

function npmNotices(lockPath, root, out) {
  const lock = JSON.parse(readFileSync(lockPath, "utf8"));
  const parts = [];
  let missing = 0;
  for (const [key, pkg] of Object.entries(lock.packages ?? {})) {
    if (!key || pkg.dev || pkg.link) continue; // "" is the project itself
    const name = pkg.name ?? key.slice(key.lastIndexOf("node_modules/") + "node_modules/".length);
    const texts = licenceTexts(path.join(root, key));
    if (!texts.length) missing += 1;
    parts.push({ sort: `${name}@${pkg.version}`, make: () => entry({ name, version: pkg.version, licence: pkg.license, texts }) });
  }
  write(out, parts);
  report(out, parts.length, missing);
}

function cargoNotices(metaPath, roots, out) {
  const meta = JSON.parse(readFileSync(metaPath, "utf8"));
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  const members = new Set(meta.workspace_members);
  const want = new Set(roots.split(",").map((s) => s.trim()).filter(Boolean));
  const start = meta.packages.filter((p) => members.has(p.id) && want.has(p.name)).map((p) => p.id);
  if (start.length !== want.size) {
    console.error(`::error::root crates not found in the workspace: ${roots}`);
    process.exit(1);
  }
  // Normal dependencies only: build-dependencies run on the build machine
  // and dev-dependencies only in tests; neither ends up in the binary.
  const seen = new Set();
  const queue = [...start];
  while (queue.length) {
    const id = queue.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const d of nodes.get(id)?.deps ?? []) {
      if (d.dep_kinds.some((k) => k.kind === null)) queue.push(d.pkg);
    }
  }
  const parts = [];
  let missing = 0;
  for (const id of seen) {
    if (members.has(id)) continue;
    const p = byId.get(id);
    const dir = path.dirname(p.manifest_path);
    const texts = licenceTexts(dir, [p.license_file]);
    if (!texts.length) missing += 1;
    parts.push({
      sort: `${p.name}@${p.version}`,
      make: () => entry({ name: p.name, version: p.version, licence: p.license, authors: p.authors, repository: p.repository, texts }),
    });
  }
  write(out, parts);
  report(out, parts.length, missing);
}

// Entries are built in sorted order, so "same text as ... above" holds.
function write(out, parts) {
  parts.sort((a, b) => a.sort.localeCompare(b.sort));
  writeFileSync(out, parts.map((p) => p.make()).join("\n\n") + "\n");
}

function report(out, count, missing) {
  console.log(`${out}: ${count} packages`);
  if (missing) console.log(`::warning::${missing} package(s) in ${path.basename(out)} ship no licence file (listed with their SPDX licence)`);
}

// Clauses we are bound to reproduce, checked against what was generated.
const REQUIRED = [
  { when: /^ring 0\.16\./m, must: /OpenSSL License/, why: "ring 0.16 carries code under the OpenSSL licence" },
  { when: /^webpki-roots \S+\nLicence: [^\n]*CDLA-Permissive-2\.0/m, must: /Community Data License Agreement/i, why: "webpki-roots' CA data is CDLA-Permissive-2.0" },
];

function merge(out, partFiles) {
  const header = [
    "MVS: third-party notices",
    "",
    "MVS includes the open-source software listed below. Each entry gives the",
    "package, its version, its licence and the licence text it ships with.",
    "",
    "Electron and Chromium: their licences are installed next to the app as",
    "LICENSE.electron.txt and LICENSES.chromium.html.",
    "",
    "Generated from the lockfiles at build time (.github/scripts/third-party-notices.mjs).",
  ].join("\n");
  const sections = partFiles.map((f) => {
    const title = path.basename(f, ".txt");
    return `${"#".repeat(78)}\n# ${title}\n${"#".repeat(78)}\n\n${readFileSync(f, "utf8").trimEnd()}`;
  });
  const text = `${header}\n\n${sections.join("\n\n")}\n`;
  for (const r of REQUIRED) {
    if (r.when.test(text) && !r.must.test(text)) {
      console.error(`::error::third-party notices: ${r.why}, but its licence text is missing`);
      process.exit(1);
    }
  }
  writeFileSync(out, text);
  console.log(`${out}: ${text.length} bytes from ${partFiles.length} part(s)`);
}

const [mode, ...args] = process.argv.slice(2);
if (mode === "npm" && args.length === 3) npmNotices(...args);
else if (mode === "cargo" && args.length === 3) cargoNotices(...args);
else if (mode === "merge" && args.length >= 2) merge(args[0], args.slice(1));
else {
  console.error("usage: third-party-notices.mjs npm <lock> <dir> <out> | cargo <metadata.json> <roots> <out> | merge <out> <part>...");
  process.exit(2);
}
