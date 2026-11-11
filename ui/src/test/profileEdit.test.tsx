// B11e: edit my profile from Settings → Account (name, avatar, bio).

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
// A 1×1 PNG.
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

async function openAccount(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('textbox', { name: /^Message / });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Edit profile' }));
  });
  const dialog = await screen.findByRole('dialog', { name: 'Settings' });
  const name = within(dialog).getByRole('textbox', { name: 'Display name' }) as HTMLInputElement;
  const save = () => within(dialog).getByRole('button', { name: /^(Save|Saving…|Try again)$/ }) as HTMLButtonElement;
  return { core, dialog, name, save };
}

const type = (el: HTMLElement, value: string) => fireEvent.change(el, { target: { value } });

describe('edit profile (B11e)', () => {
  it('the panel with my name opens Account; Save waits for a change', async () => {
    const { dialog, name, save } = await openAccount();
    expect(within(dialog).getByRole('heading', { name: 'Account' })).toBeTruthy();
    expect(name.value).toBe('מורן');
    expect(save().disabled).toBe(true);
    expect(within(dialog).getByText('Everyone in your servers sees your name, picture and colour. For now, your bio is seen only on this computer.')).toBeTruthy();
  });

  it('saves name and bio: my panel and the member list follow', async () => {
    const { core, dialog, name, save } = await openAccount();
    type(name, 'Moran B');
    type(within(dialog).getByRole('textbox', { name: /Bio/ }), 'Plays at night');
    expect(save().disabled).toBe(false);
    expect(within(dialog).getByRole('img', { name: 'Unsaved changes' })).toBeTruthy();
    await act(async () => {
      fireEvent.click(save());
    });
    expect(await within(dialog).findByRole('status')).toHaveProperty('textContent', 'Profile savedOthers in your servers will see it within a few minutes.');
    const me = await core.getIdentity();
    expect(me).toMatchObject({ displayName: 'Moran B', bio: 'Plays at night' });
    const [server] = await core.listServers();
    expect((await core.listMembers(server.id)).find((m) => m.id === me!.id)?.displayName).toBe('Moran B');
    expect(screen.getAllByText('Moran B').length).toBeGreaterThan(1); // preview and my panel
    expect(save().disabled).toBe(true);
    expect(within(dialog).queryByRole('img', { name: 'Unsaved changes' })).toBeNull();
  });

  it('a name of only invisible characters is refused before the core, with focus on the field', async () => {
    const { core, dialog, name, save } = await openAccount();
    type(name, '​​');
    await act(async () => {
      fireEvent.click(save());
    });
    expect(within(dialog).getByRole('alert').textContent).toBe('Enter a name.');
    expect(document.activeElement).toBe(name);
    expect((await core.getIdentity())!.displayName).toBe('מורן');
  });

  it('a failed save says so, keeps what was typed, and offers Try again', async () => {
    const { core, dialog, name, save } = await openAccount({ profileError: true });
    type(name, 'Kept');
    await act(async () => {
      fireEvent.click(save());
    });
    expect(within(dialog).getByRole('alert').textContent).toBe("Couldn't save your profile. Try again.");
    expect(name.value).toBe('Kept');
    expect(save().textContent).toBe('Try again');
    expect((await core.getIdentity())!.displayName).toBe('מורן');
  });

  it('picks and removes an avatar; a file of another type is refused and the picture stays', async () => {
    const { core, dialog, save } = await openAccount();
    const input = within(dialog).getByLabelText('Profile picture') as HTMLInputElement;
    const png = new File([Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0))], 'me.png', { type: 'image/png' });
    await act(async () => {
      fireEvent.change(input, { target: { files: [png] } });
    });
    await waitFor(() => expect(save().disabled).toBe(false));
    await act(async () => {
      fireEvent.click(save());
    });
    expect((await core.getIdentity())!.avatarDataUrl).toBe(`data:image/png;base64,${PNG}`);
    await act(async () => {
      fireEvent.change(input, { target: { files: [new File(['x'], 'a.svg', { type: 'image/svg+xml' })] } });
    });
    expect(await within(dialog).findByText("Can't open this file. Choose a PNG, JPEG, WebP or GIF image.")).toBeTruthy();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: 'Remove' }));
    });
    await act(async () => {
      fireEvent.click(save());
    });
    expect((await core.getIdentity())!.avatarDataUrl).toBeUndefined();
  });

  it('leaving Account with unsaved changes asks: keep editing, discard, or save', async () => {
    const { core, dialog, name } = await openAccount();
    type(name, 'Draft');
    const goTo = (s: string) =>
      act(async () => {
        fireEvent.click(within(dialog).getByRole('button', { name: s }));
      });
    await goTo('Appearance');
    const ask = screen.getByRole('dialog', { name: 'Save your profile changes?' });
    await act(async () => {
      fireEvent.click(within(ask).getByRole('button', { name: 'Keep editing' }));
    });
    expect(name.value).toBe('Draft');
    await goTo('Appearance');
    await act(async () => {
      fireEvent.click(within(screen.getByRole('dialog', { name: 'Save your profile changes?' })).getByRole('button', { name: 'Discard' }));
    });
    expect(within(dialog).getByRole('heading', { name: 'Appearance' })).toBeTruthy();
    expect((await core.getIdentity())!.displayName).toBe('מורן');
    await goTo('Account');
    const again = within(dialog).getByRole('textbox', { name: 'Display name' }) as HTMLInputElement;
    expect(again.value).toBe('מורן');
    type(again, 'Saved on the way out');
    await goTo('Appearance');
    await act(async () => {
      fireEvent.click(within(screen.getByRole('dialog', { name: 'Save your profile changes?' })).getByRole('button', { name: 'Save' }));
    });
    expect((await core.getIdentity())!.displayName).toBe('Saved on the way out');
    expect(within(dialog).getByRole('heading', { name: 'Appearance' })).toBeTruthy();
  });
});
