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
// A fake microphone, so the voice engine (piece D) finds one in call shots.
const browser = await chromium.launch({ args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });

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
  // Since #290 the restore asks the agreement, country and birth date first.
  await p.getByRole('checkbox').check();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByLabel('יום').fill('12');
  await p.getByLabel('חודש').fill('4');
  await p.getByLabel('שנה').fill('2001');
  await p.getByRole('button', { name: 'המשך' }).click();
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
  ['notifications', '361-settings-notifications-he'],
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

// ---- B11c: channels (owner) ----------------------------------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const list = p.getByRole('navigation', { name: 'ערוצים' });
  await list.getByRole('button', { name: 'יצירת ערוץ' }).click();
  await p.getByLabel('שם הערוץ').fill('חדשות משחקים');
  const typed = await p.getByLabel('שם הערוץ').inputValue();
  if (typed !== 'חדשות-משחקים') problems.push(`116: channel name is "${typed}", expected spaces as dashes`);
  await shot(p, '116-channel-create-he');
  await p.getByRole('button', { name: 'ביטול' }).click();
  await list.getByRole('button', { name: 'עריכת הערוץ: כללי' }).click();
  if (await p.getByRole('button', { name: 'מחיקת הערוץ' }).count()) problems.push('117: #general offers delete');
  await shot(p, '117-channel-edit-general-he');
  await p.getByRole('button', { name: 'ביטול' }).click();
  await list.getByRole('button', { name: 'עריכת הערוץ: clips' }).click({ force: true });
  await p.getByRole('button', { name: 'מחיקת הערוץ' }).click();
  await shot(p, '118-channel-delete-confirm-he');
  await p.close();
}

// ---- B8: in-app update notes (190-193) ------------------------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&installed=1', 'x');
  await p.getByText(/עודכן לגרסה/).waitFor();
  await shot(p, '190-update-installed-notes-collapsed');
  await p.getByText('מה חדש').click();
  await shot(p, '191-update-installed-notes-expanded');
  await p.close();
}

{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&rolledBack=1', 'x');
  await p.getByText(/לא עלה כמו שצריך/).waitFor();
  await shot(p, '192-update-rolled-back');
  await p.close();
}

{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=300&inCall=1', 'x');
  await p.getByRole('button', { name: /עדכון מוכן – הפעלה מחדש/ }).click();
  await p.getByText('עדכון מוכן – יותקן כשהשיחה תסתיים').waitFor();
  await shot(p, '193-update-pill-after-call');
  await p.close();
}

// ---- T8-01: the core is not answering (220-222) -----------------------------------
for (const [detail, name] of [
  ['crash-loop', '220-core-down-crash-loop-he'],
  ['missing', '221-core-down-missing-he'],
  ['keyring', '222-core-down-keyring-he'],
]) {
  const p = await page(`?demo=1&still=1&lang=he&theme=light&update=999999&coreDown=${detail}`);
  await p.getByRole('heading', { name: 'MVS לא מצליחה לפעול כרגע' }).waitFor();
  await shot(p, name);
  await p.close();
}

// ---- connection type per online member (#196, T20) --------------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const list = p.locator('.members');
  if ((await list.locator('.connection-icon').count()) === 0) problems.push('230: no connection icons in the member list');
  const box = await list.boundingBox();
  await p.waitForTimeout(250);
  await p.screenshot({ path: `${OUT}/230-member-connection-he.png`, clip: box });
  console.log('saved', '230-member-connection-he');
  await p.close();
}

