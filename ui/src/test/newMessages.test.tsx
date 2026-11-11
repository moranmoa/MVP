// G-21 (docs/ux/GAPS.md §2.3): where the new messages start, and a jump to them.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';
import { cleanChannel } from '../core/sanitize';
import type { Channel } from '../core/types';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const channelList = () => screen.getByRole('navigation', { name: 'Channels' });

async function open(core: MockCore, name: RegExp) {
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('textbox', { name: /^Message / });
  await act(async () => {
    fireEvent.click(within(channelList()).getByRole('button', { name }));
  });
}
const unreadOf = async (core: MockCore, id: string) => {
  const [server] = await core.listServers();
  return (await core.listChannels(server.id)).find((c) => c.id === id)!;
};

describe('new messages (G-21)', () => {
  it('the sanitizer keeps sane counts and drops anything else', () => {
    const c = { id: 'c', serverId: 's', name: 'x', adult: false, isDefault: false, unread: true, mentions: 0 } as Channel;
    expect(cleanChannel({ ...c, firstUnreadIndex: 88, unreadCount: 12 })).toMatchObject({ firstUnreadIndex: 88, unreadCount: 12 });
    for (const bad of [-1, 1.5, '3', Number.NaN, 2 ** 31]) {
      const out = cleanChannel({ ...c, firstUnreadIndex: bad as number, unreadCount: bad as number });
      expect('firstUnreadIndex' in out || 'unreadCount' in out).toBe(false);
    }
  });

  it('300 new: opens at the bottom with "300 new messages · Jump"; ✕ reads the channel', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    await open(core, /^clips/);
    const bar = await screen.findByRole('button', { name: '300 new messages · Jump' });
    expect(bar).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Mark as read' }));
    });
    expect(screen.queryByRole('button', { name: /new messages · Jump/ })).toBeNull();
    await waitFor(async () => expect((await unreadOf(core, 'c-clips')).unread).toBe(false));
  });

  it('Esc in the messages reads the channel', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    await open(core, /^clips/);
    await screen.findByRole('button', { name: /new messages · Jump/ });
    await act(async () => {
      fireEvent.keyDown(screen.getByRole('feed'), { key: 'Escape' });
    });
    await waitFor(async () => expect((await unreadOf(core, 'c-clips')).unread).toBe(false));
  });

  it('a few new: the divider sits right above the first new message', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    // A short #general with its last 3 messages new (jsdom draws only the first rows).
    const ch = (core as unknown as { channelIndex: Map<string, { baseCount: number; channel: Channel }> }).channelIndex.get('c-general')!;
    ch.baseCount = 6;
    ch.channel = { ...ch.channel, unread: true, firstUnreadIndex: 3, unreadCount: 3 };
    render(<App core={core} initialSettings={en} />);
    await screen.findByRole('textbox', { name: /^Message / });
    const divider = await screen.findByRole('separator', { name: 'New' });
    expect(divider.closest('[data-index]')?.getAttribute('data-index')).toBe('3');
    // (Whether the bar shows depends on real row heights: jsdom measures every row 800px,
    // so that part is checked in the screenshot run, 340.)
  });
});

describe('read while watching (core sends channel.updated on new messages)', () => {
  it('a new message from someone else in the open channel, at the bottom, is read at once', async () => {
    // The window has focus (jsdom's document.hasFocus() is false on its own).
    const focus = vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    render(<App core={core} initialSettings={en} />);
    await screen.findByRole('textbox', { name: /^Message / });
    let reads = 0;
    const real = core.markRead.bind(core);
    // The store's guard reads the channel: make the open one unread, as the core now announces.
    const ch = (core as unknown as { channelIndex: Map<string, { channel: Channel }> }).channelIndex.get('c-general')!;
    core.markRead = (id) => (reads++, real(id));
    await act(async () => {
      ch.channel = { ...ch.channel, unread: true, unreadCount: 1, firstUnreadIndex: 99_999 };
      (core as unknown as { emit: (e: string, p: unknown) => void }).emit('channel.updated', ch.channel);
    });
    await waitFor(async () => expect((await unreadOf(core, 'c-general')).unread).toBe(false));
    expect(reads).toBeGreaterThan(0);
    focus.mockRestore();
  });
});
