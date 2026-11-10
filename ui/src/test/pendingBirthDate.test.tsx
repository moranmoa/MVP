// B7 §2 / §9 (#306): a birth-date correction that raises my age waits 30 days;
// Settings → Account shows it.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';
import { cleanIdentity } from '../core/sanitize';
import type { Identity } from '../core/types';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function openAccount(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('textbox', { name: /^Message / });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Edit profile' }));
  });
  return within(await screen.findByRole('dialog', { name: 'Settings' }));
}

describe('pending birth-date correction (B7, #306)', () => {
  const base = { id: 'me', shortCode: 'AB12-CD34', displayName: 'Me', avatarColor: '#123456', country: 'IL', birthDate: '1998-04-12', recoveryChecked: true } as Identity;

  it('cleanIdentity keeps a well-formed pendingBirthDate and drops anything else', () => {
    const ok = { date: '1996-04-12', effectiveAt: 1_800_000_000_000 };
    expect(cleanIdentity({ ...base, pendingBirthDate: ok }).pendingBirthDate).toEqual(ok);
    expect('pendingBirthDate' in cleanIdentity(base)).toBe(false);
    for (const bad of [
      { date: '1996-02-30', effectiveAt: 1 }, // not a real day
      { date: '12/04/1996', effectiveAt: 1 },
      { date: '1996-04-12', effectiveAt: '1' },
      { date: '1996-04-12', effectiveAt: Number.NaN },
      { date: '1996-04-12', effectiveAt: -5 },
      { date: '1996-04-12' },
      'tomorrow',
    ]) {
      expect('pendingBirthDate' in cleanIdentity({ ...base, pendingBirthDate: bad as never })).toBe(false);
    }
  });

  it('Account shows my date of birth, and no pending line without a correction', async () => {
    const account = await openAccount();
    expect(account.getByRole('heading', { name: 'Date of birth' })).toBeTruthy();
    expect(account.getByText('April 12, 1998')).toBeTruthy();
    expect(account.queryByText(/Pending correction/)).toBeNull();
  });

  it('a waiting correction shows the new date and when it takes effect', async () => {
    const account = await openAccount({ pendingDob: true });
    const line = account.getByText(/Pending correction/);
    expect(line.textContent).toContain('April 12, 1996');
    expect(line.textContent).toMatch(/from .*\d{4}/);
    // The current date stays until then.
    expect(account.getByText('April 12, 1998')).toBeTruthy();
  });
});