// ---- voice v1, pieces E and E2: the voice room and the call bar (240-249) -------
// Joining: the room entry, then the one-time IP notice (243-244, docs/ux/specs/voice-ip-notice.md).
async function joinRoom(p, ok = 'הבנתי') {
  await p.locator('.voice-room-entry').click();
  await p.getByRole('dialog').getByRole('button', { name: ok }).click();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.locator('.voice-room-people li').first().waitFor();
  await shot(p, '245-voice-room-he');
  await p.locator('.voice-room-entry').click();
  await p.getByRole('dialog').waitFor();
  await shot(p, '243-call-ip-notice-he');
  await p.getByRole('dialog').getByRole('button', { name: 'הבנתי' }).click();
  const bar = p.getByRole('region', { name: 'שיחה קולית' });
  await bar.waitFor();
  await bar.getByRole('button', { name: 'השתקה' }).click();
  await bar.locator('button[aria-pressed="true"]').waitFor();
  await shot(p, '240-call-bar-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&callError=relay-only', 'x');
  await joinRoom(p);
  await p.locator('.voice-room-reason').waitFor();
  await shot(p, '241-call-relay-only-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&roomFull=1', 'x');
  await p.locator('.voice-room-reason').waitFor();
  await shot(p, '242-call-full-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=en&theme=dark&update=999999', 'x', { colorScheme: 'dark' });
  await p.locator('.voice-room-people li').first().waitFor();
  await shot(p, '246-voice-room-en-dark');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=en&theme=light&update=999999', 'x');
  await p.locator('.voice-room-entry').click();
  await p.getByRole('dialog').waitFor();
  await shot(p, '244-call-ip-notice-en-light');
  await p.close();
}
for (const [sim, name, wait] of [
  ['noMic', '247-call-no-mic-he', '.call-no-mic'],
  ['peerFailed', '248-call-peer-failed-he', '.call-peer-failed'],
  ['peerRefused', '249-call-peer-refused-he', '.call-peer-failed.refused'],
]) {
  const p = await chat(`?demo=1&still=1&lang=he&theme=light&update=999999&voiceSim=${sim}`, 'x');
  await joinRoom(p);
  await p.locator(wait).waitFor();
  await p.waitForTimeout(300);
  await shot(p, name);
  await p.close();
}

// ---- #288: messages held for the channel key (280-289) ----------------------
for (const [mode, name, wait] of [
  ['wait', '280-held-sending-he', '.send-state.held.pending'],
  ['fail', '281-held-failed-he', '.send-state.held.failed'],
]) {
  const p = await chat(`?demo=1&still=1&lang=he&theme=light&update=999999&keyPending=${mode}`, 'x');
  const box = p.getByRole('textbox', { name: /כתיבה ב/ });
  await box.fill('הצטרפתי עכשיו, שלום לכולם');
  await box.press('Enter');
  await p.locator(wait).waitFor();
  await p.waitForTimeout(300);
  await shot(p, name);
  await p.close();
}

// ---- B11e: edit my profile in Settings → Account (90-99) --------------------
// 94 (onboarding, no Skip), 95 (profile card with bio and banner) and 96 (long
// name at 200%) wait for the onboarding change and the banner.
for (const [query, name, opts, steps] of [
  ['lang=he&theme=light', '90-account-he', {}, 'open'],
  ['lang=he&theme=light', '91-account-dirty-he', {}, 'dirty'],
  ['lang=en&theme=light', '92-account-en-light', {}, 'open'],
  ['lang=he&theme=light&profileError=1', '93-account-save-error-he', {}, 'error'],
  ['lang=he&theme=light', '97-account-phone-he', { width: 375, height: 812, phone: true }, 'dirty'],
]) {
  const p = opts.phone
    ? await page(`?demo=1&still=1&update=999999&settings=account&${query}`, opts)
    : await chat(`?demo=1&still=1&update=999999&${query}`, 'x', opts);
  if (opts.phone) {
    await p.getByRole('button', { name: 'הגדרות' }).first().click();
  } else {
    await p.locator('.me-profile').click();
  }
  const dialog = p.getByRole('dialog');
  const field = dialog.locator('.profile-editor input:not([type=file])');
  await field.waitFor();
  if (steps !== 'open') {
    await field.fill(name.includes('-en') ? 'Moran B' : 'מורן ב.');
    await dialog.locator('.profile-editor textarea').fill('גיימרית בלילות, מתכנתת בימים');
  }
  if (steps === 'error') {
    await dialog.locator('.profile-actions .primary').click();
    await dialog.locator('.profile-actions .error').waitFor();
  }
  await p.waitForTimeout(300);
  await shot(p, name);
  await p.close();
}

