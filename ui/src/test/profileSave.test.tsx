// Profile save (docs/ux/specs/profile-save.md): one explicit Save, and every
// place that shows me follows at once, from my identity, even when the core
// sends only identity.changed (the real core before 0.4.5 did).

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { Store } from '../app/store';
import { MockCore, type MockOptions } from '../core/mock';
import type { Identity } from '../core/types';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const PINK = 'rgb(233, 30, 99)';

/** As the real core did: only identity.changed, no members.changed. */
const emitIdentity = (core: MockCore, identity: Identity) =>
  (core as unknown as { emit(e: string, p: unknown): void }).emit('identity.changed', identity);

async function setup(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const me = (await core.getIdentity())!;
  const myRowAvatar = () => {
    const list = screen.getByRole('complementary', { name: 'Server members' });
    const row = within(list).getAllByRole('button').find((b) => b.querySelector('.avatar') && b.textContent?.includes(me.displayName))!;
    return row.querySelector<HTMLElement>('.avatar')!;
  };
  const openEditor = async () => {
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Edit profile' }));
    });
    return screen.findByRole('dialog', { name: 'Settings' });
  };
  return { core, me, myRowAvatar, openEditor };
}

describe('profile save (0.4.5)', () => {
  it('the store shows my identity in my row in every server, at once', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    const store = new Store(core);
    store.start();
    await store.init();
    const me = (await core.getIdentity())!;
    act(() => emitIdentity(core, { ...me, displayName: 'Moran Z', color: '#e91e63' }));
    const rows = Object.values(store.getState().members)
      .map((list) => list.find((m) => m.id === me.id))
      .filter(Boolean);
    expect(rows.length).toBeGreaterThan(1);
    for (const m of rows) expect(m).toMatchObject({ displayName: 'Moran Z', color: '#e91e63' });
    // A later members.changed (my old post coming back) doesn't undo it.
    const [server] = await core.listServers();
    const stale = (await core.listMembers(server.id)).map((m) => (m.id === me.id ? { ...m, displayName: 'Old', color: '#123456' } : m));
    act(() => (core as unknown as { emit(e: string, p: unknown): void }).emit('members.changed', { serverId: server.id, members: stale }));
    expect(store.getState().members[server.id].find((m) => m.id === me.id)).toMatchObject({ displayName: 'Moran Z', color: '#e91e63' });
  });

  it('the member list follows identity.changed alone (no restart)', async () => {
    const { core, me, myRowAvatar } = await setup();
    expect(myRowAvatar().style.background).not.toBe(PINK);
    act(() => emitIdentity(core, { ...me, color: '#e91e63' }));
    expect(myRowAvatar().style.background).toBe(PINK);
  });

  it('before Save only the preview changes; Save shows it everywhere, with "others will see it later"', async () => {
    const { myRowAvatar, openEditor } = await setup();
    const dialog = await openEditor();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('radio', { name: 'Pink' }));
    });
    expect(myRowAvatar().style.background).not.toBe(PINK);
    expect(dialog.querySelector<HTMLElement>('.profile-preview-body .avatar')?.style.background).toBe(PINK);
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    });
    expect(myRowAvatar().style.background).toBe(PINK);
    const status = within(dialog).getByRole('status', { name: '' });
    expect(status.textContent).toContain('Profile saved');
    expect(status.textContent).toContain('Others in your servers will see it within a few minutes.');
  });

  it('a failed save keeps the draft and changes nothing else', async () => {
    const { myRowAvatar, openEditor } = await setup({ profileError: true });
    const before = myRowAvatar().style.background;
    const dialog = await openEditor();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('radio', { name: 'Pink' }));
    });
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));
    });
    expect(within(dialog).getByRole('alert').textContent).toBeTruthy();
    expect(within(dialog).getByRole('radio', { name: 'Pink' }).getAttribute('aria-checked')).toBe('true');
    expect(myRowAvatar().style.background).toBe(before);
    expect(dialog.querySelector<HTMLButtonElement>('.profile-actions .primary')?.disabled).toBe(false);
  });
});
