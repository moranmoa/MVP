// Summarises results/<room>/*.json into one Markdown table row per room and
// receive direction (sender variant -> receiver variant).
//
//   node summarize.mjs [room ...]     (default: every room in results/)
//   RESULTS=results-windows node summarize.mjs win6

import { readdirSync, readFileSync } from 'node:fs';

const base = process.env.RESULTS || 'results';
const rooms = process.argv.slice(2).length ? process.argv.slice(2) : readdirSync(base);
const median = (xs) => {
  const s = xs.filter((x) => typeof x === 'number').sort((a, b) => a - b);
  return s.length ? s[Math.floor(s.length / 2)] : null;
};
const max = (xs) => {
  const s = xs.filter((x) => typeof x === 'number');
  return s.length ? Math.max(...s) : null;
};
const f = (x, d = 0) => (x === null || x === undefined ? '–' : x.toFixed(d));

console.log('| room | netem | direction | links | latency median ms | latency p95 ms (worst link) | jitter ms | loss % | jitter buffer ms | concealed % |');
console.log('| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |');
const cpuRows = [];
for (const room of rooms) {
  const reports = readdirSync(`${base}/${room}`).map((n) => JSON.parse(readFileSync(`${base}/${room}/${n}`, 'utf8')));
  const groups = new Map();
  for (const r of reports) {
    for (const p of Object.values(r.peers)) {
      const key = `${p.kind} -> ${r.variant}`;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(p);
    }
  }
  // A fake microphone is not aligned to the clock: no latency from mic runs.
  const timed = reports[0].src === 'pulses';
  for (const [dir, links] of [...groups].sort()) {
    console.log(
      `| ${room} | ${reports[0].netem} | ${dir} | ${links.length} | ${timed ? f(median(links.map((l) => l.latencyMs?.median))) : 'n/a'} | ${timed ? f(max(links.map((l) => l.latencyMs?.p95))) : 'n/a'} | ${f(median(links.map((l) => l.jitterMs?.median)), 1)} | ${f(median(links.map((l) => l.lossPct)), 1)} | ${f(median(links.map((l) => l.jitterBufferMs)))} | ${f(median(links.map((l) => l.concealedPct)), 1)} |`,
    );
  }
  for (const variant of new Set(reports.map((r) => r.variant))) {
    const rs = reports.filter((r) => r.variant === variant);
    cpuRows.push(
      `| ${room} | ${variant} | ${rs.length} | ${rs[0].src} | ${f(median(rs.map((r) => r.cpuPctOfOneCore)), 1)} | ${f(max(rs.map((r) => r.cpuPctOfOneCore)), 1)} | ${f(median(rs.map((r) => r.memMiB)))} |`,
    );
  }
}
console.log('\n| room | variant | participants | source | CPU % of one core, median | max | container memory MiB |');
console.log('| --- | --- | --- | --- | --- | --- | --- |');
cpuRows.forEach((r) => console.log(r));
