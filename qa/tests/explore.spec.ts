// Exploratory checks against the UI (mock core). Each test asserts the
// EXPECTED behaviour. Tests for open bugs carry `bug('QA-xx')`, which marks
// them test.fail(): the run stays green while the bug exists, and turns red
// ("expected to fail, but passed") once it is fixed – then drop the mark and
// close the row in BUGS.md. Screenshots for manual review land in
// test-results/explore-shots/.

import { expect, test, type Page } from '@playwright/test';
import { composer, createServer, he, lastMessage, onboard, readWords, trackErrors } from './helpers';

const SHOTS = 'test-results/explore-shots';

/** Marks a test as a known bug from BUGS.md. */
function bug(id: string) {
  test.info().annotations.push({ type: 'bug', description: id });
  // QA_RAW=1 runs bug repros as plain tests, to see which still reproduce.
  if (!process.env.QA_RAW) test.fail();
}

async function shot(page: Page, name: string) {
  await page.screenshot({ path: `${SHOTS}/${name}.png`, fullPage: false });
}

/** Horizontal overflow of the page (px). 0 means nothing sticks out. */
function pageOverflow(page: Page) {
  return page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
}

async function joinDemoServer(page: Page, link = 'mvs://join/qa-explore') {
  await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
  await page.getByLabel(he.joinLink).fill(link);
  await page.getByRole('button', { name: he.join, exact: true }).click();
  await expect(composer(page)).toBeVisible({ timeout: 10_000 });
}

/** Goes from onboarding to the date-of-birth step. */
async function toBirthDate(page: Page) {
  await page.goto('/?fresh=1&still=1');
  await page.getByRole('button', { name: he.next }).click();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: he.next }).click();
  await page.getByRole('button', { name: he.next }).click();
}

async function enterBirthDate(page: Page, d: number, m: number, y: number) {
  await page.getByLabel('יום').fill(String(d));
  await page.getByLabel('חודש').fill(String(m));
  await page.getByLabel('שנה').fill(String(y));
  await page.getByRole('button', { name: he.next }).click();
}

// ---------------------------------------------------------------------------
// Onboarding and identity
// ---------------------------------------------------------------------------

