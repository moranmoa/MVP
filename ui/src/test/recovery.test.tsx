// B19 (spec docs/ux/specs/B19-recovery-words.md): the words screen, the
// challenge-based check with its 5-attempt limit, and resuming onboarding
// after a reload (QA-01).

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
const FSI = String.fromCharCode(0x2068);

function memoryStorage() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), removeItem: (k: string) => void m.delete(k) };
}

const shownWords = () => [...document.querySelectorAll('.word-grid .word:not(.word-mask)')].map((e) => e.textContent);

describe('recovery words screen', () => {
  it('keeps the words out of the page until shown, and "written" waits for that', async () => {
    let done = false;
    const { container } = render(
      <IntlProvider locale="en" messages={en}>
        <RecoveryWordsPanel load={async () => WORDS} onDone={() => (done = true)} />
      </IntlProvider>,
    );
    for (const w of WORDS) expect(container.textContent).not.toContain(w);
    const written = screen.getByRole('button', { name: 'I wrote them down' });
    expect(written.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(written);
    expect(done).toBe(false);

    const toggle = screen.getByRole('button', { name: 'Show the words' });
    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(shownWords()).toEqual(WORDS); // DOM order 1-12; CSS lays it out down the columns
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(written.getAttribute('aria-disabled')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Hide the words' }));
    expect(shownWords()).toEqual([]);
    fireEvent.click(written); // still allowed: the words were shown once
    expect(done).toBe(true);
  });

  it('says so when OS verification is cancelled', async () => {
    render(
      <IntlProvider locale="en" messages={en}>
        <RecoveryWordsPanel load={() => Promise.reject({ code: 'auth-cancelled' })} onDone={() => {}} />
      </IntlProvider>,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Show the words' }));
    });
    expect(screen.getByRole('alert').textContent).toContain('Verification was cancelled');
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  });
});

describe('recovery challenge (mock core)', () => {
  it('only accepts the challenge positions, reports wrong ones, and burns after 5 failures', async () => {
    const core = new MockCore({ speed: 0 });
    const { words } = await core.createIdentity({ locale: 'en', country: 'IL', birthDate: '2000-01-01' });
    const { positions } = await core.getRecoveryChallenge();
    expect(positions).toHaveLength(3);
    expect(await core.getRecoveryChallenge()).toEqual({ positions }); // same challenge until solved
    const other = [1, 2, 3, 4].find((p) => !positions.includes(p))!;
    await expect(core.checkRecoveryWords({ [other]: words[other - 1] })).rejects.toMatchObject({ code: 'not-allowed' });

    const wrongAll = Object.fromEntries(positions.map((p) => [p, 'nope']));
    for (let i = 0; i < 4; i++) expect(await core.checkRecoveryWords(wrongAll)).toEqual({ ok: false, wrong: positions });
    // The 5th failure locks right away.
    await expect(core.checkRecoveryWords(wrongAll)).rejects.toMatchObject({ code: 'too-many-attempts' });
    await expect(core.checkRecoveryWords(wrongAll)).rejects.toMatchObject({ code: 'too-many-attempts' });
    await expect(core.getRecoveryChallenge()).rejects.toMatchObject({ code: 'too-many-attempts' });

    await core.getRecoveryWords(); // OS authentication in the real core lifts the burn
    const fresh = await core.getRecoveryChallenge();
    expect(await core.checkRecoveryWords(Object.fromEntries(fresh.positions.map((p) => [p, words[p - 1]])))).toEqual({ ok: true });
    expect((await core.getIdentity())?.recoveryChecked).toBe(true);
  });
});

describe('onboarding after a reload (QA-01)', () => {
  it('resumes on the hidden words, and the check marks only the wrong field', async () => {
    const storage = memoryStorage();
    const first = new MockCore({ speed: 0, storage });
    const { words } = await first.createIdentity({ locale: 'en', country: 'IL', birthDate: '2000-01-01' });
    localStorage.setItem('mvs.onboarding.v1', 'words'); // the app was closed on the words step

    const core = new MockCore({ speed: 0, storage });
    render(<App core={core} initialSettings={settings} />);
    expect(await screen.findByRole('heading', { name: 'Your recovery words' })).toBeTruthy();
    expect(shownWords()).toEqual([]);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Show the words' }));
    });
    expect(shownWords()).toEqual(words);
    fireEvent.click(screen.getByRole('button', { name: 'I wrote them down' }));
    await waitFor(() => expect(localStorage.getItem('mvs.onboarding.v1')).toBe('check'));

    const fields = (await screen.findAllByLabelText(/^Word /)) as HTMLInputElement[];
    const asked = fields.map((f) => Number(f.labels![0].textContent!.match(/\d+/)![0]));
    fields.forEach((f, i) => fireEvent.change(f, { target: { value: i === 1 ? 'nope' : words[asked[i] - 1] } }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Check' }));
    });
    await waitFor(() => expect(fields[1].getAttribute('aria-invalid')).toBe('true'));
    expect(fields[0].getAttribute('aria-invalid')).toBeNull();
    expect(fields[2].getAttribute('aria-invalid')).toBeNull();
    expect(document.activeElement).toBe(fields[1]);
    expect(screen.getByRole('alert').textContent).toContain(`Word ${FSI}${asked[1]}`);
  });

  it('starts a fresh install on the language step', async () => {
    render(<App core={new MockCore({ speed: 0 })} initialSettings={settings} />);
    expect(await screen.findByRole('heading', { name: 'Choose a language' })).toBeTruthy();
    expect(localStorage.getItem('mvs.onboarding.v1')).toBeNull();
  });
});
