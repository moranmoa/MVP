// #359: members' small pictures and chosen colours.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { inkOn } from '../components/common';
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
      fireEvent.click(within(dialog).getByRole('radio', { name: 'Pink' }));
    });
    // #362: one radio is checked and focusable, the rest are not.
    const group = within(dialog).getByRole('radiogroup', { name: 'Colour' });
    const pink = within(group).getByRole('radio', { name: 'Pink' });
    expect(pink.getAttribute('aria-checked')).toBe('true');
    expect(pink.tabIndex).toBe(0);
    expect(within(group).getAllByRole('radio', { checked: true })).toHaveLength(1);
    expect(group.getAttribute('aria-describedby')).toBeTruthy();
    // Arrows move and pick (LTR: right is next).
    act(() => {
      pink.focus();
      fireEvent.keyDown(pink, { key: 'ArrowRight' });
    });
    const purple = within(group).getByRole('radio', { name: 'Purple' });
    expect(purple.getAttribute('aria-checked')).toBe('true');
    expect(document.activeElement).toBe(purple);
    act(() => {
      fireEvent.keyDown(purple, { key: 'ArrowLeft' });
    });
    // A custom colour unticks the palette and ticks "Custom colour".
    const custom = within(dialog).getByLabelText('Custom colour');
    act(() => {
      fireEvent.change(custom, { target: { value: '#FFD43B' } });
    });
    expect(within(group).queryAllByRole('radio', { checked: true })).toHaveLength(0);
    expect(custom.closest('.color-custom')?.classList.contains('is-picked')).toBe(true);
    act(() => {
      fireEvent.click(within(group).getByRole('radio', { name: 'Pink' }));
    });
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    });
    const me = (await core.getIdentity())!;
    expect(me.color).toBe('#e91e63');
    const [server] = await core.listServers();
    expect((await core.listMembers(server.id)).find((m) => m.id === me.id)?.color).toBe('#e91e63');
  });

  it('the letter is dark on light colours and white on dark ones (#362)', () => {
    expect(inkOn('#ffd43b')).toBe('#000');
    expect(inkOn('#ffffff')).toBe('#000');
    expect(inkOn('#1e3a8a')).toBe('#fff');
    expect(inkOn('#000000')).toBe('#fff');
    // AA: whichever is picked is the better of the two, and at least 4.5:1 (AA) on any colour.
    const lum = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255).map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)).reduce((a, c, i) => a + c * [0.2126, 0.7152, 0.0722][i], 0);
    const ratio = (a: string, b: string) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
    for (let v = 0; v < 4096; v++) {
      const hex = '#' + [v >> 8, (v >> 4) & 15, v & 15].map((c) => (c * 17).toString(16).padStart(2, '0')).join('');
      const ink = inkOn(hex);
      const other = ink === '#fff' ? '#000000' : '#ffffff';
      expect(ratio(hex, ink === '#fff' ? '#ffffff' : '#000000')).toBeGreaterThanOrEqual(ratio(hex, other) - 1e-9);
      expect(ratio(hex, ink === '#fff' ? '#ffffff' : '#000000')).toBeGreaterThanOrEqual(4.5);
    }
  });
});