// ---- #304: sign out on this computer (300-309) ------------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.locator('.me-profile').click();
  const button = p.locator('.sign-out button.danger');
  await button.scrollIntoViewIfNeeded();
  await p.waitForTimeout(200);
  await shot(p, '300-sign-out-button-he');
  await button.click();
  const dialog = p.locator('.sign-out-dialog');
  await dialog.locator('.sign-out-facts').waitFor();
  await shot(p, '301-sign-out-confirm-he');
  await dialog.getByRole('button', { name: 'המשך' }).click();
  await dialog.locator('.word-inputs').waitFor();
  await shot(p, '302-sign-out-words-he');
  await dialog.getByRole('button', { name: 'אין לי את המילים' }).click();
  await dialog.locator('.field input').fill('יציאה בלי מילים');
  await shot(p, '303-sign-out-no-words-he');
  await dialog.getByRole('button', { name: 'יציאה בלי מילים' }).click();
  await p.getByText('כבר יש לי חשבון').waitFor();
  await p.waitForTimeout(300);
  await shot(p, '305-after-sign-out-he');
  await p.close();
}
{
  const p = await page('?demo=1&still=1&lang=he&theme=light&update=999999&settings=account', { width: 375, height: 812, phone: true });
  await p.getByRole('button', { name: 'הגדרות' }).first().click();
  const button = p.locator('.sign-out button.danger');
  await button.scrollIntoViewIfNeeded();
  await button.click();
  await p.locator('.sign-out-dialog .sign-out-facts').waitFor();
  await p.waitForTimeout(300);
  await shot(p, '304-sign-out-confirm-phone-he');
  await p.close();
}

// ---- B7 / #306: a birth-date correction waiting its 30 days (310-319) -------
for (const [name, opts] of [
  ['310-account-dob-pending-he', {}],
  ['311-account-dob-pending-phone-he', { width: 375, height: 812, phone: true }],
]) {
  const query = '?demo=1&still=1&lang=he&theme=light&update=999999&pendingDob=1&settings=account';
  const p = opts.phone ? await page(query, opts) : await chat(query, 'x');
  if (opts.phone) await p.getByRole('button', { name: 'הגדרות' }).first().click();
  else await p.locator('.me-profile').click();
  const pending = p.locator('.dob-pending');
  await pending.waitFor();
  await pending.scrollIntoViewIfNeeded();
  await p.waitForTimeout(300);
  await shot(p, name);
  await p.close();
}

// ---- G-01: reorder my servers in the rail (320-329) --------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const items = p.locator('.rail li[data-server-li]');
  const third = await items.nth(2).locator('button.rail-button').boundingBox();
  const first = await items.nth(0).locator('button.rail-button').boundingBox();
  await p.mouse.move(third.x + third.width / 2, third.y + third.height / 2);
  await p.mouse.down();
  await p.mouse.move(third.x + third.width / 2, third.y + third.height / 2 - 10, { steps: 2 });
  await p.mouse.move(first.x + first.width / 2 + 6, first.y + 6, { steps: 6 });
  await p.locator('.rail li.drop-before').waitFor();
  await shot(p, '320-server-order-drag-he');
  await p.mouse.up();
  await p.waitForTimeout(200);
  await shot(p, '321-server-order-dropped-he');
  await p.close();
}
{
  const p = await page('?demo=1&still=1&lang=he&theme=light&update=999999', { width: 375, height: 812, phone: true });
  const rail = p.locator('.rail');
  await rail.locator('button[data-server-id]').first().waitFor();
  await rail.locator('button[data-server-id]').first().dispatchEvent('contextmenu');
  await p.getByRole('menuitem', { name: 'שינוי סדר השרתים' }).click();
  await rail.locator('.rail-arrange').first().waitFor();
  await p.waitForTimeout(200);
  await shot(p, '322-server-order-phone-he');
  await p.close();
}

