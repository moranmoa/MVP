// M1 "talks to itself": the built UI in the Electron shell with the REAL core
// (mvs-core). Creates an identity through onboarding, creates a server, posts
// two messages, restarts the app and checks the messages are still there.
// Screenshots go to ui/screenshots/20x-real-core-*.png.
//
// Runs in desktop's real-core container (a session D-Bus and an unlocked
// gnome-keyring for the core's keychain). Build ui/dist and desktop/.core/mvs-core
// first; see the PR note for the command.

import { createRequire } from 'node:module';

const require = createRequire('/repo/desktop/package.json');
const { _electron: electron } = require('playwright-core');

const OUT = '/repo/ui/screenshots';
const problems = [];
const say = (...a) => console.log('[real-core]', ...a);

async function launch() {
  const app = await electron.launch({ args: ['.'], cwd: '/repo/desktop', env: { ...process.env } });
  const page = await app.firstWindow();
  await page.setViewportSize({ width: 1366, height: 800 }).catch(() => {});
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  return { app, page };
}

const shot = async (page, name) => {
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${OUT}/${name}.png` });
  say('saved', name);
};

let { app, page } = { app: null, page: null };
// On a failed step: what the page showed, for the PR note.
process.on('uncaughtException', async (e) => {
  console.error('[real-core] FAILED:', e.message.split('\n')[0]);
  try {
    console.error('[real-core] page text:', (await page.evaluate(() => document.body.innerText)).slice(0, 1500));
    await page.screenshot({ path: `${OUT}/99-real-core-failure.png` });
  } catch {}
  process.exit(1);
});

// ---- first run: identity, server, two messages -------------------------------
({ app, page } = await launch());
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
say('words shown:', words.length);
await page.getByRole('button', { name: 'כתבתי אותן' }).click();
const fields = page.getByLabel(/^מילה מספר \d+$/);
await fields.first().waitFor();
for (const f of await fields.all()) {
  const n = Number((await f.evaluate((el) => el.labels[0].textContent)).match(/\d+/)[0]);
  await f.fill(words[n - 1]);
}
await page.getByRole('button', { name: 'בדיקה', exact: true }).click();
await page.getByLabel('שם תצוגה').waitFor({ timeout: 30_000 });
await page.getByLabel('שם תצוגה').fill('מורן');
await page.getByRole('button', { name: 'סיום' }).click();
await page.getByRole('heading', { name: 'ברוכים הבאים' }).waitFor({ timeout: 30_000 });
await shot(page, '200-real-core-home');

await page.getByRole('button', { name: /יצירת שרת/ }).click();
await page.getByLabel('שם השרת').fill('ערב בדיקות');
await page.getByLabel(/בלי הגבלה/).check();
await page.getByRole('button', { name: 'יצירה' }).click();
await page.getByRole('heading', { name: 'השרת מוכן' }).waitFor({ timeout: 30_000 });
await page.getByRole('button', { name: 'מעבר לשרת' }).click();

const box = page.getByRole('textbox', { name: /^כתיבה ב-/ });
await box.waitFor({ timeout: 30_000 });
for (const text of ['ההודעה הראשונה דרך הליבה האמיתית', 'lol זה עובד, וגם שומר אחרי הפעלה מחדש']) {
  await box.fill(text);
  await box.press('Enter');
  await page.getByText(text).waitFor({ timeout: 30_000 });
}
const alert = await page.locator('#composer-status[role=alert]').textContent().catch(() => null);
if (alert) problems.push(`composer error after sending: ${alert}`);
await shot(page, '201-real-core-chat');
await app.close();

// ---- second run: the messages are still there ------------------------------------
({ app, page } = await launch());
await page.getByRole('button', { name: /ערב בדיקות/ }).first().click({ timeout: 30_000 });
for (const text of ['ההודעה הראשונה דרך הליבה האמיתית', 'lol זה עובד, וגם שומר אחרי הפעלה מחדש']) {
  await page.getByText(text).waitFor({ timeout: 30_000 }).catch(() => problems.push(`after restart: "${text}" missing`));
}
await shot(page, '202-real-core-after-restart');
await app.close();

if (problems.length) {
  console.error(problems.join('\n'));
  process.exit(1);
}
say('ok: identity, server, 2 messages, restart, messages still there');
