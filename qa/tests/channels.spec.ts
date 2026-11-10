// Channels smoke (B11c, #186 UI on the #185 api), against the mock core.
// The owner creates, renames and deletes a channel; a member who isn't the
// owner sees no channel controls; a message stays in the channel it was sent to.

import { expect, test, type Page } from '@playwright/test';
import { composer, createServer, lastMessage, onboard, trackErrors } from './helpers';

const ch = {
  create: 'יצירת ערוץ',
  edit: 'עריכת הערוץ',
  name: 'שם הערוץ',
  save: 'שמירה',
  submit: 'יצירה',
  delete: 'מחיקת הערוץ',
  cannotDeleteDefault: 'אי אפשר למחוק את הערוץ הראשון של השרת',
  deleted: (name: string) => `הערוץ ⁨#${name}⁩ נמחק.`,
};

function channelList(page: Page) {
  return page.getByRole('navigation', { name: 'ערוצים' });
}

/** The channel row's select button (its label starts with the name). */
function channelButton(page: Page, name: string) {
  return channelList(page).getByRole('button', { name: new RegExp(`^${name}(,|$)`) });
}

async function createChannel(page: Page, name: string) {
  await channelList(page).getByRole('button', { name: ch.create }).click();
  const dialog = page.getByRole('dialog', { name: ch.create });
  await dialog.getByLabel(ch.name).fill(name);
  await dialog.getByRole('button', { name: ch.submit, exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(channelButton(page, name)).toBeVisible();
}

async function send(page: Page, text: string) {
  const box = composer(page);
  await box.fill(text);
  await box.press('Enter');
  await expect(box).toHaveValue('');
  await expect(lastMessage(page).locator('.msg-body')).toContainText(text);
}

test.describe('channels', () => {
  test('the owner creates, renames and deletes a text channel', async ({ page }) => {
    const errors = trackErrors(page);
    await onboard(page);
    await createServer(page, 'שרת ערוצים');

    // Create: spaces become '-', and the new channel opens.
    await channelList(page).getByRole('button', { name: ch.create }).click();
    const create = page.getByRole('dialog', { name: ch.create });
    await create.getByLabel(ch.name).fill('qa text');
    await expect(create.getByText('#qa-text')).toBeVisible();
    await create.getByRole('button', { name: ch.submit, exact: true }).click();
    await expect(channelButton(page, 'qa-text')).toBeVisible();
    await expect(channelButton(page, 'qa-text')).toHaveAttribute('aria-current', 'page');
    await expect(composer(page)).toHaveAccessibleName(/qa-text/);

    // A duplicate name is refused in place.
    await channelList(page).getByRole('button', { name: ch.create }).click();
    const dup = page.getByRole('dialog', { name: ch.create });
    await dup.getByLabel(ch.name).fill('qa-text');
    await dup.getByRole('button', { name: ch.submit, exact: true }).click();
    await expect(dup.getByRole('alert')).toHaveText('כבר יש ערוץ בשם הזה.');
    await dup.getByRole('button', { name: 'ביטול' }).click();
    await expect(dup).toHaveCount(0);

    // Rename.
    await channelList(page).getByRole('button', { name: `${ch.edit}: qa-text` }).click();
    const edit = page.getByRole('dialog', { name: ch.edit });
    await edit.getByLabel(ch.name).fill('qa-renamed');
    await edit.getByRole('button', { name: ch.save }).click();
    await expect(edit).toHaveCount(0);
    await expect(channelButton(page, 'qa-renamed')).toBeVisible();
    await expect(channelButton(page, 'qa-text')).toHaveCount(0);

    // Delete, through the confirmation; the open channel shows the notice.
    await channelList(page).getByRole('button', { name: `${ch.edit}: qa-renamed` }).click();
    await page.getByRole('dialog', { name: ch.edit }).getByRole('button', { name: ch.delete }).click();
    const confirm = page.getByRole('dialog', { name: /^למחוק את #/ });
    await expect(confirm).toBeVisible();
    await confirm.getByRole('button', { name: ch.delete }).click();
    await expect(channelButton(page, 'qa-renamed')).toHaveCount(0);
    await expect(page.getByText(ch.deleted('qa-renamed'))).toBeVisible();
    await expect(channelButton(page, 'general')).toBeVisible();
    expect(errors).toEqual([]);
  });

  test('#general can be renamed but not deleted', async ({ page }) => {
    await onboard(page);
    await createServer(page, 'שרת ברירת מחדל');
    await channelList(page).getByRole('button', { name: `${ch.edit}: general` }).click();
    const edit = page.getByRole('dialog', { name: ch.edit });
    await expect(edit.getByText(ch.cannotDeleteDefault)).toBeVisible();
    await expect(edit.getByRole('button', { name: ch.delete })).toHaveCount(0);
  });

  // The v0.2.5 UI has text channels only: Channel has no kind and there is no
  // voice-channel option in the create dialog. Unskip when voice channels land.
  test.fixme('the owner creates, renames and deletes a voice channel', async () => {});

  test('a member who is not the owner sees no channel controls', async ({ page }) => {
    await onboard(page);
    await page.getByRole('button', { name: /^הצטרפות בלינק/ }).click();
    await page.getByLabel('לינק הזמנה').fill('mvs://join/qa-channels-member');
    await page.getByRole('button', { name: 'הצטרפות', exact: true }).click();
    await expect(composer(page)).toBeVisible({ timeout: 10_000 });

    await expect(channelButton(page, 'lfg')).toBeVisible();
    await expect(page.getByRole('button', { name: ch.create })).toHaveCount(0);
    await expect(page.getByRole('button', { name: new RegExp(`^${ch.edit}`) })).toHaveCount(0);
  });

  test('a message stays in the channel it was sent to', async ({ page }) => {
    const errors = trackErrors(page);
    await onboard(page);
    await createServer(page, 'שרת הפרדה');
    await send(page, 'רק ב-general');

    await createChannel(page, 'qa-other');
    await expect(page.getByText('רק ב-general')).toHaveCount(0);
    await send(page, 'רק ב-qa-other');

    await channelButton(page, 'general').click();
    await expect(composer(page)).toHaveAccessibleName(/general/);
    await expect(page.getByText('רק ב-general')).toBeVisible();
    await expect(page.getByText('רק ב-qa-other')).toHaveCount(0);

    await channelButton(page, 'qa-other').click();
    await expect(page.getByText('רק ב-qa-other')).toBeVisible();
    await expect(page.getByText('רק ב-general')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
});
