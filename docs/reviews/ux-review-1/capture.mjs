// How to run (from ui/, with the dev server up via `docker compose up ui`):
//   docker run --rm --network <compose network> -e BASE=http://<ui container address>:5173/ \
//     -v <this folder>:/work -v <ui node_modules volume>:/work/node_modules -v <out folder>:/out \
//     -w /work mcr.microsoft.com/playwright:v1.56.1-noble node capture.mjs
// Vite only accepts localhost or an IP as the host, so BASE must use the container's IP.
// Annotated screenshots for docs/reviews/ux-review-1. Runs in the Playwright
// container against the dev server (BASE), writes PNGs to OUT.
import { chromium } from 'playwright';
import { mkdirSync } from 'node:fs';

const BASE = process.env.BASE ?? 'http://uxreview-ui:5173/';
const OUT = process.env.OUT ?? '/out';
mkdirSync(OUT, { recursive: true });
const browser = await chromium.launch();

async function open(query, { width = 1366, height = 800, scheme = 'dark', dpr = 1, storage } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height }, colorScheme: scheme, locale: 'he-IL', deviceScaleFactor: dpr });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => console.error('page error:', e.message));
  if (storage) {
    await p.goto(BASE);
    await p.evaluate((s) => { for (const [k, v] of Object.entries(s)) localStorage.setItem(k, v); }, storage);
  }
  await p.goto(BASE + query);
  return p;
}

// Draw numbered red boxes over elements (locators) or rects.
async function mark(p, items) {
  const rects = [];
  for (const it of items) {
    let r = it.rect;
    if (!r) {
      const b = await it.loc.first().boundingBox();
      if (!b) { console.warn('no box for', it.n); continue; }
      r = b;
    }
    rects.push({ ...r, n: it.n, pad: it.pad ?? 4 });
  }
  await p.evaluate((rects) => {
    for (const r of rects) {
      const d = document.createElement('div'); d.className = 'rv-mark';
      Object.assign(d.style, {
        position: 'fixed', left: r.x - r.pad + 'px', top: r.y - r.pad + 'px', width: r.width + 2 * r.pad + 'px', height: r.height + 2 * r.pad + 'px',
        border: '3px solid #ff2d55', borderRadius: '6px', zIndex: 99999, pointerEvents: 'none', boxSizing: 'border-box',
      });
      const tag = document.createElement('div');
      tag.textContent = r.n;
      Object.assign(tag.style, {
        position: 'absolute', top: '-14px', left: '-14px', width: '24px', height: '24px', borderRadius: '50%', background: '#ff2d55',
        color: '#fff', font: 'bold 14px/24px Arial', textAlign: 'center', direction: 'ltr',
      });
      d.appendChild(tag);
      document.body.appendChild(d);
    }
  }, rects);
}

const shot = async (p, name, clip) => {
  await p.waitForTimeout(300);
  await p.screenshot({ path: `${OUT}/${name}.png`, clip });
  console.log('saved', name);
};
const unmark = (p) => p.evaluate(() => document.querySelectorAll('.rv-mark').forEach((e) => e.remove()));

// ---- 1: fresh onboarding: recovery words grid, check overflow, skipped profile, new server -------------
{
  const p = await open('?fresh=1&lang=he&theme=dark');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('checkbox').check();
  await p.getByRole('button', { name: 'המשך' }).click();
  await mark(p, [{ n: 1, loc: p.getByRole('combobox') }]);
  await shot(p, '03-country-default-us');
  await unmark(p);
  await p.getByRole('button', { name: 'המשך' }).click();
  await mark(p, [{ n: 1, loc: p.locator('.dob .field').first() }]);
  await shot(p, '04-dob-day-misaligned');
  await unmark(p);
  await p.getByLabel('יום').fill('12');
  await p.getByLabel('חודש').fill('4');
  await p.getByLabel('שנה').fill('2001');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('heading', { name: 'מילות השחזור שלך' }).waitFor();
  const words = await p.$$eval('.word-grid .word', (els) => els.map((e) => e.textContent.trim()));
  const cells = p.locator('.word-num');
  await mark(p, [
    { n: 1, loc: cells.nth(0).locator('..') },
    { n: 2, loc: cells.nth(1).locator('..') },
    { n: 3, loc: cells.nth(2).locator('..') },
  ]);
  await shot(p, '01-recovery-words-order');
  await unmark(p);
  await p.getByRole('button', { name: 'כתבתי אותן' }).click();
  const labels = await p.$$eval('.check-fields .field span', (els) => els.map((e) => Number(e.textContent.match(/\d+/)[0])));
  const inputs = p.locator('.check-fields input');
  for (let i = 0; i < labels.length; i++) await inputs.nth(i).fill(i === 1 ? 'wrongword' : words[labels[i] - 1]);
  await p.getByRole('button', { name: 'בדיקה', exact: true }).click();
  await p.getByRole('alert').waitFor();
  await mark(p, [
    { n: 1, loc: p.locator('.check-fields') },
    { n: 2, loc: inputs.nth(0) },
    { n: 3, loc: inputs.nth(2) },
  ]);
  await shot(p, '02-check-words-overflow');
  await unmark(p);
  await inputs.nth(1).fill(words[labels[1] - 1]);
  await p.getByRole('button', { name: 'בדיקה', exact: true }).click();
  await p.getByRole('heading', { name: 'איך יקראו לך?' }).waitFor();
  await p.getByRole('button', { name: 'דילוג' }).click();
  await p.getByRole('heading', { name: 'ברוכים הבאים' }).waitFor();
  await p.getByRole('button', { name: /יצירת שרת/ }).click();
  await p.getByLabel('שם השרת').fill('Valorant ישראל');
  await mark(p, [{ n: 1, loc: p.locator('fieldset label').nth(0) }, { n: 2, loc: p.locator('fieldset label').nth(1) }]);
  await shot(p, '05-create-age-options');
  await unmark(p);
  await p.getByLabel('16 ומעלה').check();
  await p.getByRole('button', { name: 'יצירה' }).click();
  await p.getByRole('heading', { name: 'השרת מוכן' }).waitFor();
  await mark(p, [{ n: 1, loc: p.locator('dialog[open] .code').first() }]);
  await shot(p, '06-server-ready-link');
  await unmark(p);
  await p.getByRole('button', { name: 'מעבר לשרת' }).click();
  await p.waitForTimeout(500);
  await mark(p, [
    { n: 1, loc: p.locator('.offline-banner') },
    { n: 2, loc: p.locator('.channel').first() },
    { n: 3, loc: p.locator('textarea') },
    { n: 4, loc: p.locator('.rail-button').nth(1) },
    { n: 5, loc: p.locator('.me-panel') },
  ]);
  await shot(p, '07-new-server-first-run');
  await p.close();
}

