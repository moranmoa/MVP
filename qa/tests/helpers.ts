import { expect, type Page } from '@playwright/test';

// Hebrew is the default interface language; selectors use the Hebrew copy
// from ui/src/i18n/he.json, the same way a tester reads the screen.
export const he = {
  next: 'המשך',
  languageTitle: 'בחירת שפה',
  wordsTitle: 'מילות השחזור שלך',
  wordsGrid: '12 מילות שחזור, לפי הסדר',
  written: 'כתבתי אותן',
  checkTitle: 'בדיקה קצרה',
  verify: 'בדיקה',
  later: 'אעשה את זה אחר כך',
  displayName: 'שם תצוגה',
  done: 'סיום',
  homeTitle: 'ברוכים הבאים',
  serverName: 'שם השרת',
  create: 'יצירה',
  serverReady: 'השרת מוכן',
  goToServer: 'מעבר לשרת',
  joinLink: 'לינק הזמנה',
  join: 'הצטרפות',
  pending: 'ממתינה – תישלח כשמישהו מהשרת יתחבר',
  received: 'התקבלה אצל חבר',
  failed: 'לא נשלחה',
  recoveryBanner: 'מילות השחזור עוד לא נבדקו',
};

export interface OnboardOptions {
  /** `YYYY-MM-DD`; defaults to an adult. */
  birthDate?: string;
  displayName?: string;
  /** Pass the 3-word check (true) or choose "later" (false). */
  checkWords?: boolean;
}

/** Fresh account through the whole onboarding. Returns the 12 words. */
export async function onboard(page: Page, opts: OnboardOptions = {}): Promise<string[]> {
  const { birthDate = '2001-04-12', displayName = 'בודקת QA', checkWords = true } = opts;
  await page.goto('/?fresh=1&still=1&update=600000');
  await expect(page.getByRole('heading', { name: he.languageTitle })).toBeVisible();
  await page.getByRole('button', { name: he.next }).click();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: he.next }).click();
  await page.getByRole('button', { name: he.next }).click(); // country is prefilled
  const [y, m, d] = birthDate.split('-');
  await page.getByLabel('יום').fill(String(Number(d)));
  await page.getByLabel('חודש').fill(String(Number(m)));
  await page.getByLabel('שנה').fill(y);
  await page.getByRole('button', { name: he.next }).click();

  await expect(page.getByRole('heading', { name: he.wordsTitle })).toBeVisible({ timeout: 10_000 });
  const words = await readWords(page);
  expect(words).toHaveLength(12);
  await page.getByRole('button', { name: he.written }).click();

  await expect(page.getByRole('heading', { name: he.checkTitle })).toBeVisible();
  if (checkWords) {
    await fillWordCheck(page, words);
    await page.getByRole('button', { name: he.verify, exact: true }).click();
  } else {
    await page.getByRole('button', { name: he.later }).click();
  }

  await page.getByLabel(he.displayName).fill(displayName);
  await page.getByRole('button', { name: he.done }).click();
  await expect(page.getByRole('heading', { name: he.homeTitle })).toBeVisible();
  return words;
}

export async function readWords(page: Page): Promise<string[]> {
  return page.getByRole('list', { name: he.wordsGrid }).locator('.word').allTextContents();
}

/** Fills whichever 3 positions the check asks for ("מילה מספר N"). */
export async function fillWordCheck(page: Page, words: string[]): Promise<void> {
  const fields = page.getByLabel(/^מילה מספר \d+$/);
  await expect(fields).toHaveCount(3);
  for (const field of await fields.all()) {
    const label = await field.evaluate((el) => el.closest('label')?.querySelector('span')?.textContent ?? '');
    const n = Number(label.replace(/\D/g, ''));
    await field.fill(words[n - 1]);
  }
}

export async function createServer(page: Page, name: string, age: '16 ומעלה' | '18 ומעלה' | RegExp = '16 ומעלה') {
  await page.getByRole('button', { name: /^יצירת שרת/ }).click();
  await page.getByLabel(he.serverName).fill(name);
  await page.getByRole('radio', { name: age }).check();
  await page.getByRole('button', { name: he.create, exact: true }).click();
  await expect(page.getByRole('heading', { name: he.serverReady })).toBeVisible();
  const link = (await page.locator('.invite-box .code').textContent()) ?? '';
  await page.getByRole('button', { name: he.goToServer }).click();
  return link;
}

export function composer(page: Page) {
  return page.getByRole('textbox', { name: /^כתיבה ב-#/ });
}

/** The last message row in the virtualised list. */
export function lastMessage(page: Page) {
  return page.getByRole('article').filter({ has: page.locator('.msg-body') }).last();
}

/** Collects uncaught page errors so a test can assert there were none. */
export function trackErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  return errors;
}
