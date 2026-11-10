// Restoring an account (#290): the restore asks the agreement, country and
// birth date again (spec, age: "restore asks again"), sends them with the words,
// then asks for the name, and ends on a notice that servers don't come back by
// themselves yet.

import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';
import { WORDS } from '../core/mockData';

const he: Settings = { locale: 'he', theme: 'light', uiScale: 1, fontSize: 15 };

async function toDob(year: string) {
  fireEvent.click(await screen.findByRole('button', { name: 'כבר יש לי חשבון' }));
  // The agreement, as for a new account.
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: 'המשך' }));
  fireEvent.click(screen.getByRole('button', { name: 'המשך' })); // country is prefilled
  fireEvent.change(screen.getByLabelText('יום'), { target: { value: '12' } });
  fireEvent.change(screen.getByLabelText('חודש'), { target: { value: '4' } });
  fireEvent.change(screen.getByLabelText('שנה'), { target: { value: year } });
  fireEvent.click(screen.getByRole('button', { name: 'המשך' }));
}

describe('restore', () => {
  it('asks the agreement, country and birth date first, sends them with the words, then the name, then the notice', async () => {
    const core = new MockCore({ speed: 0 });
    const restore = vi.spyOn(core, 'restoreIdentity');
    render(<App core={core} initialSettings={he} />);
    await toDob('2001');

    const words = WORDS.slice(0, 12);
    words.forEach((w, i) => fireEvent.change(screen.getByLabelText(`מילה ${i + 1}`, { exact: true }), { target: { value: w } }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'שחזור' }));
    });
    expect(restore).toHaveBeenCalledWith(words, { locale: 'he', country: expect.any(String), birthDate: '2001-04-12' });

    // The profile step, as for a new account.
    fireEvent.change(await screen.findByLabelText('שם תצוגה'), { target: { value: 'מורן' } });
    await act(async () => {
      fireEvent.submit(screen.getByLabelText('שם תצוגה').closest('form')!);
    });
    expect((await core.getIdentity())?.displayName).toBe('מורן');

    // Servers don't come back by themselves yet: say so, and how to return.
    expect(await screen.findByRole('heading', { name: 'החשבון שוחזר' })).toBeTruthy();
    expect(screen.getByText(/לינק הזמנה/).textContent).toContain('כאותו חבר, עם אותו תפקיד');
    fireEvent.click(screen.getByRole('button', { name: 'המשך' }));
    expect(await screen.findByRole('navigation', { name: 'שרתים' })).toBeTruthy();
  });

  it('under 16, the restore stops at the same calm block as a new account', async () => {
    const core = new MockCore({ speed: 0 });
    const restore = vi.spyOn(core, 'restoreIdentity');
    render(<App core={core} initialSettings={he} />);
    await toDob(String(new Date().getFullYear() - 10));
    expect(await screen.findByRole('heading', { name: 'עוד קצת מוקדם' })).toBeTruthy();
    expect(restore).not.toHaveBeenCalled();
  });
});
