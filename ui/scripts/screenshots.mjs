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

// Problems found while driving the built app; any of them fails the run.
const problems = [];

async function page(query, { width = 1366, height = 800, colorScheme = 'light', phone = false } = {}) {
  const ctx = await browser.newContext({
    viewport: { width, height },
    colorScheme,
    locale: 'he-IL',
    ...(phone ? { isMobile: true, hasTouch: true, deviceScaleFactor: 2 } : {}),
  });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  // The build carries a Content-Security-Policy; the app must run without breaking it.
  p.on('console', (m) => {
    if (m.type() === 'error' && /Content.Security.Policy/i.test(m.text())) problems.push(`CSP: ${m.text()}`);
  });
  await p.goto(BASE + query);
  return p;
}

const shot = async (p, name) => {
  await p.waitForTimeout(250);
  // Nothing may stick out sideways, in the page or in an open dialog.
  const overflow = await p.evaluate(() =>
    [document.documentElement, ...document.querySelectorAll('dialog[open]')].map((el) => el.scrollWidth - el.clientWidth).find((d) => d > 1),
  );
  if (overflow) problems.push(`${name}: content is ${overflow}px wider than its box`);
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
  await shot(p, '80-words-hidden-he');
  // Hidden words must not be in the page at all, and "I wrote them" waits for the reveal.
  if ((await p.locator('.word-grid .word:not(.word-mask)').count()) > 0) problems.push('recovery words in the page before reveal');
  if ((await p.getByRole('button', { name: 'כתבתי אותן' }).getAttribute('aria-disabled')) !== 'true') problems.push('"I wrote them" enabled before reveal');
  await p.getByRole('button', { name: 'הצגת המילים' }).click();
  await shot(p, '81-words-shown-he');
  const shownWords = await p.locator('.word-grid .word').allTextContents();
  await p.getByRole('button', { name: 'כתבתי אותן' }).click();
  await shot(p, '07-onboarding-check-words');
  // One right word, two wrong: only the wrong ones are marked (B19), and nothing is checked yet.
  const asked = (await p.locator('.check-fields label span').allTextContents()).map((s) => Number(s.match(/\d+/)[0]));
  const fields = p.locator('.check-fields input');
  await fields.nth(0).fill(shownWords[asked[0] - 1]);
  await fields.nth(1).fill('wrong');
  await fields.nth(2).fill('wrong');
  await p.getByRole('button', { name: 'בדיקה', exact: true }).click();
  await p.getByRole('alert').waitFor();
  await shot(p, '83-check-one-wrong-he');
  const invalid = await fields.evaluateAll((els) => els.map((e) => e.getAttribute('aria-invalid') === 'true'));
  if (invalid.join() !== 'false,true,true') problems.push(`check marks the wrong fields: ${invalid.join()}`);
  await p.getByRole('button', { name: 'אעשה את זה אחר כך' }).click();
  await p.getByLabel('שם תצוגה').fill('מורן');
  await shot(p, '08-onboarding-profile');
  await p.getByRole('button', { name: 'סיום' }).click();
  await p.getByRole('heading', { name: 'ברוכים הבאים' }).waitFor();
  await shot(p, '09-home-no-servers-reminder-banner');
  // The recovery words must not be anywhere in the page's storage.
  const stored = await p.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  const storedWords = new Set(stored.toLowerCase().split(/[^a-z]+/));
  for (const w of shownWords) if (storedWords.has(w)) problems.push(`recovery word "${w}" found in storage`);
  if (shownWords.length !== 12) problems.push(`expected 12 recovery words on screen, saw ${shownWords.length}`);
  await p.getByRole('button', { name: /יצירת שרת/ }).click();
  await shot(p, '10-create-server-needs-word-check');
  await p.close();
}

// ---- recovery words, more states (B19 spec §9) --------------------------------
{
  const p = await page('?demo=unchecked&still=1&lang=en&theme=light&update=999999');
  await p.getByRole('button', { name: 'Check now' }).click();
  await p.getByRole('button', { name: 'Show the words again' }).click();
  await p.getByRole('button', { name: 'Show the words' }).click();
  await p.locator('.word-grid .word:not(.word-mask)').first().waitFor();
  await shot(p, '82-words-shown-en-light');
  await p.close();
}

