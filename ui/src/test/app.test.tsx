import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import { MockCore } from '../core/mock';
import type { Settings } from '../app/settings';

const he: Settings = { locale: 'he', theme: 'light', uiScale: 1, fontSize: 15 };
const en: Settings = { ...he, locale: 'en' };

describe('app', () => {
  it('runs onboarding in Hebrew, RTL, and blocks under 16 calmly', async () => {
    render(<App core={new MockCore({ speed: 0 })} initialSettings={he} />);
    expect(await screen.findByRole('heading', { name: 'בחירת שפה' })).toBeTruthy();
    expect(document.documentElement.dir).toBe('rtl');

    fireEvent.click(screen.getByRole('button', { name: 'המשך' }));
    const next = screen.getByRole('button', { name: 'המשך' }) as HTMLButtonElement;
    expect(next.disabled).toBe(true);
    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(next);
    fireEvent.click(screen.getByRole('button', { name: 'המשך' })); // country is prefilled

    const year = String(new Date().getFullYear() - 10);
    fireEvent.change(screen.getByLabelText('יום'), { target: { value: '1' } });
    fireEvent.change(screen.getByLabelText('חודש'), { target: { value: '1' } });
    fireEvent.change(screen.getByLabelText('שנה'), { target: { value: year } });
    fireEvent.click(screen.getByRole('button', { name: 'המשך' }));
    expect(await screen.findByRole('heading', { name: 'עוד קצת מוקדם' })).toBeTruthy();
  });

  it('switches to English and LTR', async () => {
    render(<App core={new MockCore({ speed: 0 })} initialSettings={he} />);
    fireEvent.click(await screen.findByRole('radio', { name: 'English' }));
    expect(await screen.findByRole('heading', { name: 'Choose a language' })).toBeTruthy();
    expect(document.documentElement.dir).toBe('ltr');
  });

  it('shows the shell with servers, channels, messages and members', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0 })} initialSettings={en} />);
    expect(await screen.findByRole('navigation', { name: 'Servers' })).toBeTruthy();
    await waitFor(() => expect(screen.getAllByRole('article').length).toBeGreaterThan(0));
    // Only a window of the 100k messages is in the DOM.
    expect(screen.getAllByRole('article').length).toBeLessThan(200);
    expect(screen.getByRole('complementary', { name: 'Server members' })).toBeTruthy();
    expect(screen.getByRole('button', { name: /^כללי/ })).toBeTruthy();
  });

  it('sends a message from the composer', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    render(<App core={core} initialSettings={en} />);
    const box = await screen.findByRole('textbox', { name: /Message #/ });
    fireEvent.change(box, { target: { value: 'hello from the test' } });
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    expect(await core.getMessageCount('c-general')).toBe(100_001);
    const last = (await core.getMessages('c-general', 100_000, 100_001)).messages[0];
    expect(last.body).toBe('hello from the test');
  });

  it('shows the nobody-online banner on a server with no one connected', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0 })} initialSettings={en} />);
    fireEvent.click(await screen.findByRole('button', { name: /^החבר׳ה/ }));
    expect(await screen.findByText(/Nobody from the server is online right now/)).toBeTruthy();
  });
});
