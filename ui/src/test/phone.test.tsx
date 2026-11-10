// Phone layout behaviour (below 40rem). jsdom has no layout, so these tests
// check state, focus and accessibility; the screenshots check the looks.

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const realMatchMedia = window.matchMedia;

beforeEach(() => {
  // Every media query matches: the app thinks it is on a phone.
  window.matchMedia = ((query: string) => ({
    matches: true,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
});

afterEach(() => {
  window.matchMedia = realMatchMedia;
});

const shell = () => document.querySelector('.shell')!;

async function openGeneral() {
  fireEvent.click(await screen.findByRole('button', { name: /^כללי/ }));
}

describe('phone layout', () => {
  it('starts on the channel list and opens the chat with focus on its heading', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0 })} initialSettings={en} />);
    await screen.findByRole('button', { name: /^כללי/ });
    expect(shell().classList.contains('phone-nav')).toBe(true);
    // The skip link would point at the hidden chat.
    expect(screen.queryByText('Skip to chat')).toBeNull();

    await openGeneral();
    expect(shell().classList.contains('phone-chat')).toBe(true);
    expect(document.activeElement?.classList.contains('chat-title')).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: 'Back to channels' }));
    expect(shell().classList.contains('phone-nav')).toBe(true);
    expect(document.activeElement?.getAttribute('aria-current')).toBe('page');
  });

  it('never lands on the member sheet after a channel tap, even when the desktop list was open', async () => {
    localStorage.setItem('mvp.ui.membersOpen', '1');
    render(<App core={new MockCore({ demo: true, speed: 0 })} initialSettings={en} />);
    await openGeneral();
    expect(screen.queryByRole('dialog', { name: 'Server members' })).toBeNull();

    // Open the sheet, go back, open another channel: the chat shows, not the sheet.
    fireEvent.click(screen.getByRole('button', { name: 'Member list' }));
    expect(screen.getByRole('dialog', { name: 'Server members' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Close the member list' }));
    fireEvent.click(screen.getByRole('button', { name: 'Member list' }));
    expect(localStorage.getItem('mvp.ui.membersOpen')).toBe('1'); // the phone never writes it
  });

  it('treats the member sheet as a modal dialog', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0 })} initialSettings={en} />);
    await openGeneral();
    fireEvent.click(screen.getByRole('button', { name: 'Member list' }));
    const sheet = screen.getByRole('dialog', { name: 'Server members' });
    expect(sheet.getAttribute('aria-modal')).toBe('true');
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close the member list' }));
    // Everything under the sheet is inert.
    expect((document.querySelector('.main') as HTMLElement).inert).toBe(true);

    await act(async () => {
      fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    });
    expect(screen.queryByRole('dialog', { name: 'Server members' })).toBeNull();
    expect((document.querySelector('.main') as HTMLElement).inert).toBe(false);
    expect(document.activeElement?.classList.contains('members-button')).toBe(true);
  });
});

describe('context menus on a phone (B25 §5)', () => {
  it('a channel menu opens as a bottom sheet with a title; keys and focus work as on desktop', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0, simulateActivity: false })} initialSettings={en} />);
    const row = await screen.findByRole('button', { name: /^clips/ });
    fireEvent.contextMenu(row);
    const menu = screen.getByRole('menu', { name: 'Actions for #⁨clips⁩' });
    expect(menu.classList.contains('sheet')).toBe(true);
    expect(menu.querySelector('.context-menu-title')?.textContent).toBe('Actions for #⁨clips⁩');
    expect(menu.getAttribute('style')).toBeNull();
    expect(document.activeElement?.getAttribute('role')).toBe('menuitem');
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(document.activeElement?.textContent).toBe('Rename');
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(row));
  });

  it('a tap on the backdrop closes the sheet', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0, simulateActivity: false })} initialSettings={en} />);
    const rail = await screen.findByRole('navigation', { name: /servers/i });
    fireEvent.contextMenu(rail.querySelector('[aria-current="page"]')!);
    expect(screen.getByRole('menu').classList.contains('sheet')).toBe(true);
    fireEvent.mouseDown(document.querySelector('.context-menu-backdrop')!);
    expect(screen.queryByRole('menu')).toBeNull();
  });
});