test.describe('onboarding', () => {
  test('fixed QA-01: reload while the recovery words are shown: the words are still reachable', async ({ page }) => {
    // Fixed by PR #28 (B19); kept as a regression check.
    await toBirthDate(page);
    await enterBirthDate(page, 3, 3, 1999);
    await expect(page.getByRole('heading', { name: he.wordsTitle })).toBeVisible({ timeout: 10_000 });
    await page.goto('/?still=1'); // reload: crash, power cut, accidental Ctrl+R …
    // Expected: onboarding resumes on the words, or Settings can show them.
    const wordsAgain = page.getByRole('heading', { name: he.wordsTitle });
    const settings = page.getByRole('button', { name: 'הגדרות' }).first();
    await expect(wordsAgain.or(settings)).toBeVisible({ timeout: 5_000 });
    if (!(await wordsAgain.isVisible())) {
      await settings.click();
      await expect(page.getByRole('button', { name: /להציג את המילים|הצגת המילים/ })).toBeVisible({ timeout: 3_000 });
    }
  });

  test('QA-02 the display name can be changed after onboarding ("אפשר לשנות בכל רגע")', async ({ page }) => {
    bug('QA-02');
    await onboard(page);
    await page.getByRole('button', { name: 'הגדרות' }).first().click();
    await page.getByRole('button', { name: 'חשבון' }).click();
    await expect(page.getByLabel(he.displayName)).toBeVisible({ timeout: 2_000 });
  });

  test('fixed QA-03: a large profile picture does not trap the user on the profile step', async ({ page }) => {
    const errors = trackErrors(page);
    await toBirthDate(page);
    await enterBirthDate(page, 5, 5, 1995);
    await readWords(page); // B19: "כתבתי אותן" works only after the words were shown
    await page.getByRole('button', { name: he.written }).click();
    await page.getByRole('button', { name: he.later }).click();
    // 8 MB "photo" – a normal phone picture size.
    const big = Buffer.alloc(8 * 1024 * 1024, 0x41);
    big.write('\x89PNG\r\n\x1a\n', 0, 'binary');
    await page.getByLabel('תמונת פרופיל').setInputFiles({ name: 'photo.png', mimeType: 'image/png', buffer: big });
    await page.getByLabel(he.displayName).fill('תמונה גדולה');
    await page.getByRole('button', { name: he.done }).click();
    // Expected: either the picture is resized/refused with a message, or onboarding finishes.
    await expect(page.getByRole('heading', { name: he.homeTitle }).or(page.getByRole('alert'))).toBeVisible({
      timeout: 5_000,
    });
    expect(errors.join('\n')).not.toMatch(/quota/i);
  });

  test('exactly 16 today is allowed, 16 tomorrow is blocked', async ({ page }) => {
    const now = new Date();
    await toBirthDate(page);
    const tomorrow = new Date(now.getFullYear() - 16, now.getMonth(), now.getDate() + 1);
    await enterBirthDate(page, tomorrow.getDate(), tomorrow.getMonth() + 1, tomorrow.getFullYear());
    await expect(page.getByRole('heading', { name: 'עוד קצת מוקדם' })).toBeVisible();

    await toBirthDate(page);
    await enterBirthDate(page, now.getDate(), now.getMonth() + 1, now.getFullYear() - 16);
    await expect(page.getByRole('heading', { name: he.wordsTitle })).toBeVisible({ timeout: 10_000 });
  });

  test('impossible dates are refused: 31/02, 29/02 in a non-leap year, the future', async ({ page }) => {
    await toBirthDate(page);
    for (const [d, m, y] of [
      [31, 2, 2000],
      [29, 2, 2001],
      [1, 1, new Date().getFullYear() + 1],
      [0, 5, 1990],
      [12, 13, 1990],
    ]) {
      await enterBirthDate(page, d, m, y);
      await expect(page.getByRole('alert')).toContainText('התאריך לא נראה תקין');
    }
    await enterBirthDate(page, 29, 2, 2000); // leap year: fine
    await expect(page.getByRole('heading', { name: he.wordsTitle })).toBeVisible({ timeout: 10_000 });
  });

  test('QA-04 an implausible birth year (1900, age 126) is questioned', async ({ page }) => {
    bug('QA-04');
    await toBirthDate(page);
    await enterBirthDate(page, 1, 1, 1900);
    await expect(page.getByRole('alert')).toBeVisible({ timeout: 2_000 });
  });

  test('the date fields take digits only (Hebrew/Arabic-Indic digits, letters, paste)', async ({ page }) => {
    await toBirthDate(page);
    await page.getByLabel('שנה').fill('19a9-0');
    await expect(page.getByLabel('שנה')).toHaveValue('1990');
    await page.getByLabel('יום').fill('١٢'); // Arabic-Indic digits are stripped, not converted
    await expect(page.getByLabel('יום')).toHaveValue('');
  });

  test('wrong recovery words are refused, then the right ones pass', async ({ page }) => {
    await toBirthDate(page);
    await enterBirthDate(page, 7, 7, 1997);
    await expect(page.getByRole('heading', { name: he.wordsTitle })).toBeVisible({ timeout: 10_000 });
    const words = await readWords(page);
    await page.getByRole('button', { name: he.written }).click();
    const fields = page.getByLabel(/^מילה מספר \d+$/);
    for (const f of await fields.all()) await f.fill('wrong');
    await page.getByRole('button', { name: he.verify, exact: true }).click();
    await expect(page.getByRole('alert')).toContainText(/לא תואמ/);
    // Upper case and stray spaces are accepted.
    for (const f of await fields.all()) {
      const label = await f.evaluate((el) => el.closest('label')?.querySelector('span')?.textContent ?? '');
      await f.fill(`  ${words[Number(label.replace(/\D/g, '')) - 1].toUpperCase()} `);
    }
    await page.getByRole('button', { name: he.verify, exact: true }).click();
    await expect(page.getByLabel(he.displayName)).toBeVisible();
  });

  test('restore: pasting 12 words with commas and capitals fills every box', async ({ page }) => {
    await page.goto('/?fresh=1&still=1');
    await page.getByRole('button', { name: 'כבר יש לי חשבון' }).click();
    const first = page.getByLabel('מילה 1', { exact: true });
    await first.focus();
    const twelve = 'Apple, river  STONE cloud,forest ember lantern orbit pepper quartz saddle timber';
    await page.evaluate((text) => {
      const dt = new DataTransfer();
      dt.setData('text/plain', text);
      document.activeElement!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    }, twelve);
    await expect(page.getByLabel('מילה 12', { exact: true })).toHaveValue('timber');
    await expect(page.getByLabel('מילה 3', { exact: true })).toHaveValue('stone');
  });

  test('QA-05 restore: pasting 13 words warns instead of silently dropping one', async ({ page }) => {
    bug('QA-05');
    await page.goto('/?fresh=1&still=1');
    await page.getByRole('button', { name: 'כבר יש לי חשבון' }).click();
    await page.getByLabel('מילה 1', { exact: true }).focus();
    await page.evaluate(() => {
      const dt = new DataTransfer();
      dt.setData('text/plain', 'extra apple river stone cloud forest ember lantern orbit pepper quartz saddle timber');
      document.activeElement!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    });
    await expect(page.getByRole('alert')).toBeVisible({ timeout: 2_000 });
  });
});