{
  const p = await page('?demo=unchecked&still=1&lang=he&theme=light&update=999999', { width: 375, height: 812, phone: true });
  await p.getByRole('button', { name: 'לבדיקה עכשיו' }).click();
  await p.getByRole('button', { name: 'להציג את המילים שוב' }).click();
  await p.getByRole('button', { name: 'הצגת המילים' }).click();
  await p.locator('.word-grid .word:not(.word-mask)').first().waitFor();
  await p.locator('.word-grid').scrollIntoViewIfNeeded();
  await shot(p, '84-words-phone-he');
  await p.close();
}

{
  const p = await page('?demo=unchecked&still=1&lang=he&theme=light&update=999999', { height: 768 });
  await p.evaluate(() => localStorage.setItem('mvp.settings.v1', JSON.stringify({ uiScale: 2, fontSize: 24 })));
  await p.reload();
  await p.getByRole('button', { name: 'לבדיקה עכשיו' }).click();
  const fields = p.getByLabel(/^מילה /);
  await fields.first().waitFor();
  const xs = await fields.evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().left)));
  if (new Set(xs).size !== 1) problems.push(`85: check fields are not in one column at 200%: ${xs.join()}`);
  await shot(p, '85-check-200-he');
  await p.evaluate(() => localStorage.removeItem('mvp.settings.v1'));
  await p.close();
}

{
  const p = await page('?demo=unchecked&still=1&lang=he&theme=light&update=999999');
  await p.getByRole('button', { name: 'לבדיקה עכשיו' }).click();
  await p.getByRole('button', { name: 'להציג את המילים שוב' }).click();
  await p.getByRole('button', { name: 'הצגת המילים' }).waitFor();
  await shot(p, '86-check-dialog-show-words');
  await p.close();
}

