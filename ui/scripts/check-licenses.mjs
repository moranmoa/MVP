// Fails when an npm package has a licence outside the allow-list.
// Reads package-lock.json, so it needs no network: `npm run licenses`.
//
// What ships (dependencies) must be permissive. Dev-only tools may also use
// the few extra permissive licences below; they are never bundled.

import { readFileSync } from 'node:fs';

const SHIPPED = ['MIT', 'ISC', 'BSD-2-Clause', 'BSD-3-Clause', '0BSD', 'Apache-2.0'];
const DEV_ONLY = [...SHIPPED, 'BlueOak-1.0.0', 'MIT-0', 'CC0-1.0', 'CC-BY-4.0'];

/** True when an SPDX expression is satisfied by the allow-list ("A OR B", "A AND B", brackets). */
function allowed(expr, list) {
  const e = expr.trim().replace(/^\((.*)\)$/, '$1');
  if (/ OR /i.test(e)) return e.split(/ OR /i).some((p) => allowed(p, list));
  if (/ AND /i.test(e)) return e.split(/ AND /i).every((p) => allowed(p, list));
  return list.includes(e);
}

const lock = JSON.parse(readFileSync(new URL('../package-lock.json', import.meta.url), 'utf8'));
const bad = [];
const seen = { shipped: new Map(), dev: new Map() };

for (const [path, pkg] of Object.entries(lock.packages)) {
  if (!path) continue; // the project itself
  const name = path.slice(path.lastIndexOf('node_modules/') + 'node_modules/'.length);
  const license = typeof pkg.license === 'string' ? pkg.license : 'UNKNOWN';
  const kind = pkg.dev || pkg.devOptional ? 'dev' : 'shipped';
  seen[kind].set(license, (seen[kind].get(license) ?? 0) + 1);
  if (!allowed(license, kind === 'dev' ? DEV_ONLY : SHIPPED)) bad.push(`${name}@${pkg.version} (${kind}): ${license}`);
}

const summary = (m) => [...m].sort((a, b) => b[1] - a[1]).map(([l, n]) => `${l} ×${n}`).join(', ');
console.log(`shipped: ${summary(seen.shipped)}`);
console.log(`dev only: ${summary(seen.dev)}`);
if (bad.length) {
  console.error(`\nNot allowed:\n  ${bad.join('\n  ')}`);
  process.exit(1);
}
console.log('licences ok');