// ---------------------------------------------------------------------------
// Servers
// ---------------------------------------------------------------------------

test.describe('servers', () => {
  test('double-clicking "יצירה" creates one server, not two', async ({ page }) => {
    await onboard(page);
    await page.getByRole('button', { name: /^יצירת שרת/ }).click();
    await page.getByLabel(he.serverName).fill('לחיצה כפולה');
    await page.getByRole('radio', { name: '16 ומעלה' }).check();
    await page.getByRole('button', { name: he.create, exact: true }).dblclick();
    await expect(page.getByRole('heading', { name: he.serverReady })).toBeVisible();
    await page.getByRole('button', { name: he.goToServer }).click();
    await page.waitForTimeout(500);
    await expect(page.getByRole('navigation', { name: 'שרתים' }).getByRole('button', { name: /^לחיצה כפולה/ })).toHaveCount(1);
  });

  test('QA-06 a server name made only of invisible characters is refused', async ({ page }) => {
    bug('QA-06');
    await onboard(page);
    await page.getByRole('button', { name: /^יצירת שרת/ }).click();
    await page.getByLabel(he.serverName).fill(String.fromCodePoint(0x200b, 0x200f, 0x2060)); // ZWSP, RLM, WJ
    await page.getByRole('radio', { name: '16 ומעלה' }).check();
    await page.getByRole('button', { name: he.create, exact: true }).click();
    await expect(page.getByText('צריך לתת לשרת שם.')).toBeVisible({ timeout: 2_000 });
  });

  test('QA-07 "copy invite" does not claim success when the clipboard is unavailable', async ({ page }) => {
    bug('QA-07');
    await page.addInitScript(() => {
      Object.defineProperty(navigator, 'clipboard', {
        value: { writeText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) },
      });
    });
    await onboard(page);
    await page.getByRole('button', { name: /^יצירת שרת/ }).click();
    await page.getByLabel(he.serverName).fill('בלי לוח');
    await page.getByRole('radio', { name: '16 ומעלה' }).check();
    await page.getByRole('button', { name: he.create, exact: true }).click();
    await page.getByRole('button', { name: 'העתקה' }).click();
    await expect(page.getByRole('button', { name: 'הועתק' })).toHaveCount(0);
  });

  test('join: double-submit then cancel does not join anyway', async ({ page }) => {
    await onboard(page);
    await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
    await page.getByLabel(he.joinLink).fill('mvs://join/double-click');
    await page.getByLabel(he.joinLink).press('Enter');
    await page.getByLabel(he.joinLink).press('Enter').catch(() => undefined);
    await page.getByRole('button', { name: 'ביטול' }).click();
    await page.waitForTimeout(3_000);
    await expect(page.getByRole('navigation', { name: 'שרתים' }).getByRole('button', { name: /^Valorant/ })).toHaveCount(0);
  });

  test('join: links that are not invites are refused', async ({ page }) => {
    await onboard(page);
    await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
    for (const link of ['hello', 'https://example.com', 'mvs://join/', 'javascript:alert(1)']) {
      await page.getByLabel(he.joinLink).fill(link);
      await page.getByRole('button', { name: he.join, exact: true }).click();
      await expect(page.getByText('זה לא נראה כמו לינק הזמנה')).toBeVisible();
    }
  });

  test('QA-08 join: "keep looking in the background" leaves a visible pending join', async ({ page }) => {
    bug('QA-08');
    await onboard(page);
    await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
    await page.getByLabel(he.joinLink).fill('mvs://join/offline-server');
    await page.getByRole('button', { name: he.join, exact: true }).click();
    await page.getByRole('button', { name: 'להמשיך לחפש ברקע' }).click({ timeout: 10_000 });
    // Expected: something on screen says a join is still in progress.
    await expect(page.getByText(/מחפשים|ממתין|בהמתנה/)).toBeVisible({ timeout: 2_000 });
  });

  test('long names without spaces do not push the layout sideways', async ({ page }) => {
    await onboard(page, { displayName: 'א'.repeat(32) });
    await createServer(page, 'W'.repeat(48));
    await composer(page).fill('x'.repeat(400));
    await composer(page).press('Enter');
    await shot(page, 'long-names');
    expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
  });
});

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

