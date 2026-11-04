// P0 from v0.2.0: copying the invite link did nothing in the installed app.
// The link is now copied through the desktop bridge (window.mvs.copyText),
// the button says whether it worked, and the link is always shown as text.

import { act, fireEvent, render, screen } from '@testing-library/react';
import { IntlProvider } from 'react-intl';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { InviteBox } from '../components/common';
import { MockCore } from '../core/mock';
import en from '../i18n/en.json';

const LINK = `mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}/${'e'.repeat(64)}`;
type Bridge = { mvs?: { copyText?: (t: string) => Promise<boolean> } };

afterEach(() => {
  delete (window as Bridge).mvs;
});

const box = () =>
  render(
    <IntlProvider locale="en" messages={en}>
      <InviteBox link={LINK} />
    </IntlProvider>,
  );

describe('invite link', () => {
  it('copies through the desktop bridge and says so', async () => {
    const copyText = vi.fn(async () => true);
    (window as Bridge).mvs = { copyText };
    box();
    expect(document.querySelector('.invite-link')?.textContent).toBe(LINK); // always shown, selectable
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    });
    expect(copyText).toHaveBeenCalledWith(LINK);
    expect(screen.getByRole('button', { name: 'Copied' })).toBeTruthy();
  });

  it('says when copying failed, and how to copy by hand', async () => {
    (window as Bridge).mvs = { copyText: async () => false };
    box();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    });
    expect(screen.getByRole('status').textContent).toBe("Couldn't copy. Select the link and press Ctrl+C.");
    expect(screen.getByRole('button', { name: 'Copy' })).toBeTruthy(); // not "Copied"
  });

  it('is shown from the server sidebar', async () => {
    const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
    render(<App core={new MockCore({ demo: true, speed: 0, simulateActivity: false })} initialSettings={settings} />);
    const button = await screen.findByRole('button', { name: 'Show the invite link' });
    fireEvent.click(button);
    expect(button.getAttribute('aria-expanded')).toBe('true');
    expect(document.querySelector('#sidebar-invite .invite-link')?.textContent).toMatch(/^mvp:\/\/join\//);
  });
});
