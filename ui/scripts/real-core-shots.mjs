// M1 with the REAL core (mvs-core) behind the Electron shell, driven through
// the built UI. Screenshots go to ui/screenshots/2xx-real-core-*.png (range
// 200–219, docs/ux/specs/README.md).
//
// Default ("talks to itself"): onboarding → create a server → two messages →
// restart → the messages are still there (200–202).
//
// MVS_TWO=1 ("talks to a friend"): two instances, A and B, each with its own
// HOME (so its own Electron data folder and core data dir) and its own D-Bus
// session and keyring (MVS_BUS_A / MVS_BUS_B). Both cores sync through a
// throwaway relay (MVS_CORE_PATH is a wrapper that adds --relay). A creates a
// server and posts; B joins by the invite link and sees A's message; B posts
// and A sees it (203–206). Run by ui/scripts/real-core-two.sh.
//
// Runs in desktop's real-core image. Build ui/dist and desktop/.core/mvs-core
// first; see the PR notes for the commands.

import { createRequire } from 'node:module';

const require = createRequire('/repo/desktop/package.json');
const { _electron: electron } = require('playwright-core');

const OUT = '/repo/ui/screenshots';
const SYNC_WAIT = 90_000; // the core's own sync loop, through the relay
const problems = [];
const say = (...a) => console.log('[real-core]', ...a);
const pages = new Set();

async function launch(env = {}) {
  const app = await electron.launch({ args: ['.'], cwd: '/repo/desktop', env: { ...process.env, ...env } });
  const page = await app.firstWindow();
  await page.setViewportSize({ width: 1366, height: 800 }).catch(() => {});
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  pages.add(page);
  return { app, page };
}

