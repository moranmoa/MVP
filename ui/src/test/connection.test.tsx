// Direct / relay per online member (#196): a small tag in the member list.

import { act, render, screen, within } from '@testing-library/react';
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
  it('tags online members as direct or relay, never me or someone offline', async () => {
    const { core, list } = await setup();
    const members = await core.listMembers('s-gamers');
    const online = members.filter((m) => m.presence === 'online' && m.connection);
    const tags = list.querySelectorAll('.connection-tag');
    expect(tags.length).toBe(online.length);
    expect(within(list).getAllByText('Direct').length).toBeGreaterThan(0);
    expect(within(list).getAllByText('Relay').length).toBeGreaterThan(0);
    // The tag is part of what a screen reader hears for the row.
    const direct = online.find((m) => m.connection === 'direct')!;
    expect(screen.getByRole('button', { name: new RegExp(`${direct.displayName}.*Connected directly`) })).toBeTruthy();
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
    expect(within(list).queryAllByText('Relay')).toHaveLength(0);
  });

  it('keeps only a known value, and only for someone online', () => {
    const base: Member = { id: 'u', serverId: 's', displayName: 'x', shortCode: 'AAAA-BBBB', avatarColor: '#123456', presence: 'online', isOwner: false };
    expect(cleanMember({ ...base, connection: 'direct' }).connection).toBe('direct');
    expect(cleanMember({ ...base, connection: 'carrier-pigeon' as never }).connection).toBeNull();
    expect(cleanMember({ ...base, presence: 'offline', connection: 'relay' }).connection).toBeNull();
    expect(cleanMember(base).connection).toBeNull();
  });
});
