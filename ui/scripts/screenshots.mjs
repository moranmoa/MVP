// Captures the main screens into ./screenshots. Runs inside the Playwright
// container (see compose.yaml): `docker compose run --rm shots`.
// Builds first, serves dist/ with `vite preview`, then drives the UI.

import { execSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';
import { preview } from 'vite';

const OUT = 'screenshots';
const PORT = 4173;
const BASE = `http://localhost:${PORT}/`;

mkdirSync(OUT, { recursive: true });
if (!process.argv.includes('--no-build')) execSync('npx vite build', { stdio: 'inherit' });
const server = await preview({ preview: { port: PORT, strictPort: true, host: '127.0.0.1' }, logLevel: 'warn' });
const browser = await chromium.launch();

async function page(query, { width = 1366, height = 800, colorScheme = 'light' } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height }, colorScheme, locale: 'he-IL' });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => console.error('page error:', e.message));
  await p.goto(BASE + query);
  return p;
}

const shot = async (p, name) => {
  await p.waitForTimeout(250);
  await p.screenshot({ path: `${OUT}/${name}.png` });
  console.log('saved', name);
};

// ---- onboarding (Hebrew) ----------------------------------------------------
{
  const p = await page('?fresh=1&lang=he&theme=light');
  await p.getByRole('heading', { name: 'בחירת שפה' }).waitFor();
  await shot(p, '01-onboarding-language');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('checkbox').check();
  await shot(p, '02-onboarding-agreement');
  await p.getByRole('button', { name: 'המשך' }).click();
  await shot(p, '03-onboarding-country');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByLabel('יום').fill('12');
  await p.getByLabel('חודש').fill('4');
  await p.getByLabel('שנה').fill('2001');
  await shot(p, '04-onboarding-birth-date');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.waitForTimeout(700);
  await shot(p, '05-onboarding-preparing');
  await p.getByRole('heading', { name: 'מילות השחזור שלך' }).waitFor();
  await shot(p, '06-onboarding-recovery-words');
  await p.getByRole('button', { name: 'כתבתי אותן' }).click();
  await shot(p, '07-onboarding-check-words');
  await p.getByRole('button', { name: 'אעשה את זה אחר כך' }).click();
  await p.getByLabel('שם תצוגה').fill('מורן');
  await shot(p, '08-onboarding-profile');
  await p.getByRole('button', { name: 'סיום' }).click();
  await p.getByRole('heading', { name: 'ברוכים הבאים' }).waitFor();
  await shot(p, '09-home-no-servers-reminder-banner');
  await p.getByRole('button', { name: /יצירת שרת/ }).click();
  await shot(p, '10-create-server-needs-word-check');
  await p.close();
}

// ---- under 16 ---------------------------------------------------------------
{
  const p = await page('?fresh=1&lang=he&theme=dark', { colorScheme: 'dark' });
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('checkbox').check();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByLabel('יום').fill('1');
  await p.getByLabel('חודש').fill('1');
  await p.getByLabel('שנה').fill(String(new Date().getFullYear() - 13));
  await p.getByRole('button', { name: 'המשך' }).click();
  await shot(p, '11-onboarding-under-16-dark');
  await p.close();
}

// ---- restore ----------------------------------------------------------------
{
  const p = await page('?fresh=1&lang=he&theme=light');
  await p.getByRole('button', { name: 'כבר יש לי חשבון' }).click();
  const words = 'apple river stone cloud tiger piano lemon rockett forest candle silver garden';
  await p.getByLabel('מילה 1', { exact: true }).focus();
  await p.evaluate((text) => {
    const dt = new DataTransfer();
    dt.setData('text', text);
    document.activeElement.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true }));
  }, words);
  await p.getByRole('button', { name: 'שחזור' }).click();
  await p.getByRole('alert').waitFor();
  await shot(p, '12-restore-with-typo');
  await p.close();
}

// ---- create & join (demo account, words checked) ----------------------------
{
  const p = await page('?demo=1&still=1&lang=he&theme=light&update=999999');
  await p.getByRole('button', { name: 'בית' }).click();
  await shot(p, '13-home-with-servers');
  await p.getByRole('button', { name: /יצירת שרת/ }).click();
  await p.getByLabel('שם השרת').fill('ערב משחקים');
  await p.getByRole('button', { name: 'יצירה' }).click();
  await shot(p, '14-create-server-age-required');
  await p.getByLabel('18 ומעלה').check();
  await p.getByRole('button', { name: 'יצירה' }).click();
  await p.getByRole('heading', { name: 'השרת מוכן' }).waitFor();
  await shot(p, '15-create-server-done');
  await p.getByRole('button', { name: 'מעבר לשרת' }).click();
  await p.getByRole('button', { name: 'בית' }).click();
  await p.getByRole('button', { name: /הצטרפות בלינק/ }).click();
  await p.getByLabel('לינק הזמנה').fill('mvp://join/valorant-7k2q');
  await p.getByRole('button', { name: 'הצטרפות', exact: true }).click();
  await shot(p, '16-join-searching');
  await p.getByRole('button', { name: 'ביטול' }).click();
  await p.getByRole('button', { name: /הצטרפות בלינק/ }).click();
  await p.getByLabel('לינק הזמנה').fill('mvp://join/offline-crew');
  await p.getByRole('button', { name: 'הצטרפות', exact: true }).click();
  await p.getByText('אף אחד מהשרת לא מחובר כרגע', { exact: false }).waitFor({ timeout: 5000 });
  await shot(p, '17-join-nobody-online');
  await p.close();
}