const shot = async (page, name) => {
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${OUT}/${name}.png` });
  say('saved', name);
};

// On a failed step: what each window showed, for the PR note.
process.on('uncaughtException', async (e) => {
  console.error('[real-core] FAILED:', e.message.split('\n')[0]);
  let i = 0;
  for (const page of pages) {
    try {
      console.error(`[real-core] window ${i} text:`, (await page.evaluate(() => document.body.innerText)).slice(0, 1200));
      await page.screenshot({ path: `${OUT}/219-real-core-failure-${i}.png` });
    } catch {}
    i++;
  }
  process.exit(1);
});

/** Onboarding in Hebrew, words checked, display name set; ends on Home. */
async function onboard(page, name) {
  // The container's locale is English; choose Hebrew like a person would.
  await page.getByRole('heading', { name: /Choose a language|בחירת שפה/ }).waitFor({ timeout: 30_000 });
  await page.getByText('עברית', { exact: true }).click();
  await page.getByRole('heading', { name: 'בחירת שפה' }).waitFor();
  await page.getByRole('button', { name: 'המשך' }).click();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'המשך' }).click();
  await page.getByRole('button', { name: 'המשך' }).click();
  await page.getByLabel('יום').fill('12');
  await page.getByLabel('חודש').fill('4');
  await page.getByLabel('שנה').fill('1998');
  await page.getByRole('button', { name: 'המשך' }).click();
  await page.getByRole('heading', { name: 'מילות השחזור שלך' }).waitFor({ timeout: 120_000 });
  await page.getByRole('button', { name: 'הצגת המילים' }).click();
  await page.locator('.word-grid .word:not(.word-mask)').first().waitFor();
  const words = await page.locator('.word-grid .word:not(.word-mask)').allTextContents();
  await page.getByRole('button', { name: 'כתבתי אותן' }).click();
  const fields = page.getByLabel(/^מילה מספר \d+$/);
  await fields.first().waitFor();
  for (const f of await fields.all()) {
    const n = Number((await f.evaluate((el) => el.labels[0].textContent)).match(/\d+/)[0]);
    await f.fill(words[n - 1]);
  }
  await page.getByRole('button', { name: 'בדיקה', exact: true }).click();
  await page.getByLabel('שם תצוגה').waitFor({ timeout: 30_000 });
  await page.getByLabel('שם תצוגה').fill(name);
  await page.getByRole('button', { name: 'סיום' }).click();
  await page.getByRole('heading', { name: 'ברוכים הבאים' }).waitFor({ timeout: 30_000 });
}

/** Creates a server from Home and opens it; returns its invite link. */
async function createServer(page, name) {
  await page.getByRole('button', { name: /יצירת שרת/ }).click();
  await page.getByLabel('שם השרת').fill(name);
  await page.getByLabel(/בלי הגבלה/).check();
  await page.getByRole('button', { name: 'יצירה' }).click();
  await page.getByRole('heading', { name: 'השרת מוכן' }).waitFor({ timeout: 30_000 });
  const link = (await page.locator('dialog[open]').innerText()).match(/mvs:\/\/join\/[0-9a-f]{64}\/[0-9a-f]{64}/)?.[0];
  if (!link) throw new Error('no invite link in the "server ready" dialog');
  await page.getByRole('button', { name: 'מעבר לשרת' }).click();
  return link;
}

async function post(page, text) {
  const box = page.getByRole('textbox', { name: /^כתיבה ב-/ });
  await box.waitFor({ timeout: 30_000 });
  await box.fill(text);
  await box.press('Enter');
  await page.getByText(text).waitFor({ timeout: 30_000 });
  const alert = await page.locator('#composer-status[role=alert]').textContent({ timeout: 500 }).catch(() => null);
  if (alert) problems.push(`composer error after "${text}": ${alert}`);
}

const FIRST = 'ההודעה הראשונה דרך הליבה האמיתית';
const SECOND = 'lol זה עובד, וגם שומר אחרי הפעלה מחדש';
const FROM_A = 'שלום מ-A, מישהו שומע?';
const FROM_B = 'היי A, כאן B דרך הממסר';

if (!process.env.MVS_TWO) {
  // ---- talks to itself ------------------------------------------------------------
  let { app, page } = await launch();
  await onboard(page, 'מורן');
  await shot(page, '200-real-core-home');
  await createServer(page, 'ערב בדיקות');
  await post(page, FIRST);
  await post(page, SECOND);
  await shot(page, '201-real-core-chat');
  await app.close();

  ({ app, page } = await launch());
  await page.getByRole('button', { name: /ערב בדיקות/ }).first().click({ timeout: 30_000 });
  for (const text of [FIRST, SECOND]) {
    await page.getByText(text).waitFor({ timeout: 30_000 }).catch(() => problems.push(`after restart: "${text}" missing`));
  }
  await shot(page, '202-real-core-after-restart');
  await app.close();
} else {
  // ---- talks to a friend: two instances through a relay -------------------------
  const envA = { HOME: '/tmp/mvs-a', DBUS_SESSION_BUS_ADDRESS: process.env.MVS_BUS_A };
  const envB = { HOME: '/tmp/mvs-b', DBUS_SESSION_BUS_ADDRESS: process.env.MVS_BUS_B };
  const a = await launch(envA);
  await onboard(a.page, 'מורן');
  const link = await createServer(a.page, 'ערב חברים');
  say('invite link ok');
  await post(a.page, FROM_A);
  await shot(a.page, '203-real-core-two-a-posted');

  const b = await launch(envB);
  await onboard(b.page, 'נועה');
  await b.page.getByRole('button', { name: /הצטרפות בלינק/ }).click();
  await b.page.getByLabel('לינק הזמנה').fill(link);
  await b.page.getByRole('button', { name: 'הצטרפות', exact: true }).click();
  await b.page.getByRole('textbox', { name: /^כתיבה ב-/ }).waitFor({ timeout: SYNC_WAIT });
  await b.page.getByText(FROM_A).waitFor({ timeout: SYNC_WAIT });
  await shot(b.page, '204-real-core-two-b-joined');

  await post(b.page, FROM_B);
  await a.page.getByText(FROM_B).waitFor({ timeout: SYNC_WAIT });
  await shot(a.page, '205-real-core-two-a-sees-b');
  await shot(b.page, '206-real-core-two-b-chat');
  await a.app.close();
  await b.app.close();
}

if (problems.length) {
  console.error(problems.join('\n'));
  process.exit(1);
}
say(process.env.MVS_TWO ? 'ok: B joined A by link, messages both ways' : 'ok: identity, server, 2 messages, restart, messages still there');
