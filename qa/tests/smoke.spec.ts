// Smoke suite: the closed-alpha happy path against the UI with the mock core.
// Onboarding → recovery-word check → create a server → send a message →
// join a server by link → message delivered → reload keeps the account.

import { expect, test } from '@playwright/test';
import { composer, createServer, he, lastMessage, onboard, trackErrors } from './helpers';

test.describe('smoke', () => {
  test('onboarding in Hebrew is RTL and ends on the home screen', async ({ page }) => {
    const errors = trackErrors(page);
    await onboard(page);
    await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
    await expect(page.locator('html')).toHaveAttribute('lang', 'he');
    await expect(page.getByText(he.recoveryBanner)).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test('recovery words: 12 distinct-looking words, always LTR', async ({ page }) => {
    await page.goto('/?fresh=1&still=1');
    await page.getByRole('button', { name: he.next }).click();
    await page.getByRole('checkbox').check();
    await page.getByRole('button', { name: he.next }).click();
    await page.getByRole('button', { name: he.next }).click();
    await page.getByLabel('יום').fill('1');
    await page.getByLabel('חודש').fill('1');
    await page.getByLabel('שנה').fill('1990');
    await page.getByRole('button', { name: he.next }).click();
    const grid = page.getByRole('list', { name: he.wordsGrid });
    await expect(grid).toBeVisible({ timeout: 10_000 });
    await expect(grid).toHaveAttribute('dir', 'ltr');
    await expect(grid.locator('.word')).toHaveCount(12);
  });

  test('under 16 is blocked calmly and no account is created', async ({ page }) => {
    await page.goto('/?fresh=1&still=1');
    await page.getByRole('button', { name: he.next }).click();
    await page.getByRole('checkbox').check();
    await page.getByRole('button', { name: he.next }).click();
    await page.getByRole('button', { name: he.next }).click();
    await page.getByLabel('יום').fill('1');
    await page.getByLabel('חודש').fill('1');
    await page.getByLabel('שנה').fill(String(new Date().getFullYear() - 12));
    await page.getByRole('button', { name: he.next }).click();
    await expect(page.getByRole('heading', { name: 'עוד קצת מוקדם' })).toBeVisible();
    await page.reload();
    await expect(page.getByRole('heading', { name: he.languageTitle })).toBeVisible();
  });

  test('create a server, send a message, it waits while nobody is online', async ({ page }) => {
    const errors = trackErrors(page);
    await onboard(page);
    const link = await createServer(page, 'שרת בדיקה');
    expect(link).toMatch(/^mvp:\/\/join\//);

    await expect(page.getByRole('heading', { name: 'general' })).toBeVisible();
    await expect(page.getByText('אף אחד מהשרת לא מחובר כרגע').first()).toBeVisible();

    const box = composer(page);
    await box.fill('הודעה ראשונה 👋 hello');
    await box.press('Enter');
    await expect(box).toHaveValue('');
    const msg = lastMessage(page);
    await expect(msg.locator('.msg-body')).toContainText('הודעה ראשונה 👋 hello');
    await expect(msg.locator('.msg-body')).toHaveAttribute('dir', 'auto');
    await expect(msg.getByText(he.pending)).toBeVisible();
    expect(errors).toEqual([]);
  });

  test('join a server by link and get a message delivered', async ({ page }) => {
    const errors = trackErrors(page);
    await onboard(page);
    await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
    await page.getByLabel(he.joinLink).fill('mvp://join/qa-smoke-test');
    await page.getByRole('button', { name: he.join, exact: true }).click();
    await expect(page.getByText('מחפשים חברים מהשרת…')).toBeVisible();
    await expect(page.getByRole('navigation', { name: 'שרתים' }).getByRole('button', { name: /^Valorant ישראל/ })).toBeVisible({
      timeout: 10_000,
    });

    const box = composer(page);
    await expect(box).toBeVisible();
    await box.fill('מישהו פה? anyone here');
    await page.getByRole('button', { name: 'שליחה' }).click();
    const msg = lastMessage(page);
    await expect(msg.locator('.msg-body')).toContainText('מישהו פה? anyone here');
    await expect(msg.getByRole('img', { name: he.received })).toBeVisible({ timeout: 5_000 });
    expect(errors).toEqual([]);
  });

  test('a failed message can be retried', async ({ page }) => {
    await onboard(page);
    await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
    await page.getByLabel(he.joinLink).fill('mvp://join/qa-retry');
    await page.getByRole('button', { name: he.join, exact: true }).click();
    const box = composer(page);
    await expect(box).toBeVisible({ timeout: 10_000 });
    await box.fill('זה ייכשל /fail');
    await box.press('Enter');
    const msg = lastMessage(page);
    await expect(msg.getByText(he.failed)).toBeVisible();
    await msg.getByRole('button', { name: 'לנסות שוב' }).click();
    await expect(msg.getByRole('img', { name: he.received })).toBeVisible({ timeout: 5_000 });
  });

  test('the account and server survive a reload', async ({ page }) => {
    await onboard(page, { displayName: 'נשאר אחרי רענון' });
    await createServer(page, 'שרת קבוע');
    await page.goto('/?still=1&update=600000');
    await expect(page.getByRole('navigation', { name: 'שרתים' }).getByRole('button', { name: /^שרת קבוע/ })).toBeVisible();
    await expect(page.getByRole('heading', { name: he.languageTitle })).toHaveCount(0);
  });

  test('English switches the layout to LTR', async ({ page }) => {
    await page.goto('/?fresh=1&still=1');
    await page.getByRole('radio', { name: 'English' }).click();
    await expect(page.getByRole('heading', { name: 'Choose a language' })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');
  });
});
