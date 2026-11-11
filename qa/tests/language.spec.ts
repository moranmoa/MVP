// The interface language default (0.4.1): the system language when we have it,
// else English; the picker lists every language in its own name.

import { expect, test } from '@playwright/test';

test.describe('language default: he-IL system', () => {
  test.use({ locale: 'he-IL' });
  test('opens in Hebrew, RTL', async ({ page }) => {
    await page.goto('/?fresh=1&still=1');
    await expect(page.getByRole('heading', { name: 'בחירת שפה' })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('lang', 'he');
    await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  });
});

test.describe('language default: unsupported system language', () => {
  test.use({ locale: 'fr-FR' });
  test('fr-FR opens in English, LTR, and the picker lists English and עברית', async ({ page }) => {
    await page.goto('/?fresh=1&still=1');
    await expect(page.getByRole('heading', { name: 'Choose a language' })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('lang', 'en');
    await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');
    const options = await page.getByRole('combobox', { name: 'Choose a language' }).locator('option').allTextContents();
    expect(options).toEqual(expect.arrayContaining(['English', 'עברית']));
  });
});
