// #359: members' small pictures and chosen colours.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';
import { cleanHexColor, cleanMember, cleanSmallAvatar } from '../core/sanitize';
import type { Member } from '../core/types';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const PNG = 'data:image/png;base64,iVBORw0KGgo=';

describe('avatars and colours (#359)', () => {
  it('the sanitizer keeps only small WebP/PNG pictures and #rrggbb colours', () => {
    expect(cleanSmallAvatar(PNG)).toBe(PNG);
    expect(cleanSmallAvatar('data:image/webp;base64,UklGRg==')).toBeTruthy();
    for (const bad of [
      'data:image/svg+xml;base64,PHN2Zz4=',
      'data:image/jpeg;base64,/9j/',
      'https://tracker.invalid/x.png',
      `data:image/png;base64,${'A'.repeat(3600)}`,
      'data:image/png;base64,<script>',
    ]) {
      expect(cleanSmallAvatar(bad)).toBeUndefined();
    }
    expect(cleanHexColor('#A1B2C3')).toBe('#a1b2c3');
    for (const bad of ['red', '#abc', 'url(x)', '#12345g']) expect(cleanHexColor(bad)).toBeUndefined();
    const m = cleanMember({ id: 'u', serverId: 's', displayName: 'D', shortCode: 'AB12-CD34', avatarColor: '#123456', presence: 'online', avatarDataUrl: 'javascript:x', color: 'blue' } as unknown as Member);
    expect('avatarDataUrl' in m || 'color' in m).toBe(false);
  });

  it('a member picture shows as <img src> in the member list; the colour backs the letter otherwise', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    const [server] = await core.listServers();
    const target = (await core.listMembers(server.id)).find((m) => m.displayName === 'Tal')!;
    // As a profile post would bring it.
    const s = (core as unknown as { servers: Map<string, { members: Member[] }> }).servers.get(server.id)!;
    s.members = s.members.map((m) => (m.id === target.id ? { ...m, avatarDataUrl: PNG, color: '#e91e63' } : m));
    render(<App core={core} initialSettings={en} />);
    await screen.findByRole('textbox', { name: /^Message / });
    const list = screen.getByRole('complementary', { name: 'Server members' });
    const row = within(list).getAllByRole('button').find((b) => b.textContent?.includes('Tal'))!;
    expect(row.querySelector('img')?.getAttribute('src')).toBe(PNG);
  });

  it('Account: picking a colour saves it with updateProfile, and my member entry follows', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    render(<App core={core} initialSettings={en} />);
    await screen.findByRole('textbox', { name: /^Message / });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Edit profile' }));
    });
    const dialog = await screen.findByRole('dialog', { name: 'Settings' });
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: '#e91e63' }));
    });
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    });
    const me = (await core.getIdentity())!;
    expect(me.color).toBe('#e91e63');
    const [server] = await core.listServers();
    expect((await core.listMembers(server.id)).find((m) => m.id === me.id)?.color).toBe('#e91e63');
  });
});