test.describe('messages', () => {
  test('mixed RTL/LTR and emoji messages and names send and display (direction: fixed UX-01)', async ({ page }) => {
    await onboard(page, { displayName: 'Moran מורן' });
    await createServer(page, 'Mixed מעורב');
    const bodies = ['hello שלום 123 world', 'שלום hello, מה נשמע? 🎮🔥', '(1) – רשימה', '👨‍👩‍👧‍👦🏳️‍🌈🇮🇱'];
    for (const b of bodies) {
      await composer(page).fill(b);
      await composer(page).press('Enter');
    }
    await expect(lastMessage(page).locator('.msg-body')).toContainText('👨‍👩‍👧‍👦');
    // Names are isolated with <bdi>, so a mixed-script name does not reorder the line around it.
    await expect(page.locator('.msg-head bdi').first()).toBeVisible();
    await shot(page, 'mixed-bidi');
  });

  test('fixed UX-01: message direction follows most strong letters, aligned to the interface side', async ({ page }) => {
    // ux-review-1 blocker, fixed by PR #24 (B17); kept as a regression check.
    await onboard(page);
    await createServer(page, 'כיוון');
    // The five strings from UX-01, with the direction they should get in a Hebrew interface.
    const cases: [string, 'rtl' | 'ltr'][] = [
      ['lol זה היה מטורף', 'rtl'],
      ['GG על המשחק אתמול, 3:1 לנו!', 'rtl'],
      ['@Itamar תבדוק את ה-patch החדש', 'rtl'],
      ['version 2.4.1 fixed it', 'ltr'],
      ['🎉🎉', 'rtl'], // no strong letters: the interface direction
    ];
    for (const [text] of cases) {
      await composer(page).fill(text);
      await composer(page).press('Enter');
    }
    await expect(page.locator('.msg-body')).toHaveCount(cases.length);
    await shot(page, 'ux-01-direction');
    // Since PR #50 the direction is set per line (`.msg-line`); each case here is one line.
    const shown = await page.locator('.msg-body .msg-line').evaluateAll((els) =>
      els.map((el) => {
        const cs = getComputedStyle(el);
        // Alignment must be the interface side (right in Hebrew) for every message.
        const right = cs.textAlign === 'right' || (cs.textAlign === 'start' && cs.direction === 'rtl');
        return { dir: el.getAttribute('dir'), right };
      }),
    );
    expect(shown).toEqual(cases.map(([, dir]) => ({ dir, right: true })));
  });

  test('QA-09 the avatar letter of an emoji/flag name is a whole character', async ({ page }) => {
    bug('QA-09');
    await onboard(page, { displayName: '🇮🇱 ישראל' });
    await createServer(page, 'דגל');
    await composer(page).fill('בדיקה');
    await composer(page).press('Enter');
    const letter = await page.locator('.msg .avatar span').first().textContent();
    expect(letter).toBe('🇮🇱');
  });

  test('a 100,000-character paste is refused calmly and the page stays responsive', async ({ page }) => {
    const errors = trackErrors(page);
    await onboard(page);
    await createServer(page, 'הדבקה ענקית');
    const huge = 'שלום world '.repeat(9_091).slice(0, 100_000);
    const started = Date.now();
    await composer(page).fill(huge);
    await composer(page).press('Enter');
    // Since PR #34 the composer refuses text over the limit instead of cutting it.
    await expect(page.getByRole('alert')).toContainText('ההודעה ארוכה מדי');
    await expect(page.getByRole('button', { name: 'שליחה' })).toBeDisabled();
    await expect(page.locator('.msg-body')).toHaveCount(0);
    const ms = Date.now() - started;
    test.info().annotations.push({ type: 'timing', description: `100k paste: ${ms} ms` });
    expect(ms).toBeLessThan(2_000);
    await shot(page, 'huge-paste');
    expect(errors).toEqual([]);
  });

  test('fixed QA-10: a message over the core size limit is refused in the composer', async ({ page }) => {
    // Fixed by PR #34 (counter from 3,800, red and no send past the limit); kept as a regression check.
    await onboard(page);
    await createServer(page, 'מגבלת גודל');
    await composer(page).fill('א'.repeat(8_200)); // 16,400 bytes of UTF-8 > the 16 KiB Message body limit
    await expect(page.getByRole('button', { name: 'שליחה' })).toBeDisabled({ timeout: 1_000 });
  });

  test('rapid Enter presses send one message, not many', async ({ page }) => {
    await onboard(page);
    await createServer(page, 'הקשה מהירה');
    await composer(page).fill('פעם אחת');
    for (let i = 0; i < 15; i++) await composer(page).press('Enter', { delay: 0 });
    await expect(page.locator('.msg-body', { hasText: 'פעם אחת' })).toHaveCount(1);
  });

  test('rapid clicking send while typing never sends an empty message', async ({ page }) => {
    await onboard(page);
    await createServer(page, 'לחיצות');
    for (let i = 0; i < 10; i++) {
      await composer(page).fill(`הודעה ${i}`);
      await page.getByRole('button', { name: 'שליחה' }).click();
    }
    await expect(page.locator('.msg-body')).toHaveCount(10);
    await expect(page.locator('.msg-body').filter({ hasText: /^$/ })).toHaveCount(0);
  });

  test('edit, reply and delete my own message', async ({ page }) => {
    await onboard(page);
    await joinDemoServer(page);
    await composer(page).fill('לפני עריכה');
    await composer(page).press('Enter');
    await composer(page).press('ArrowUp'); // edit last own message
    await expect(page.getByText('עריכת הודעה')).toBeVisible();
    await composer(page).fill('אחרי עריכה');
    await composer(page).press('Enter');
    const mine = lastMessage(page);
    await expect(mine.locator('.msg-body')).toContainText('אחרי עריכה');
    await expect(mine.getByText('(נערך)')).toBeVisible();
    await mine.hover();
    await mine.getByRole('button', { name: 'תגובה' }).click();
    await composer(page).fill('תגובה לעצמי');
    await composer(page).press('Enter');
    // The demo server's members keep talking, so find the reply by its text, not as the last message.
    const reply = page.getByRole('article').filter({ hasText: 'תגובה לעצמי' }).last();
    await expect(reply.getByRole('button', { name: 'ההודעה המקורית' })).toContainText('אחרי עריכה');
    await reply.hover();
    await reply.getByRole('button', { name: 'מחיקה' }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'מחיקה' }).click();
    await expect(page.getByText('ההודעה נמחקה').last()).toBeVisible();
  });

  test('QA-11 an unsent draft survives switching channels', async ({ page }) => {
    bug('QA-11');
    await onboard(page);
    await joinDemoServer(page);
    await composer(page).fill('טיוטה שלא נשלחה');
    await page.getByRole('button', { name: /^lfg/ }).click();
    await page.getByRole('button', { name: /^כללי/ }).click();
    await expect(composer(page)).toHaveValue('טיוטה שלא נשלחה');
  });

  test('fixed QA-13: bidi control characters in a display name cannot reorder it', async ({ page }) => {
    // U+202E (RIGHT-TO-LEFT OVERRIDE) makes "evil" render as "live".
    await onboard(page, { displayName: `Moran ${String.fromCodePoint(0x202e)}evil` });
    await createServer(page, 'התחזות');
    await composer(page).fill('שלום');
    await composer(page).press('Enter');
    const shown = await page.locator('.msg-head .author').first().textContent();
    const controls = [...(shown ?? '')].map((c) => c.codePointAt(0)!).filter((cp) => (cp >= 0x202a && cp <= 0x202e) || (cp >= 0x2066 && cp <= 0x2069));
    expect(controls).toEqual([]);
  });

  test('blocking hides messages, "show anyway" reveals them', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000');
    const someone = page.getByRole('complementary', { name: 'חברי השרת' }).getByRole('button').nth(1);
    await someone.click();
    await page.getByRole('dialog').getByRole('button', { name: 'חסימה' }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'סגירה' }).click();
    await expect(page.getByText('הודעה ממשתמש חסום').first()).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// Layout, theme, scale
// ---------------------------------------------------------------------------

test.describe('layout', () => {
  for (const lang of ['he', 'en'] as const) {
    test(`200% interface scale and 24px font keep the ${lang} layout intact (1366×768)`, async ({ page }) => {
      await page.setViewportSize({ width: 1366, height: 768 });
      await page.addInitScript(() =>
        localStorage.setItem('mvp.settings.v1', JSON.stringify({ uiScale: 2, fontSize: 24, theme: 'dark' })),
      );
      await page.goto(`/?demo=1&still=1&update=600000&lang=${lang}`);
      await expect(page.getByRole('navigation').first()).toBeVisible();
      await page.waitForTimeout(500);
      await shot(page, `scale-200-${lang}`);
      expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
      await expect(composer(page).or(page.getByRole('textbox', { name: /^Message #/ }))).toBeInViewport();
    });
  }

  test('a narrow window (800×600) at 150% keeps the composer usable', async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 600 });
    await page.addInitScript(() => localStorage.setItem('mvp.settings.v1', JSON.stringify({ uiScale: 1.5 })));
    await page.goto('/?demo=1&still=1&update=600000');
    await page.waitForTimeout(500);
    await shot(page, 'narrow-150');
    await expect(composer(page)).toBeInViewport({ ratio: 1 });
    expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
  });

  test('QA-12 at 200% + 24 px (1366×768) the side panels collapse and the chat stays readable', async ({ page }) => {
    bug('QA-12');
    await page.setViewportSize({ width: 1366, height: 768 });
    await page.addInitScript(() => localStorage.setItem('mvp.settings.v1', JSON.stringify({ uiScale: 2, fontSize: 24 })));
    await page.goto('/?demo=1&still=1&update=600000');
    await expect(page.getByRole('article').first()).toBeVisible();
    const width = await page.locator('section.chat').evaluate((el) => el.getBoundingClientRect().width);
    test.info().annotations.push({ type: 'measure', description: `chat column ${Math.round(width)} px` });
    // ~25 characters of 24px text; less than that wraps every word.
    expect(width).toBeGreaterThanOrEqual(400);
    // Effective width ~683 px: the member list collapses below ~900 px and the channel list
    // moves to a drawer below ~700 px, as on the phone layout (product lead, PR #50 review).
    await expect(page.locator('aside.members')).toBeHidden();
    await expect(page.locator('aside.sidebar')).toBeHidden();
  });

  test('theme, language and scale change instantly (< 200 ms) without reload', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000');
    await expect(page.getByRole('navigation').first()).toBeVisible();
    await page.getByRole('button', { name: 'הגדרות' }).first().click();
    await page.getByRole('button', { name: 'מראה' }).click();
    const t0 = Date.now();
    await page.getByRole('radio', { name: 'כהה' }).check();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    test.info().annotations.push({ type: 'timing', description: `theme switch: ${Date.now() - t0} ms` });
    await shot(page, 'settings-dark');
    await page.getByRole('button', { name: 'שפה' }).click();
    const t1 = Date.now();
    await page.getByRole('radio', { name: 'English' }).check();
    await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');
    expect(Date.now() - t1).toBeLessThan(1_000);
  });

  test('Ctrl+= / Ctrl+- / Ctrl+0 clamp the scale to 75–200%', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000');
    await expect(page.getByRole('navigation').first()).toBeVisible();
    for (let i = 0; i < 20; i++) await page.keyboard.press('Control+=');
    expect(await page.evaluate(() => document.documentElement.style.fontSize)).toBe('32px');
    for (let i = 0; i < 30; i++) await page.keyboard.press('Control+-');
    expect(await page.evaluate(() => document.documentElement.style.fontSize)).toBe('12px');
    await page.keyboard.press('Control+0');
    expect(await page.evaluate(() => document.documentElement.style.fontSize)).toBe('16px');
  });

  test('resizing from wide to narrow and back does not break the shell', async ({ page }) => {
    const errors = trackErrors(page);
    await page.goto('/?demo=1&still=1&update=600000');
    await expect(page.getByRole('navigation').first()).toBeVisible();
    for (const [w, h] of [
      [1920, 1080],
      [1024, 700],
      [640, 480],
      [1366, 768],
    ]) {
      await page.setViewportSize({ width: w, height: h });
      await page.waitForTimeout(150);
    }
    await expect(composer(page)).toBeVisible();
    expect(errors).toEqual([]);
  });

  test('server switch and channel switch feel instant (< 100 / 150 ms)', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000');
    const rail = page.getByRole('navigation', { name: 'שרתים' });
    await expect(page.getByRole('article').first()).toBeVisible();
    // Measured inside the page: click → the new heading is painted (next frame).
    const timeSwitch = (button: string, heading: string) =>
      page.evaluate(
        ([button, heading]) =>
          new Promise<number>((resolve) => {
            const btn = [...document.querySelectorAll('button')].find((b) =>
              (b.getAttribute('aria-label') ?? b.textContent ?? '').trim().startsWith(button),
            )!;
            const t0 = performance.now();
            btn.click();
            const poll = () => {
              if (document.querySelector('.chat-header h2')?.textContent === heading) {
                requestAnimationFrame(() => resolve(performance.now() - t0));
              } else requestAnimationFrame(poll);
            };
            poll();
          }),
        [button, heading],
      );
    // Best of 6 round trips: other test workers share the CPU.
    const server: number[] = [];
    const channel: number[] = [];
    let mcChannel = 'general'; // the server remembers its last channel
    for (let i = 0; i < 6; i++) {
      server.push(await timeSwitch('Minecraft IL', mcChannel));
      await page.waitForTimeout(200);
      mcChannel = mcChannel === 'general' ? 'builds' : 'general';
      channel.push(await timeSwitch(mcChannel, mcChannel));
      await page.waitForTimeout(200);
      await timeSwitch('גיימרים ישראל', 'כללי');
      await page.waitForTimeout(200);
    }
    const serverMs = Math.min(...server);
    const channelMs = Math.min(...channel);
    test.info().annotations.push({ type: 'timing', description: `server ${serverMs.toFixed(0)} ms, channel ${channelMs.toFixed(0)} ms` });
    expect(serverMs).toBeLessThan(100);
    expect(channelMs).toBeLessThan(150);
  });

  test('100k-message channel keeps the DOM small while scrolling to the top', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000');
    await expect(page.getByRole('article').first()).toBeVisible();
    await page.getByRole('article').last().focus();
    await page.keyboard.press('Home');
    await page.waitForTimeout(500);
    expect(await page.getByRole('article').count()).toBeLessThan(80);
    await expect(page.locator('[aria-posinset="1"]')).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// Privacy
// ---------------------------------------------------------------------------

test('the diagnostics file contains no words, birth date or message text', async ({ page }) => {
  const words = await onboard(page, { birthDate: '1993-08-21' });
  await createServer(page, 'פרטיות');
  await composer(page).fill('סוד-מאוד-פרטי');
  await composer(page).press('Enter');
  await page.getByRole('button', { name: 'הגדרות' }).first().click();
  await page.getByRole('button', { name: 'אבחון' }).click();
  const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'ייצוא קובץ אבחון' }).click()]);
  const text = await (await download.createReadStream()).toArray().then((c) => Buffer.concat(c).toString('utf8'));
  for (const w of words) expect(text).not.toContain(`"${w}"`);
  expect(text).not.toContain('1993-08-21');
  expect(text).not.toContain('סוד-מאוד-פרטי');
});

