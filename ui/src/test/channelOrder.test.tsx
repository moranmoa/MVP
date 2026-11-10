// G-11 (#323): the owner reorders a server's channels; every member sees the
// core's order.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';
import { cleanChannel } from '../core/sanitize';
import type { Channel } from '../core/types';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const channelList = () => screen.getByRole('navigation', { name: 'Channels' });
const names = () => [...channelList().querySelectorAll<HTMLButtonElement>('li.channel-row button.channel')].map((b) => b.dataset.channelId);

async function setup(core = new MockCore({ demo: true, speed: 0, simulateActivity: false })) {
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('textbox', { name: /^Message / });
  return core;
}

describe('channel order (G-11)', () => {
  it('the sanitizer keeps a valid position key and drops anything else', () => {
    const c = { id: 'c', serverId: 's', name: 'x', adult: false, isDefault: false, unread: false, mentions: 0 } as Channel;
    expect(cleanChannel({ ...c, position: 'aZ9' }).position).toBe('aZ9');
    for (const bad of ['a0', '', 'x'.repeat(33), 'a-b', 'שלום', 7 as unknown as string]) {
      expect('position' in cleanChannel({ ...c, position: bad })).toBe(false);
    }
  });

  it('the owner moves a channel with Alt+Shift+Down: sent as one move, announced, kept', async () => {
    const core = await setup();
    const before = names();
    const row = channelList().querySelector<HTMLButtonElement>(`button.channel[data-channel-id="${before[0]}"]`)!;
    row.focus();
    await act(async () => {
      fireEvent.keyDown(row, { key: 'ArrowDown', altKey: true, shiftKey: true });
    });
    await waitFor(() => expect(names()).toEqual([before[1], before[0], ...before.slice(2)]));
    expect(document.activeElement?.getAttribute('data-channel-id')).toBe(before[0]);
    expect(channelList().querySelector('[aria-live]')?.textContent).toMatch(/moved to position ⁨2⁩ of ⁨\d+⁩$/);
    // The core's own list now has the same order, with positions.
    const [server] = await core.listServers();
    const listed = await core.listChannels(server.id);
    const shown = names();
    expect(listed.map((c) => c.id).filter((id) => shown.includes(id))).toEqual(shown); // the UI may hide some (B7)
    expect(listed.every((c) => typeof c.position === 'string')).toBe(true);
  });

  it('the move names its neighbour: before the channel it lands above', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    const seen: [string, string | undefined, string | undefined][] = [];
    const real = core.moveChannel.bind(core);
    core.moveChannel = (serverId, channelId, before, after) => (seen.push([channelId, before, after]), real(serverId, channelId, before, after));
    await setup(core);
    const before = names();
    fireEvent.contextMenu(channelList().querySelector(`button.channel[data-channel-id="${before[2]}"]`)!);
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Move up' }));
    });
    expect(seen).toEqual([[before[2], before[1], undefined]]);
  });

  it('a refused move goes back and says why', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    core.moveChannel = async () => {
      throw { code: 'not-allowed' };
    };
    await setup(core);
    const before = names();
    fireEvent.contextMenu(channelList().querySelector(`button.channel[data-channel-id="${before[0]}"]`)!);
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Move down' }));
    });
    await waitFor(() => expect(names()).toEqual(before));
    expect(screen.getAllByRole('status').some((s) => s.textContent?.startsWith('Only the owner can change the channel order.'))).toBe(true);
  });

  it('not the owner: no handle, no keyboard move, no move items', async () => {
    await setup();
    // Minecraft IL is someone else's server in the demo.
    await act(async () => {
      fireEvent.click(within(screen.getByRole('navigation', { name: /servers/i })).getByRole('button', { name: /^Minecraft IL/ }));
    });
    await screen.findByRole('textbox', { name: /^Message / });
    expect(channelList().querySelector('.channel-handle')).toBeNull();
    const before = names();
    const row = channelList().querySelector<HTMLButtonElement>('button.channel')!;
    row.focus();
    await act(async () => {
      fireEvent.keyDown(row, { key: 'ArrowDown', altKey: true, shiftKey: true });
    });
    expect(names()).toEqual(before);
    fireEvent.contextMenu(row);
    expect(screen.queryByRole('menuitem', { name: 'Move down' })).toBeNull();
  });
});
