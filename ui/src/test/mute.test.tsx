// Mute a server or a channel on this device (G-03, G-17; GAPS §2.4), from the
// "Notifications" dialog (G-61), and mark a whole server as read (G-04).

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import { Store } from '../app/store';
import type { Settings } from '../app/settings';
import { channelMuteKey, isMuted, liveMutes, loadMutes, MUTE_FOREVER, serverMuteKey } from '../app/mute';
import { MockCore } from '../core/mock';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const menuItems = () => within(screen.getByRole('menu')).getAllByRole('menuitem').map((b) => b.textContent);

beforeEach(() => localStorage.removeItem('mvs.mute.v1'));

async function setup() {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const channels = () => screen.getByRole('navigation', { name: 'Channels' });
  const rail = () => screen.getByRole('navigation', { name: /servers/i });
  return { core, channels, rail };
}

describe('mute a channel', () => {
  it('Notifications asks how long; the row then says Muted, shows 🔕, and the dialog offers Unmute', async () => {
    const { channels } = await setup();
    fireEvent.contextMenu(within(channels()).getByRole('button', { name: /^clips/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Notifications' }));
    const dialog = screen.getByRole('dialog', { name: 'Notifications for ⁨#clips⁩' });
    const mute = within(dialog).getByRole('group', { name: 'Mute' });
    expect(within(mute).getAllByRole('button').map((b) => b.textContent)).toEqual(['For 1 hour', 'For 8 hours', 'For 24 hours', 'Until I turn it back on']);
    fireEvent.click(within(mute).getByRole('button', { name: 'Until I turn it back on' }));
    expect(within(mute).getByRole('status').textContent).toBe('Muted');

    const row = within(channels()).getByRole('button', { name: /^clips, Muted/ });
    expect(row.classList.contains('muted')).toBe(true);
    expect(row.querySelector('.bell-off')).not.toBeNull();
    // Kept on this device.
    expect(loadMutes()).toEqual({ [channelMuteKey('c-clips')]: MUTE_FOREVER });

    fireEvent.click(within(mute).getByRole('button', { name: 'Unmute' }));
    expect(within(channels()).getByRole('button', { name: /^clips/ }).classList.contains('muted')).toBe(false);
    expect(loadMutes()).toEqual({});
    expect(within(mute).getAllByRole('button')).toHaveLength(4);
  });
});

describe('a timed mute says until when', () => {
  it('the menu reads "Notifications · Muted until …" and the 🔕 title says the same', async () => {
    const { channels } = await setup();
    fireEvent.contextMenu(within(channels()).getByRole('button', { name: /^clips/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Notifications' }));
    fireEvent.click(screen.getByRole('button', { name: 'For 1 hour' }));
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    const row = within(channels()).getByRole('button', { name: /^clips, Muted/ });
    expect(row.querySelector('.bell-off-wrap')!.getAttribute('title')).toMatch(/^Muted until ⁨.+⁩$/);
    fireEvent.contextMenu(row);
    expect(menuItems().find((t) => t?.startsWith('Notifications'))).toMatch(/^Notifications · Muted until ⁨.+⁩$/);
  });
});

describe('mute a server, and mark it all as read', () => {
  it('the rail button says Muted; Mark all as read clears every unread channel', async () => {
    const { rail, channels } = await setup();
    const owned = () => within(rail()).getAllByRole('button').find((b) => b.getAttribute('aria-current') === 'page')!;
    fireEvent.contextMenu(owned());
    fireEvent.click(screen.getByRole('menuitem', { name: 'Notifications' }));
    fireEvent.click(screen.getByRole('button', { name: 'For 8 hours' }));
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(owned().getAttribute('aria-label')).toMatch(/, Muted/);
    expect(owned().closest('.rail-item')!.classList.contains('muted')).toBe(true);

    expect(within(channels()).getAllByRole('button').some((b) => b.classList.contains('unread'))).toBe(true);
    fireEvent.contextMenu(owned());
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Mark all as read' }));
    });
    await waitFor(() => expect(within(channels()).getAllByRole('button').some((b) => b.classList.contains('unread'))).toBe(false));
  });
});

describe('the mute state (app/mute.ts)', () => {
  afterEach(() => vi.useRealTimers());

  it("a server's mute covers its channels; a timed mute ends on time", () => {
    const now = 1_000_000;
    const mutes = { [serverMuteKey('s1')]: now + 1000, [channelMuteKey('c9')]: MUTE_FOREVER };
    expect(isMuted(mutes, 's1', 'c1', now)).toBe(true);
    expect(isMuted(mutes, 's2', 'c9', now)).toBe(true);
    expect(isMuted(mutes, 's2', 'c1', now)).toBe(false);
    expect(isMuted(mutes, 's1', undefined, now + 1001)).toBe(false);
    expect(liveMutes(mutes, now + 1001)).toEqual({ [channelMuteKey('c9')]: MUTE_FOREVER });
  });

  it('the store drops a timed mute when it ends, and ignores bad stored data', () => {
    vi.useFakeTimers();
    const store = new Store(new MockCore({ speed: 0 }));
    store.mute(serverMuteKey('s1'), '1h');
    expect(isMuted(store.getState().mutes, 's1')).toBe(true);
    vi.advanceTimersByTime(3_600_000 + 100);
    expect(store.getState().mutes).toEqual({});

    localStorage.setItem('mvs.mute.v1', JSON.stringify({ 'server:ok': MUTE_FOREVER, 'bad key': 0, 'channel:x': 'soon', 'channel:y': -5 }));
    expect(loadMutes()).toEqual({ 'server:ok': MUTE_FOREVER });
  });
});
