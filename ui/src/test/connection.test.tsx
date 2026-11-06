// Direct / relay per online member (#196, T20): an icon by the presence dot,
// and a one-line summary in the profile card.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import type { CoreEvents } from '../core/api';
import { MockCore } from '../core/mock';
import { cleanMember } from '../core/sanitize';
import type { Member } from '../core/types';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup() {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const list = await screen.findByRole('complementary', { name: /members/i });
  return { core, list };
}

describe('connection type', () => {
  it('marks online members as direct or relay, never me or someone offline', async () => {
    const { core, list } = await setup();
    const members = await core.listMembers('s-gamers');
    const online = members.filter((m) => m.presence === 'online' && m.connection);
    expect(list.querySelectorAll('.connection-icon')).toHaveLength(online.length);
    expect(within(list).getAllByTitle('Direct connection').length).toBe(online.filter((m) => m.connection === 'direct').length);
    expect(within(list).getAllByTitle('Via relay').length).toBe(online.filter((m) => m.connection === 'relay').length);
    // Each icon sits right after the avatar (its presence dot) and is drawn, not text.
    for (const icon of list.querySelectorAll('.connection-icon')) {
      expect(icon.previousElementSibling?.classList.contains('avatar')).toBe(true);
      expect(icon.querySelector('svg')).toBeTruthy();
      expect(icon.textContent).toBe('');
    }
    // It is part of what a screen reader hears for the row.
    const direct = online.find((m) => m.connection === 'direct')!;
    expect(screen.getByRole('button', { name: new RegExp(`${direct.displayName}.*Direct connection`) })).toBeTruthy();
  });

  it('says it in one line in the profile card', async () => {
    const { core, list } = await setup();
    const members = await core.listMembers('s-gamers');
    const relay = members.find((m) => m.connection === 'relay')!;
    fireEvent.click(screen.getByRole('button', { name: new RegExp(`${relay.displayName}.*Via relay`) }));
    const card = await screen.findByRole('dialog');
    const line = card.querySelector('.connection-icon.relay')!;
    expect(line.textContent).toBe('Via relay');
    expect(line.getAttribute('title')).toBe("Connected through an MVS relay. It passes encrypted messages along and can't read them.");
    const offline = members.find((m) => m.presence === 'offline')!;
    fireEvent.click(within(card).getByRole('button', { name: /close/i }));
    fireEvent.click(within(list).getByRole('button', { name: `⁨${offline.displayName}⁩'s profile` }));
    expect((await screen.findByRole('dialog')).querySelector('.connection-icon')).toBeNull();
  });

  it('follows members.changed when someone switches to direct', async () => {
    const { core, list } = await setup();
    const members = await core.listMembers('s-gamers');
    const switched = members.map((m) => (m.presence === 'online' ? { ...m, connection: 'direct' as const } : m));
    act(() =>
      (core as unknown as { emit: (e: 'members.changed', p: CoreEvents['members.changed']) => void }).emit('members.changed', {
        serverId: 's-gamers',
        members: switched,
      }),
    );
    expect(within(list).queryAllByTitle('Via relay')).toHaveLength(0);
    expect(within(list).getAllByTitle('Direct connection')).toHaveLength(switched.filter((m) => m.presence === 'online').length);
  });

  it('keeps only a known value, and only for someone online', () => {
    const base: Member = { id: 'u', serverId: 's', displayName: 'x', shortCode: 'AAAA-BBBB', avatarColor: '#123456', presence: 'online', isOwner: false };
    expect(cleanMember({ ...base, connection: 'direct' }).connection).toBe('direct');
    expect(cleanMember({ ...base, connection: 'carrier-pigeon' as never }).connection).toBeNull();
    expect(cleanMember({ ...base, presence: 'offline', connection: 'relay' }).connection).toBeNull();
    expect(cleanMember(base).connection).toBeNull();
  });
});