// ---- G-11: the owner reorders the channels (330-339) -------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const rows = p.locator('.channel-list li.channel-row');
  const third = rows.nth(2);
  await third.hover();
  const handle = await third.locator('.channel-handle').boundingBox();
  const first = await rows.nth(0).boundingBox();
  await p.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
  await p.mouse.down();
  await p.mouse.move(handle.x + handle.width / 2, handle.y - 10, { steps: 2 });
  await p.mouse.move(handle.x + 40, first.y + 6, { steps: 6 });
  await p.locator('.channel-row.drop-before').waitFor();
  await shot(p, '330-channel-order-drag-he');
  await p.mouse.up();
  await p.waitForTimeout(300);
  await shot(p, '331-channel-order-dropped-he');
  await p.close();
}
{
  const p = await page('?demo=1&still=1&lang=he&theme=light&update=999999', { width: 375, height: 812, phone: true });
  const row = p.locator('.channel-list li.channel-row button.channel').first();
  await row.waitFor();
  await row.dispatchEvent('contextmenu');
  await p.getByRole('menuitem', { name: 'שינוי סדר הערוצים' }).click();
  await p.locator('.channel-arrange').first().waitFor();
  await p.waitForTimeout(200);
  await shot(p, '332-channel-order-phone-he');
  await p.close();
}

// ---- G-21: where the new messages start (340-349) ----------------------------
for (const [channel, name, wait] of [
  [/^מחפשים-קבוצה/, '340-new-messages-divider-he', '.unread-divider'],
  [/^clips/, '341-new-messages-bar-he', '.unread-bar'],
]) {
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.getByRole('navigation', { name: 'ערוצים' }).getByRole('button', { name: channel }).click();
  await p.locator(wait).waitFor();
  await p.waitForTimeout(400);
  await shot(p, name);
  await p.close();
}

// ---- G-24: mentions (370-379) -------------------------------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const box = p.getByRole('textbox', { name: /כתיבה ב/ });
  await box.fill('');
  await box.pressSequentially('מי בא הערב @א');
  await p.locator('.mention-suggestions').waitFor();
  await shot(p, '370-mention-suggestions-he');
  await box.press('Enter');
  await box.pressSequentially('?');
  await box.press('Enter');
  await box.fill(`תודה ל-<@${'b'.repeat(64)}> על המפה`);
  await box.press('Enter');
  await p.locator('.mention-chip.former').waitFor();
  await p.waitForTimeout(400);
  await shot(p, '371-mention-chips-he');
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
  if (placeholder !== 'כתיבה ב-\u2068#clips\u2069') problems.push(`72: composer placeholder is "${placeholder}"`);
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