{
  const p = await page('?demo=unchecked&wordsError=1&still=1&lang=he&theme=light&update=999999');
  await p.getByRole('button', { name: 'לבדיקה עכשיו' }).click();
  await p.getByRole('button', { name: 'להציג את המילים שוב' }).click();
  await p.getByRole('button', { name: 'הצגת המילים' }).click();
  await p.getByRole('button', { name: 'לנסות שוב' }).waitFor();
  await shot(p, '87-words-load-error-he');
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
  // QA-10: a paste over the limit stays whole, the counter shows, sending stops.
  await p.getByRole('textbox', { name: /^Message / }).fill('word '.repeat(840));
  await shot(p, '29-chat-composer-too-long');
  if (!(await p.getByRole('button', { name: 'Send' }).isDisabled())) problems.push('send enabled past the length limit');
  await p.getByRole('textbox', { name: /^Message / }).fill('');
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

// ---- security demo: what other people could send ---------------------------
{
  const p = await chat('?demo=1&hostile=1&still=1&lang=he&theme=light&update=999999', 'x');
  await shot(p, '40-hostile-input-chat');
  await p.getByRole('button', { name: /הפרופיל של \u2068?מודן/ }).click();
  await shot(p, '41-hostile-lookalike-profile');
  const leaked = await p.evaluate(() => ({
    images: document.querySelectorAll('.message-canvas img').length,
    links: document.querySelectorAll('.message-canvas a').length,
    tracker: document.documentElement.outerHTML.includes('tracker.invalid'),
  }));
  if (leaked.images || leaked.links || leaked.tracker) problems.push(`hostile input leaked: ${JSON.stringify(leaked)}`);
  await p.close();
}

// ---- mixed direction (B17, spec docs/ux/specs/B17-mixed-direction.md §12) ---
// ?bidi=1 adds the 9 messages of the spec's §2 table to "כללי".
const BIDI_EXPECTED = [
  ['lol זה היה מטורף', 'rtl'],
  ['GG על המשחק אתמול, 3:1 לנו!', 'rtl'],
  ['@Itamar תבדוק את ה-patch החדש', 'rtl'],
  ['version 2.4.1 fixed the crash', 'ltr'],
  ['https://youtu.be/abcdefg תראו את זה', 'rtl'],
  ['\u200fok', 'rtl'],
  ['שלום לכולם', 'rtl'],
  ['npm run build', 'ltr'],
];
async function checkBidi(p, name, lang, partial = false) {
  const lines = await p.evaluate(() =>
    [...document.querySelectorAll('.msg-line')].map((el) => ({
      text: el.firstChild?.textContent ?? '',
      dir: el.getAttribute('dir'),
      align: getComputedStyle(el).textAlign,
    })),
  );
  const side = lang === 'he' ? 'right' : 'left';
  for (const [text, dir] of BIDI_EXPECTED) {
    const line = lines.find((l) => l.text === text);
    if (!line) {
      if (!partial) problems.push(`${name}: no message line "${text}"`);
    } else if (line.dir !== dir || line.align !== side) problems.push(`${name}: "${text}" is ${line.dir}/${line.align}, expected ${dir}/${side}`);
  }
  for (const text of ['🎉🎉', '3:1']) {
    const line = lines.find((l) => l.text === text);
    if (line && line.dir !== (lang === 'he' ? 'rtl' : 'ltr')) problems.push(`${name}: "${text}" should follow the interface`);
  }
}

{
  const p = await chat('?demo=1&bidi=1&still=1&lang=he&theme=dark&update=999999', 'x', { colorScheme: 'dark' });
  await checkBidi(p, '70-bidi-he-dark', 'he');
  await shot(p, '70-bidi-he-dark');
  await p.close();
}

{
  const p = await chat('?demo=1&bidi=1&still=1&lang=en&theme=light&update=999999', 'x');
  await checkBidi(p, '71-bidi-en-light', 'en');
  await shot(p, '71-bidi-en-light');
  await p.close();
}

{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.getByRole('button', { name: /^clips/ }).click();
  const box = p.getByRole('textbox', { name: /כתיבה ב/ });
  await box.waitFor();
  const placeholder = await box.getAttribute('placeholder');
  if (placeholder !== 'כתיבה ב-#\u2068clips\u2069') problems.push(`72: composer placeholder is "${placeholder}"`);
  await shot(p, '72-composer-he-latin-channel');
  await box.fill('GG על המשחק');
  if ((await box.getAttribute('dir')) !== 'rtl') problems.push('73: composer with "GG על המשחק" is not RTL');
  await shot(p, '73-composer-he-typing');
  await p.close();
}

for (const [section, name] of [
  ['appearance', '74-settings-zoom-hint-he'],
  ['storage', '75-settings-storage-he'],
]) {
  const p = await chat(`?demo=1&still=1&lang=he&theme=light&update=999999&settings=${section}`, 'x');
  await p.getByRole('button', { name: 'הגדרות' }).first().click();
  if (section === 'appearance') {
    const keys = await p.locator('kbd').allTextContents();
    if (keys.join(' ') !== 'Ctrl+= Ctrl+- Ctrl+0') problems.push(`74: shortcut keys are ${JSON.stringify(keys)}`);
    await p.locator('kbd').first().scrollIntoViewIfNeeded();
  }
  await shot(p, name);
  await p.close();
}

{
  const p = await page('?demo=1&bidi=1&still=1&lang=he&theme=light&update=999999', { height: 768 });
  await p.evaluate(() => localStorage.setItem('mvp.settings.v1', JSON.stringify({ uiScale: 2, fontSize: 24 })));
  await p.reload();
  await p.getByRole('article').first().waitFor();
  await p.waitForTimeout(800);
  await checkBidi(p, '76-bidi-he-200', 'he', true);
  // B12: at 200% the window lays out like 683px, so the member list is closed and the chat keeps its room.
  const room = await p.evaluate(() => ({ main: document.querySelector('.main')?.clientWidth ?? 0, members: !!document.querySelector('.members') }));
  if (room.main < 600 || room.members) problems.push(`76: chat is ${room.main}px wide, member list open: ${room.members}`);
  await shot(p, '76-bidi-he-200');
  await p.evaluate(() => localStorage.removeItem('mvp.settings.v1'));
  await p.close();
}

{
  const p = await page('?demo=1&bidi=1&still=1&lang=he&theme=dark&update=999999', { width: 375, height: 812, phone: true, colorScheme: 'dark' });
  await p.getByRole('button', { name: /^כללי/ }).click();
  await p.getByRole('article').first().waitFor();
  await p.mouse.move(1, 1);
  await p.waitForTimeout(600);
  await checkBidi(p, '77-bidi-he-phone', 'he', true);
  await shot(p, '77-bidi-he-phone');
  await p.close();
}

{
  const p = await chat('?demo=1&bidi=1&still=1&lang=he&theme=light&update=999999', 'x');
  const heads = p.locator('.msg-head').last();
  await heads.scrollIntoViewIfNeeded();
  const box = await p.locator('.message-scroll').boundingBox();
  await p.waitForTimeout(250);
  await p.screenshot({ path: `${OUT}/78-message-head-code.png`, clip: { x: box.x, y: Math.max(box.y, box.y + box.height - 420), width: box.width, height: Math.min(420, box.height) } });
  console.log('saved', '78-message-head-code');
  await p.close();
}

// ---- phone, 375 px ------------------------------------------------------------
const PHONE = { width: 375, height: 812, phone: true };
{
  const p = await page('?fresh=1&lang=he&theme=light', PHONE);
  await p.getByRole('heading', { name: 'בחירת שפה' }).waitFor();
  await shot(p, '50-phone-he-onboarding-language');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('checkbox').check();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByLabel('יום').fill('12');
  await p.getByLabel('חודש').fill('4');
  await p.getByLabel('שנה').fill('2001');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('heading', { name: 'מילות השחזור שלך' }).waitFor();
  await p.getByRole('button', { name: 'הצגת המילים' }).click();
  await shot(p, '51-phone-he-recovery-words');
  await p.close();
}

for (const [lang, t] of [
  ['he', { general: /^כללי/, members: 'רשימת החברים', back: 'חזרה לערוצים', close: 'סגירת רשימת החברים', settings: 'הגדרות', home: 'בית' }],
  ['en', { general: /^כללי/, members: 'Member list', back: 'Back to channels', close: 'Close the member list', settings: 'Settings', home: 'Home' }],
]) {
  const n = lang === 'he' ? 52 : 60;
  const p = await page(`?demo=unchecked&still=1&lang=${lang}&theme=light&update=999999`, PHONE);
  await p.getByRole('button', { name: t.general }).waitFor();
  await shot(p, `${n}-phone-${lang}-channels`);
  await p.getByRole('button', { name: t.general }).click();
  await p.getByRole('article').first().waitFor();
  await p.mouse.move(1, 1); // no leftover hover from the tap
  await p.waitForTimeout(600);
  await shot(p, `${n + 1}-phone-${lang}-chat`);
  await p.getByRole('button', { name: t.members }).click();
  await shot(p, `${n + 2}-phone-${lang}-members`);
  await p.getByRole('button', { name: t.close }).click();
  await p.getByRole('button', { name: t.back }).click();
  await p.getByRole('button', { name: t.settings }).first().click();
  await shot(p, `${n + 3}-phone-${lang}-settings`);
  await p.keyboard.press('Escape');
  await p.getByRole('button', { name: t.home, exact: true }).click();
  await shot(p, `${n + 4}-phone-${lang}-home`);
  await p.close();
}

{
  const p = await page('?demo=1&still=1&lang=he&theme=dark&update=999999', { ...PHONE, colorScheme: 'dark' });
  await p.getByRole('button', { name: /^כללי/ }).click();
  await p.getByRole('article').first().waitFor();
  await p.mouse.move(1, 1);
  await p.waitForTimeout(600);
  await shot(p, '57-phone-he-chat-dark');
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

if (problems.length) {
  console.error(`${problems.length} problem(s):`);
  for (const x of problems) console.error(`  ${x}`);
  process.exit(1);
}
console.log('no CSP violations, no page errors, no recovery words in storage');