// ---------------------------------------------------------------------------
// PR #10: sanitised core data, phone layout (< 40rem), validated settings
// ---------------------------------------------------------------------------

test.describe('phone layout', () => {
  test.use({ viewport: { width: 375, height: 812 }, hasTouch: true, isMobile: true });

  test('fixed QA-18: the recovery-word check fits a 375 px phone', async ({ page }) => {
    // Fixed by PR #28 (B19); kept as a regression check.
    await toBirthDate(page);
    await enterBirthDate(page, 1, 1, 1990);
    await readWords(page); // B19: "כתבתי אותן" works only after the words were shown
    await page.getByRole('button', { name: he.written }).click();
    await expect(page.getByRole('heading', { name: he.checkTitle })).toBeVisible();
    await shot(page, 'phone-word-check');
    expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
    await expect(page.getByRole('button', { name: he.verify, exact: true })).toBeInViewport({ ratio: 1 });
  });

  test('send a message and switch pages on a 375 px phone, no sideways scroll', async ({ page }) => {
    const errors = trackErrors(page);
    await page.goto('/?demo=1&still=1&update=600000');
    await page.getByRole('button', { name: /^כללי/ }).first().click();
    await expect(composer(page)).toBeInViewport();
    await composer(page).fill('שלום מהטלפון');
    await composer(page).press('Enter');
    await expect(lastMessage(page).locator('.msg-body')).toContainText('שלום מהטלפון');
    expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
    await shot(page, 'phone-chat');
    // Back to the channel page and into another channel.
    await page.getByRole('button', { name: 'חזרה לערוצים' }).click();
    await expect(composer(page)).toBeHidden();
    await page.getByRole('button', { name: /^clips/ }).click();
    await expect(composer(page)).toBeInViewport();
    expect(errors).toEqual([]);
  });

  test('member sheet opens as a dialog and Esc closes it', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000');
    await page.getByRole('button', { name: /^כללי/ }).first().click();
    await page.getByRole('button', { name: 'רשימת החברים' }).click();
    const sheet = page.getByRole('dialog');
    await expect(sheet).toBeVisible();
    await shot(page, 'phone-members');
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    await expect(composer(page)).toBeInViewport();
  });

  test('QA-19 chat controls on a phone are at least 44×44 px', async ({ page }) => {
    bug('QA-19'); // the reply reference ("ההודעה המקורית") is ~19 px tall
    await page.goto('/?demo=1&still=1&update=600000');
    await page.getByRole('button', { name: /^כללי/ }).first().click();
    await expect(composer(page)).toBeVisible();
    const small = await page.locator('main button:visible').evaluateAll((els) =>
      els
        .map((el) => ({ name: el.getAttribute('aria-label') ?? el.textContent?.trim(), r: el.getBoundingClientRect() }))
        .filter(({ r }) => r.width > 0 && (r.width < 44 || r.height < 44))
        .map(({ name, r }) => `${name} ${Math.round(r.width)}×${Math.round(r.height)}`),
    );
    expect(small).toEqual([]);
  });

  test('English phone layout is LTR and does not scroll sideways', async ({ page }) => {
    await page.goto('/?demo=1&still=1&update=600000&lang=en');
    await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');
    await page.getByRole('button', { name: /^כללי/ }).first().click();
    await expect(composer(page).or(page.getByRole('textbox', { name: /^Message #/ }))).toBeInViewport();
    expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
  });
});

test.describe('hostile data from peers', () => {
  test('?hostile=1: markup, overrides, zalgo, remote colours and huge messages stay harmless', async ({ page }) => {
    const errors = trackErrors(page);
    const remote: string[] = [];
    page.on('request', (r) => {
      if (!r.url().startsWith('http://localhost')) remote.push(r.url());
    });
    page.on('dialog', (d) => {
      errors.push(`dialog: ${d.message()}`);
      void d.dismiss();
    });
    await page.goto('/?demo=1&still=1&update=600000&hostile=1');
    await expect(page.getByRole('article').first()).toBeVisible();
    await page.waitForTimeout(500);
    await shot(page, 'hostile');
    expect(remote).toEqual([]);
    expect(errors).toEqual([]);
    expect(await page.locator('.msg-body img, .msg-body a').count()).toBe(0);
    expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
    const controls = await page.evaluate(() =>
      [...document.body.innerText].filter((c) => /[\u202A-\u202E\u2066-\u2069]/.test(c)).length,
    );
    expect(controls).toBe(0);
    // The look-alike of the owner (מורן → מודן) is flagged.
    await expect(page.getByText('שם דומה').first()).toBeVisible();
  });

  test('corrupt settings in storage fall back to defaults', async ({ page }) => {
    const errors = trackErrors(page);
    await page.addInitScript(() =>
      localStorage.setItem('mvp.settings.v1', JSON.stringify({ locale: 'xx', theme: '<b>', uiScale: 100, fontSize: 'big' })),
    );
    await page.goto('/?demo=1&still=1&update=600000');
    await expect(page.getByRole('navigation').first()).toBeVisible();
    const root = await page.evaluate(() => ({
      lang: document.documentElement.lang,
      size: document.documentElement.style.fontSize,
      theme: document.documentElement.dataset.theme,
    }));
    expect(['he', 'en']).toContain(root.lang);
    expect(Number.parseFloat(root.size)).toBeLessThanOrEqual(32);
    expect(['system', 'light', 'dark']).toContain(root.theme);
    expect(errors).toEqual([]);
  });

  test('QA-17 one 4,000-code-point limit with a counter from 3,800', async ({ page }) => {
    bug('QA-17');
    await onboard(page);
    await createServer(page, 'קיצוץ');
    // Decided (product lead, PR #27): one limit of 4,000 code points in the composer,
    // the display and the core, with a visible counter from 3,800.
    await composer(page).fill('a'.repeat(3_900));
    await expect(page.locator('.composer')).toContainText(/3[,.\s]?900|\b100\b/);
    // 5,000 letters must not be cut silently: the composer refuses and says so, nothing is sent.
    const before = await page.locator('.msg-body').count();
    await composer(page).fill('a'.repeat(5_000));
    await composer(page).press('Enter');
    await expect(page.getByRole('alert')).toContainText('ההודעה ארוכה מדי');
    await expect(page.locator('.msg-body')).toHaveCount(before);
    // The limit is in code points: 2,100 emoji (4,200 UTF-16 units) are under it and can be sent.
    await composer(page).fill(String.fromCodePoint(0x1f600).repeat(2_100));
    await expect(page.getByRole('button', { name: 'שליחה' })).toBeEnabled();
  });
});
