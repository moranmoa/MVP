// B19: the recovery-words screen (hidden until shown, column order, only the
// wrong check fields marked) and QA-01 (the words can be shown again after a
// reload, so the check can always pass).

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { IntlProvider } from 'react-intl';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';
import en from '../i18n/en.json';
import { RecoveryWordsPanel } from '../screens/onboarding/RecoveryWords';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const WORDS = ['apple', 'river', 'stone', 'cloud', 'tiger', 'piano', 'lemon', 'rocket', 'forest', 'candle', 'silver', 'garden'];

function memoryStorage() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), removeItem: (k: string) => void m.delete(k) };
}

describe('recovery words screen', () => {
  it('keeps the words out of the page until they are shown', () => {
    let done = false;
    render(
      <IntlProvider locale="en" messages={en}>
        <RecoveryWordsPanel words={WORDS} onDone={() => (done = true)} />
      </IntlProvider>,
    );
    for (const w of WORDS) expect(screen.queryByText(w)).toBeNull();
    const written = screen.getByRole('button', { name: 'I wrote them down' }) as HTMLButtonElement;
    expect(written.disabled).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: 'Show the words' }));
    for (const w of WORDS) expect(screen.getByText(w)).toBeTruthy();
    expect(written.disabled).toBe(false);
    fireEvent.click(written);
    expect(done).toBe(true);
  });

  it('keeps the DOM order 1–12, which the CSS lays out down the columns', () => {
    render(
      <IntlProvider locale="en" messages={en}>
        <RecoveryWordsPanel words={WORDS} onDone={() => {}} />
      </IntlProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Show the words' }));
    const shown = [...document.querySelectorAll('.word-grid .word')].map((e) => e.textContent);
    expect(shown).toEqual(WORDS);
  });
});

describe('word check', () => {
  it('does not mark the account checked from fewer than 3 positions', async () => {
    const core = new MockCore({ speed: 0 });
    const { words } = await core.createIdentity({ locale: 'en', country: 'IL', birthDate: '2000-01-01' });
    expect(await core.checkRecoveryWords({ 1: words[0] })).toBe(true);
    expect(await core.checkRecoveryWords({ 1: words[0], 2: words[1] })).toBe(true);
    expect((await core.getIdentity())?.recoveryChecked).toBe(false);
    expect(await core.checkRecoveryWords({ 1: words[0], 2: words[1], 3: words[2] })).toBe(true);
    expect((await core.getIdentity())?.recoveryChecked).toBe(true);
  });

  it('marks only the wrong fields, focuses the first one, and passes once fixed (QA-01)', async () => {
    // An account whose words were never checked, reopened after a reload.
    const storage = memoryStorage();
    const first = new MockCore({ speed: 0, storage });
    const { words } = await first.createIdentity({ locale: 'en', country: 'IL', birthDate: '2000-01-01' });
    const core = new MockCore({ speed: 0, storage });
    render(<App core={core} initialSettings={settings} />);

    fireEvent.click(await screen.findByRole('button', { name: 'Check now' }));
    // The words can be shown again from the reminder.
    fireEvent.click(await screen.findByRole('button', { name: 'Show the words again' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Show the words' }));
    expect([...document.querySelectorAll('.word-grid .word')].map((e) => e.textContent)).toEqual(words);
    fireEvent.click(screen.getByRole('button', { name: 'I wrote them down' }));

    const fields = (await screen.findAllByLabelText(/^Word /)) as HTMLInputElement[];
    const asked = fields.map((f) => Number(f.labels![0].textContent!.match(/\d+/)![0]));
    fireEvent.change(fields[0], { target: { value: words[asked[0] - 1] } });
    fireEvent.change(fields[1], { target: { value: 'nope' } });
    fireEvent.change(fields[2], { target: { value: words[asked[2] - 1] } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Check' }));
    });
    await waitFor(() => expect(fields[1].getAttribute('aria-invalid')).toBe('true'));
    expect(fields[0].getAttribute('aria-invalid')).toBeNull();
    expect(fields[2].getAttribute('aria-invalid')).toBeNull();
    expect(document.activeElement).toBe(fields[1]);
    expect((await core.getIdentity())?.recoveryChecked).toBe(false);

    fireEvent.change(fields[1], { target: { value: words[asked[1] - 1] } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Check' }));
    });
    await waitFor(async () => expect((await core.getIdentity())?.recoveryChecked).toBe(true));
  });
});
