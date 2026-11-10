// #304: sign out on this computer, from Settings → Account.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function openSignOut(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  const words = await core.getRecoveryWords();
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('textbox', { name: /^Message / });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Edit profile' }));
  });
  const settings = await screen.findByRole('dialog', { name: 'Settings' });
  await act(async () => {
    fireEvent.click(within(settings).getByRole('button', { name: 'Sign out on this computer' }));
  });
  const dialog = screen.getAllByRole('dialog', { name: 'Sign out on this computer' }).at(-1)!;
  const click = (name: string) =>
    act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name }));
    });
  const fill = (ws: string[]) =>
    ws.forEach((w, i) => fireEvent.change(within(dialog).getByRole('textbox', { name: `Word ${i + 1}` }), { target: { value: w } }));
  return { core, words, dialog, click, fill };
}

describe('sign out (#304)', () => {
  it('explains first: this computer only, not deleted, the words are the only way back', async () => {
    const { dialog } = await openSignOut();
    expect(dialog.textContent).toContain('removed from this computer only');
    expect(dialog.textContent).toContain("The account isn't deleted");
    expect(dialog.textContent).toContain("Without them the account can't be restored.");
  });

  it('with the right words: signs out and lands on create-or-restore', async () => {
    const { core, words, click, fill } = await openSignOut();
    await click('Continue');
    fill(words);
    await click('Sign out on this computer');
    expect(await core.getIdentity()).toBeNull();
    expect(screen.queryByRole('textbox', { name: /^Message / })).toBeNull();
    expect(await screen.findByText('I already have an account')).toBeTruthy();
  });

  it('missing, misspelled, or another account’s words: says so and stays signed in', async () => {
    const { core, words, dialog, click, fill } = await openSignOut();
    await click('Continue');
    await click('Sign out on this computer');
    expect(within(dialog).getByRole('alert').textContent).toBe('Fill in all 12 words.');
    fill([...words.slice(0, 11), 'zzzz']);
    await click('Sign out on this computer');
    expect(within(dialog).getByRole('alert').textContent).toContain('12');
    expect(within(dialog).getByRole('textbox', { name: 'Word 12' }).getAttribute('aria-invalid')).toBe('true');
    fill([...words.slice(1), words[0]]); // real words, not this account's order
    await click('Sign out on this computer');
    expect(within(dialog).getByRole('alert').textContent).toBe("These are another account's words. Use this account's words.");
    expect(await core.getIdentity()).not.toBeNull();
  });

  it('on success the page storage is cleared too', async () => {
    const { words, click, fill } = await openSignOut();
    localStorage.setItem('mvs.notices.voiceIp.v1', '1');
    await click('Continue');
    fill(words);
    await click('Sign out on this computer');
    expect(localStorage.getItem('mvs.notices.voiceIp.v1')).toBeNull();
  });

  it('the keychain refuses: nothing changed, try again', async () => {
    const { core, words, dialog, click, fill } = await openSignOut({ signOutKeyring: true });
    await click('Continue');
    fill(words);
    await click('Sign out on this computer');
    expect(within(dialog).getByRole('alert').textContent).toContain('nothing changed');
    expect(await core.getIdentity()).not.toBeNull();
  });

  it('without the words: a stronger warning, and the button waits for the typed phrase', async () => {
    const { core, dialog, click } = await openSignOut();
    await click('Continue');
    await click("I don't have my words");
    expect(dialog.textContent).toContain("the account can't be restored, on this computer or a new one");
    expect(dialog.textContent).toContain('Other computers still signed in to it keep it.');
    const go = within(dialog).getByRole('button', { name: 'Sign out without words' }) as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    const box = within(dialog).getByRole('textbox', { name: /To confirm, type/ });
    fireEvent.change(box, { target: { value: 'sign out' } });
    expect(go.disabled).toBe(true);
    fireEvent.change(box, { target: { value: ' Sign out without words ' } });
    expect(go.disabled).toBe(false);
    await click('Sign out without words');
    expect(await core.getIdentity()).toBeNull();
  });
});
