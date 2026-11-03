#!/usr/bin/env node
// npm licence policy check, the JS twin of core/deny.toml (policy in docs/spec.md).
//
// Reads the lockfile only: no packages are installed or executed, and every
// transitive dependency is covered.
//
//   node .github/scripts/npm-licenses.mjs ui/package-lock.json
//
// Shipped packages (not marked "dev") must use the permissive allow list.
// Build/test tooling never reaches users, so it may also use a few
// permissive-with-conditions licences; copyleft is refused everywhere.

import { readFileSync } from "node:fs";

const SHIPPED = new Set([
  "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "0BSD", "ISC", "Zlib",
  "Unicode-3.0", "Unicode-DFS-2016", "CC0-1.0",
  // Architect approval only, and only while its files stay unmodified.
  "MPL-2.0",
]);
// Dev tooling only. BlueOak-1.0.0 is permissive; CC-BY-4.0 is data (e.g.
// caniuse-lite) and needs attribution if it were ever shipped.
const DEV_ONLY = new Set(["BlueOak-1.0.0", "CC-BY-4.0", "Python-2.0"]);

// Explicit, reviewed exceptions: "name@version": "why". Template:
//   "some-lib@1.2.3": "LGPL-2.1, dynamically linked and unmodified; approved by <who>, <date>",
const EXCEPTIONS = {};

/** True if a (simple) SPDX expression is satisfied by `allowed`. */
function satisfies(expr, allowed) {
  const e = expr.trim().replace(/^\((.*)\)$/, "$1");
  if (/\bOR\b/.test(e)) return e.split(/\s+OR\s+/).some((p) => satisfies(p, allowed));
  if (/\bAND\b/.test(e)) return e.split(/\s+AND\s+/).every((p) => satisfies(p, allowed));
  return allowed.has(e);
}

const lockPath = process.argv[2];
if (!lockPath) {
  console.error("usage: npm-licenses.mjs <package-lock.json>");
  process.exit(2);
}
const lock = JSON.parse(readFileSync(lockPath, "utf8"));
if (lock.lockfileVersion < 2 || !lock.packages) {
  console.error(`${lockPath}: lockfileVersion 2+ required (has per-package licence fields)`);
  process.exit(2);
}

const devAllowed = new Set([...SHIPPED, ...DEV_ONLY]);
const failures = [];
let shipped = 0;
let dev = 0;
for (const [path, pkg] of Object.entries(lock.packages)) {
  if (path === "") continue; // the app itself
  if (pkg.link) continue; // workspace symlink; its own entry is checked
  const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
  const id = `${name}@${pkg.version}`;
  const isDev = pkg.dev === true || pkg.devOptional === true;
  isDev ? dev++ : shipped++;
  const licence = typeof pkg.license === "string" ? pkg.license : undefined;
  if (EXCEPTIONS[id]) continue;
  if (!licence) {
    failures.push(`${id}: no licence field${isDev ? " (dev)" : ""}`);
  } else if (!satisfies(licence, isDev ? devAllowed : SHIPPED)) {
    failures.push(`${id}: ${licence} not allowed for ${isDev ? "dev tooling" : "shipped code"}`);
  }
}

console.log(`checked ${shipped} shipped and ${dev} dev packages from ${lockPath}`);
if (failures.length) {
  console.error(`licence policy violations:\n  ${failures.join("\n  ")}`);
  process.exit(1);
}
console.log("licences ok");
