// T8 empty-state screenshots on the Electron shell. Runs inside the desktop
// real-core container: cwd /repo/desktop, OUT=/out.
import { _electron as electron } from '/repo/desktop/node_modules/playwright-core/index.mjs';
import { mkdirSync } from 'node:fs';

const OUT = '/out';
mkdirSync(OUT, { recursive: true });
const log = (...a) => console.log(...a);
const MODE = process.env.MODE ?? 'real';

const env = { ...process.env };
if (MODE === 'nocore') delete env.MVS_CORE_PATH;
const app = await electron.launch({ args: ['.'], env });
const p = await app.firstWindow();
p.on('pageerror', (e) => log('PAGEERR', e.message, JSON.stringify(e).slice(0, 200)));
p.on('console', (m) => m.type() === 'error' && log('CONSOLE', m.text().slice(0, 200)));
await p.setViewportSize({ width: 1280, height: 800 });
const t0 = Date.now();
const shot = async (name) => {
  await p.waitForTimeout(400);
  await p.screenshot({ path: `${OUT}/${MODE}-${name}.png` });
  log('saved', name, `${Date.now() - t0}ms`);
};
const h = async () => (await p.locator('h1,h2').first().textContent().catch(() => '?'))?.trim();

await p.waitForTimeout(1500);
log('first heading:', await h());
await shot('01-first-run');
if (MODE === 'nocore') {
  log('body:', (await p.locator('body').innerText()).slice(0, 400));
  await app.close();
  process.exit(0);
}

const next = () => p.getByRole('button', { name: 'המשך' }).click();
await p.getByText('עברית', { exact: true }).click();
await p.waitForTimeout(300);
await shot('01b-language-he');
await next();
await shot('01c-agreement');
await p.getByRole('checkbox').check();
await next();
await next();
await p.getByLabel('יום').fill('12');
await p.getByLabel('חודש').fill('4');
await p.getByLabel('שנה').fill('1998');
const tKeys = Date.now();
await next();
await shot('02-preparing');
await p.getByRole('heading', { name: 'מילות השחזור שלך' }).waitFor({ timeout: 120000 });
log('preparing took', Date.now() - tKeys, 'ms');
await p.getByRole('button', { name: 'הצגת המילים' }).click();
await p.waitForTimeout(500);
const words = (await p.locator('.word-grid .word').allTextContents()).map((s) => s.replace(/^\s*\d+\.?\s*/, '').trim());
log('words', words.length);
await p.getByRole('button', { name: 'כתבתי אותן' }).click();
await p.locator('.check-fields input').first().waitFor();
const asked = (await p.locator('.check-fields label span').allTextContents()).map((s) => Number(s.match(/\d+/)[0]));
for (let j = 0; j < asked.length; j++) await p.locator('.check-fields input').nth(j).fill(words[asked[j] - 1]);
await p.getByRole('button', { name: 'בדיקה', exact: true }).click();
await p.waitForTimeout(1200);
log('after check:', await h());
await shot('03-profile');
await p.getByLabel('שם תצוגה').fill('נועה');
await p.getByRole('button', { name: 'סיום' }).click();
await p.waitForTimeout(1500);
log('home heading:', await h());
await shot('04-home-no-servers');
log('home text:', (await p.locator('main').innerText().catch(() => '')).slice(0, 500));

// Phone width and 200% on the empty home.
await p.setViewportSize({ width: 375, height: 812 });
await shot('05-home-phone');
await p.setViewportSize({ width: 640, height: 400 });
await p.evaluate(() => document.documentElement.style.setProperty('font-size', '24px', 'important'));
await shot('06-home-200');
log('200 overflow', await p.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth));
await p.evaluate(() => document.documentElement.style.removeProperty('font-size'));
await p.setViewportSize({ width: 1280, height: 800 });

// Create a server: the "only me" server and its empty channel.
await p.getByRole('button', { name: /יצירת שרת/ }).first().click();
await p.waitForTimeout(500);
await shot('07-create-server');
const nameField = p.getByLabel(/שם השרת/);
if (await nameField.count()) {
  await nameField.fill('הקבוצה שלנו');
  await p.getByRole('radio', { name: '16 ומעלה' }).check();
  const tc = Date.now();
  await p.getByRole('button', { name: 'יצירה', exact: true }).click();
  await p.waitForTimeout(2500);
  log('create took ~', Date.now() - tc, 'ms; dialog text:', (await p.locator('dialog[open]').innerText().catch(() => 'none')).slice(0, 400));
  await shot('08-server-created');
  const buttons = await p.locator('dialog[open] button').allTextContents().catch(() => []);
  log('dialog buttons:', buttons);
  const go = p.locator('dialog[open]').getByRole('button', { name: 'מעבר לשרת', exact: true });
  if (await go.count()) await go.first().click();
  await p.waitForTimeout(1500);
  await shot('09-empty-channel');
  log('page text:', (await p.locator('body').innerText().catch(() => '')).slice(0, 700));
  await p.setViewportSize({ width: 375, height: 812 });
  await shot('10-empty-channel-phone');
  await p.setViewportSize({ width: 1280, height: 800 });
}

// Join a link nobody serves.
await p.getByRole('button', { name: /בית/ }).first().click().catch(() => {});
await p.waitForTimeout(500);
const joinCard = p.getByRole('button', { name: /הצטרפות בלינק/ });
if (await joinCard.count()) {
  await joinCard.first().click();
  await p.waitForTimeout(500);
  const field = p.locator('dialog[open] input').first();
  await field.fill('mvs://join/cf46aefb6c50c8105f1670717cf3227a27e34e6387a063db6f50813e5ad8ca74/bd796287ce6bf8f9b055689f4209a6a04289e719bd49daaf1167742e4e9c67b4');
  await p.locator('dialog[open]').getByRole('button', { name: /הצטרפות/ }).last().click();
  await p.waitForTimeout(3000);
  await shot('11-join-searching');
  log('join dialog:', (await p.locator('dialog[open]').innerText().catch(() => 'none')).slice(0, 400));
  await p.waitForTimeout(25000);
  await shot('12-join-after-30s');
  log('join after 30s:', (await p.locator('dialog[open]').innerText().catch(() => 'none')).slice(0, 400));
}

await app.close();