// ---- B11 server settings and B25 context menus ---------------------------------
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const channels = p.getByRole('navigation', { name: 'ערוצים' });
  await shot(p, '180-sidebar-settings-button-he');
  await channels.getByRole('button', { name: /^clips/ }).click({ button: 'right' });
  await p.getByRole('menu').waitFor();
  await shot(p, '172-menu-channel-owner-he');
  await p.keyboard.press('Escape');
  await channels.getByRole('button', { name: /^כללי/ }).click({ button: 'right' });
  await shot(p, '173-menu-general-owner-he');
  await p.keyboard.press('Escape');
  const members = p.getByRole('complementary', { name: 'חברי השרת' });
  const other = members.getByRole('button').filter({ hasNotText: 'בעלים' }).filter({ hasNotText: '(אני)' }).first();
  await other.click({ button: 'right' });
  await shot(p, '174-menu-member-owner-he');
  await p.keyboard.press('Escape');
  await p.getByRole('button', { name: 'הגדרות השרת' }).click();
  await p.getByRole('tab', { name: 'כללי' }).waitFor();
  await shot(p, '181-server-settings-general-he');
  await p.getByRole('tab', { name: 'ערוצים' }).click();
  await shot(p, '182-server-settings-channels-he');
  await p.getByRole('tab', { name: 'חברים' }).click();
  await shot(p, '183b-server-settings-members-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=en&theme=light&update=999999', 'x');
  await p.getByRole('button', { name: 'Server settings' }).click();
  await p.getByRole('tab', { name: 'General' }).waitFor();
  await shot(p, '186-server-settings-en-light');
  await p.close();
}
// Invites (#214). The B11 spec's 183–185 (removed accounts, unsaved changes,
// phone list) wait for core support, so the invite page takes those numbers.
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.getByRole('button', { name: 'הגדרות השרת' }).click();
  await p.getByRole('tab', { name: 'לינק הזמנה' }).click();
  const panel = p.getByRole('tabpanel');
  await panel.getByText(/^3 שימושים/).waitFor();
  await shot(p, '183-server-settings-invites-he');
  await panel.getByLabel('תוקף').selectOption('day');
  await panel.getByLabel('מספר שימושים').selectOption('5');
  await panel.getByRole('button', { name: 'יצירת לינק' }).click();
  await panel.getByText(/0 מתוך 5 שימושים/).waitFor();
  await shot(p, '184-server-settings-invite-created-he');
  await panel.getByRole('button', { name: 'ביטול הלינק' }).first().click();
  await panel.getByRole('group').waitFor();
  await shot(p, '185-server-settings-invite-revoke-he');
  await p.close();
}
// B25 §5: on a phone the menu is a bottom sheet.
{
  const p = await page('?demo=1&still=1&lang=he&theme=light&update=999999', PHONE);
  await p.getByRole('button', { name: /^clips/ }).click({ button: 'right' });
  await p.getByRole('menu').waitFor();
  await shot(p, '176-menu-sheet-phone-he');
  await p.close();
}

// Server lifecycle (#261): 250–256.
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.getByRole('button', { name: 'הגדרות השרת' }).click();
  await p.getByRole('tab', { name: 'כללי' }).waitFor();
  await p.getByLabel('שם השרת').fill('גיימרים ישראל HQ');
  await shot(p, '250-server-rename-he');
  await p.getByRole('tab', { name: 'אזור מסוכן' }).click();
  await shot(p, '251-server-danger-zone-he');
  await p.getByRole('button', { name: 'סגירת השרת…' }).click();
  await p.getByRole('dialog', { name: /^לסגור את/ }).getByRole('textbox').fill('גיימרים ישראל');
  await shot(p, '252-server-close-confirm-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  await p.getByRole('button', { name: /^Minecraft IL/ }).click({ button: 'right' });
  await p.getByRole('menu').waitFor();
  await shot(p, '253-server-menu-leave-he');
  await p.getByRole('menuitem', { name: 'עזיבת השרת' }).click();
  await p.getByRole('dialog', { name: /^לעזוב את/ }).waitFor();
  await shot(p, '254-server-leave-confirm-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&outdated=1', 'x');
  await p.getByRole('button', { name: 'הגדרות השרת' }).click();
  const panel = p.getByRole('tabpanel');
  await panel.getByLabel('שם השרת').fill('גיימרים ישראל HQ');
  await panel.getByRole('button', { name: 'שמירת השם' }).click();
  await panel.getByRole('alert').getByRole('listitem').first().waitFor();
  await shot(p, '255-server-update-needed-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&closed=1', 'x');
  await p.getByRole('button', { name: /^Minecraft IL/ }).click();
  await p.locator('.closed-banner').waitFor();
  await p.waitForTimeout(600);
  await shot(p, '256-server-closed-he');
  await p.close();
}