// ---- 2: chat, mixed direction, codes, actions, update pill -------------------------
{
  const p = await open('?demo=1&still=1&lang=he&theme=dark&update=300');
  await p.getByRole('article').first().waitFor();
  await p.waitForTimeout(800);
  const box = p.locator('textarea');
  await box.fill('GG על המשחק אתמול, 3:1 לנו!');
  await box.press('Enter');
  await box.fill('@Itamar תבדוק את ה-patch החדש');
  await box.press('Enter');
  await p.waitForTimeout(1500);
  const bodies = p.locator('.msg-body');
  const n = await bodies.count();
  await mark(p, [
    { n: 1, loc: bodies.nth(n - 2) },
    { n: 2, loc: bodies.nth(n - 1) },
    { n: 3, loc: p.locator('.msg-body', { hasText: 'lol' }).last() },
    { n: 4, loc: p.locator('.msg-head .code').last() },
  ]);
  await shot(p, '08-chat-mixed-direction');
  await unmark(p);
  const art = p.getByRole('article').nth(-3);
  await art.hover();
  await mark(p, [{ n: 1, loc: art.locator('.msg-actions') }]);
  await shot(p, '09-message-actions-position', { x: 240, y: 380, width: 820, height: 420 });
  await unmark(p);
  await p.locator('.update-pill').waitFor();
  await mark(p, [{ n: 1, loc: p.locator('.update-pill') }]);
  await shot(p, '10-update-pill', { x: 1040, y: 560, width: 326, height: 240 });
  await unmark(p);
  // Offline server: nothing in the rail says so.
  await p.getByRole('button', { name: /^החבר׳ה/ }).click();
  await box.fill('מישהו פה?');
  await box.press('Enter');
  await p.waitForTimeout(600);
  await mark(p, [
    { n: 1, loc: p.locator('.rail-button').nth(3) },
    { n: 2, loc: p.locator('.offline-banner') },
    { n: 3, loc: p.locator('.send-state').last() },
  ]);
  await shot(p, '11-offline-server');
  await p.close();
}

// ---- 3: English UI with Hebrew messages -----------------------------------
{
  const p = await open('?demo=1&still=1&lang=en&theme=light&update=999999', { scheme: 'light' });
  await p.getByRole('article').first().waitFor();
  await p.waitForTimeout(800);
  await mark(p, [
    { n: 1, loc: p.locator('.msg-body', { hasText: 'מישהו בא' }).first() },
    { n: 2, loc: p.locator('.msg-body', { hasText: 'lol' }).first() },
  ]);
  await shot(p, '12-english-ui-hebrew-messages');
  await p.close();
}

// ---- 4: UI scale extremes, light theme contrast -------------------------------
for (const [scale, w, h] of [[1.5, 1280, 720], [0.8, 1366, 800]]) {
  const p = await open('?demo=1&still=1&lang=he&theme=light&update=999999', {
    width: w, height: h, scheme: 'light',
    storage: { 'mvp.settings.v1': JSON.stringify({ locale: 'he', theme: 'light', uiScale: scale, fontSize: 15 }) },
  });
  await p.getByRole('article').first().waitFor();
  await p.waitForTimeout(800);
  await shot(p, `13-scale-${Math.round(scale * 100)}-${w}x${h}`);
  await p.close();
}

// ---- 5: settings zoom hint, 2x -------------------------------------------
{
  const p = await open('?demo=1&still=1&lang=he&theme=light&update=999999', { scheme: 'light', dpr: 2 });
  await p.getByRole('article').first().waitFor();
  await p.getByRole('button', { name: 'הגדרות' }).first().click();
  await p.getByRole('button', { name: 'מראה' }).click();
  const hint = p.getByText('קיצורים:', { exact: false });
  const b = await hint.boundingBox();
  await shot(p, '14-settings-zoom-hint', { x: b.x - 20, y: b.y - 10, width: b.width + 40, height: b.height + 20 });
  await p.getByRole('button', { name: 'חשבון' }).click();
  await mark(p, [{ n: 1, loc: p.locator('.settings-panel') }]);
  await shot(p, '15-settings-account');
  await p.close();
}

await browser.close();
