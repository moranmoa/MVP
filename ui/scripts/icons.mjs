// App icons from the logo (docs/ux/specs/desktop-chrome.md §1): PNGs for
// Linux and the window, and a PNG-based icon.ico for Windows and the
// installer, written to desktop/build/. Runs in the Playwright container with
// the repo mounted at /repo:
//   docker run --rm -v "$PWD:/repo" -v ui_ui_node_modules:/repo/ui/node_modules \
//     -w /repo/ui mcr.microsoft.com/playwright:v1.56.1-noble node scripts/icons.mjs

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';

const SVG = readFileSync('src/assets/mvs-logo.svg', 'utf8');
const OUT = '../desktop/build';
const PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512];
const ICO_SIZES = [16, 24, 32, 48, 64, 256];

mkdirSync(`${OUT}/icons`, { recursive: true });
const browser = await chromium.launch();
const page = await browser.newPage();
const pngs = new Map();
for (const size of PNG_SIZES) {
  await page.setViewportSize({ width: size, height: size });
  await page.setContent(`<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${SVG}`);
  const png = await page.screenshot({ omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
  pngs.set(size, png);
  writeFileSync(`${OUT}/icons/${size}x${size}.png`, png);
}
await browser.close();

// ICO with PNG entries (Windows Vista and later): header, directory, images.
const entries = ICO_SIZES.map((s) => pngs.get(s));
const header = Buffer.alloc(6 + 16 * entries.length);
header.writeUInt16LE(0, 0);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(entries.length, 4);
let offset = header.length;
ICO_SIZES.forEach((s, i) => {
  const at = 6 + 16 * i;
  header.writeUInt8(s >= 256 ? 0 : s, at);
  header.writeUInt8(s >= 256 ? 0 : s, at + 1);
  header.writeUInt16LE(1, at + 4);
  header.writeUInt16LE(32, at + 6);
  header.writeUInt32LE(entries[i].length, at + 8);
  header.writeUInt32LE(offset, at + 12);
  offset += entries[i].length;
});
writeFileSync(`${OUT}/icon.ico`, Buffer.concat([header, ...entries]));
console.log('icons written:', PNG_SIZES.join(', '), '+ icon.ico');