// Relay-only (#272): 270–271.
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&relayOnly=1&settings=privacy', 'x');
  await p.getByRole('button', { name: 'הגדרות' }).first().click();
  await p.getByRole('switch', { name: 'ממסר בלבד' }).waitFor();
  await shot(p, '270-settings-privacy-relay-only-he');
  await p.close();
}
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999&relayOnly=1', 'x');
  await p.locator('.voice-room-reason').waitFor();
  await shot(p, '271-voice-room-relay-only-he');
  await p.close();
}

// Restore (#290): 290–291. The restore asks the agreement, country and birth date
// first, then the words, the name, and ends on the servers notice.
{
  const p = await page('?fresh=1&lang=he&theme=light');
  await p.getByRole('button', { name: 'כבר יש לי חשבון' }).click();
  await p.getByRole('checkbox').check();
  await shot(p, '290-restore-agreement-he');
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByRole('button', { name: 'המשך' }).click();
  await p.getByLabel('יום').fill('12');
  await p.getByLabel('חודש').fill('4');
  await p.getByLabel('שנה').fill('2001');
  await p.getByRole('button', { name: 'המשך' }).click();
  const words = 'apple river stone cloud tiger piano lemon rocket forest candle silver garden';
  await p.getByLabel('מילה 1', { exact: true }).focus();
  await p.evaluate((text) => {
    const dt = new DataTransfer();
    dt.setData('text', text);
    document.activeElement.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true }));
  }, words);
  await p.getByRole('button', { name: 'שחזור' }).click();
  await p.getByLabel('שם תצוגה').fill('מורן');
  await p.getByRole('button', { name: 'סיום' }).click();
  await p.getByRole('heading', { name: 'החשבון שוחזר' }).waitFor();
  await shot(p, '291-restore-servers-notice-he');
  await p.close();
}

// Notifications and mute (G-61, G-03, G-17) and mark all as read (G-04): 360, 351.
{
  const p = await chat('?demo=1&still=1&lang=he&theme=light&update=999999', 'x');
  const channels = p.getByRole('navigation', { name: 'ערוצים' });
  await channels.getByRole('button', { name: /^clips/ }).click({ button: 'right' });
  await p.getByRole('menuitem', { name: 'התראות' }).click();
  await p.getByRole('dialog', { name: /^התראות של/ }).waitFor();
  await shot(p, '360-notify-dialog-channel-he');
  await p.getByRole('button', { name: 'עד שאבטל' }).click();
  await p.keyboard.press('Escape');
  await p.getByRole('button', { name: /^Minecraft IL/ }).click({ button: 'right' });
  await p.getByRole('menuitem', { name: 'התראות' }).click();
  await p.getByRole('button', { name: 'ל-8 שעות' }).click();
  await p.keyboard.press('Escape');
  await p.mouse.move(1, 1);
  await shot(p, '351-muted-server-and-channel-he');
  await p.evaluate(() => localStorage.removeItem('mvs.mute.v1'));
  await p.close();
}

// Our own title bar (frameless desktop window): 370 (he, dark), 371 (en, light).
// A stand-in for the shell's window.mvs.windowControl; the real one is in desktop/.
for (const [lang, theme, name] of [
  ['he', 'dark', '370-titlebar-he-dark'],
  ['en', 'light', '371-titlebar-en-light'],
]) {
  const ctx = await browser.newContext({ viewport: { width: 1366, height: 800 }, colorScheme: theme, locale: lang === 'he' ? 'he-IL' : 'en-US' });
  await ctx.addInitScript(() => {
    window.mvs = { windowControl: async () => ({ maximized: false }) };
  });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  await p.goto(`${BASE}?demo=1&still=1&lang=${lang}&theme=${theme}&update=999999`);
  await p.locator('.titlebar').waitFor();
  await p.getByRole('article').first().waitFor();
  await p.waitForTimeout(500);
  await shot(p, name);
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