// ---- chat -------------------------------------------------------------------
async function chat(query, name, opts) {
  const p = await page(query, opts);
  await p.getByRole('article').first().waitFor();
  await p.waitForTimeout(800);
  return p;
}

{
  const p = await chat('?demo=1&still=1&lang=he&theme=dark&update=300', 'x', { colorScheme: 'dark' });
  const box = p.getByRole('textbox', { name: /כתיבה ב/ });
  // Reply to the last message.
  const last = p.getByRole('article').last();
  await last.hover();
  await last.getByRole('button', { name: 'תגובה' }).click();
  await box.fill('אני בפנים! מי מארח?');
  await box.press('Enter');
  await box.fill('this one will fail /fail');
  await box.press('Enter');
  await box.fill('ערכתי את ההודעה הזו');
  await box.press('Enter');
  await p.waitForTimeout(1500);
  await box.press('ArrowUp');
  await box.fill('ערכתי את ההודעה הזו – עכשיו עם תיקון');
  await box.press('Enter');
  await p.waitForTimeout(600);
  await shot(p, '20-chat-hebrew-dark-send-states');
  await p.getByRole('article').last().hover();
  await shot(p, '21-chat-message-actions');
  await p.getByRole('button', { name: /^החבר׳ה/ }).click();
  await box.fill('מישהו פה?');
  await box.press('Enter');
  await p.waitForTimeout(500);
  await shot(p, '22-chat-nobody-online-pending');
  await p.close();
}

{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=300', 'x');
  await shot(p, '23-chat-hebrew-light');
  await p.getByRole('button', { name: /הפרופיל של/ }).nth(3).click();
  await shot(p, '24-profile-card');
  await p.getByRole('button', { name: 'הרחקה מהשרת' }).click();
  await shot(p, '25-profile-kick-confirm');
  await p.getByRole('button', { name: 'הרחקה', exact: true }).click();
  await p.waitForTimeout(400);
  await shot(p, '26-kick-encryption-notice');
  await p.close();
}

{
  const p = await chat('?demo=1&still=1&lang=en&theme=light&update=300', 'x');
  await shot(p, '27-chat-english-ltr');
  await p.getByRole('button', { name: 'Member list' }).click();
  await shot(p, '28-chat-english-members-collapsed');
  await p.close();
}

// ---- settings ---------------------------------------------------------------
for (const [section, name] of [
  ['appearance', '30-settings-appearance'],
  ['language', '31-settings-language'],
  ['storage', '32-settings-storage'],
  ['diagnostics', '33-settings-diagnostics'],
  ['about', '34-settings-about'],
]) {
  const p = await chat(`?demo=unchecked&still=1&lang=he&theme=light&settings=${section}`, 'x');
  await p.getByRole('button', { name: 'הגדרות' }).first().click();
  if (section === 'appearance') {
    await p.getByRole('slider').nth(1).fill('20');
  }
  await shot(p, name);
  await p.close();
}

{
  const p = await chat('?demo=1&still=1&lang=he&theme=light', 'x');
  await p.keyboard.press('Control+=');
  await p.keyboard.press('Control+=');
  await p.keyboard.press('Control+=');
  await shot(p, '35-ui-scale-130');
  await p.close();
}

{
  const p = await chat('?demo=unchecked&still=1&lang=he&theme=dark', 'x', { colorScheme: 'dark' });
  await shot(p, '36-chat-recovery-reminder-banner');
  await p.close();
}

// ---- rough performance check (headless, software rendering: indicative only) --
{
  const p = await chat('?demo=1&still=1&lang=he&theme=dark', 'x', { colorScheme: 'dark' });
  const result = await p.evaluate(async () => {
    const el = document.querySelector('.message-scroll');
    const frame = () => new Promise((r) => requestAnimationFrame(r));
    const times = [];
    let t = performance.now();
    // Scroll up through the 100k-message channel, 60 px per frame, for 300 frames.
    for (let i = 0; i < 300; i++) {
      el.scrollTop -= 60;
      await frame();
      const now = performance.now();
      times.push(now - t);
      t = now;
    }
    // Jump to the middle of the channel.
    const j0 = performance.now();
    el.scrollTop = el.scrollHeight / 2;
    await frame();
    await frame();
    const jump = performance.now() - j0;
    // Switch server and back.
    const buttons = document.querySelectorAll('.rail .rail-button');
    const s0 = performance.now();
    buttons[2].click();
    await frame();
    const switchMs = performance.now() - s0;
    times.sort((a, b) => a - b);
    return {
      avgFrameMs: times.reduce((a, b) => a + b, 0) / times.length,
      p95FrameMs: times[Math.floor(times.length * 0.95)],
      rowsInDom: document.querySelectorAll('[role=article]').length,
      jumpToMiddleMs: jump,
      serverSwitchMs: switchMs,
    };
  });
  console.log('perf', JSON.stringify(result, null, 2));
  await p.close();
}

await browser.close();
await server.close();
