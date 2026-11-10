// Server lifecycle (#261): rename, leave, close, the closed (read-only) state,
// the removal toasts and the `update-needed` gate.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const sidebar = () => screen.getByRole('complementary', { name: 'Channels' });
  const rail = () => screen.getByRole('navigation', { name: /servers/i });
  return { core, sidebar, rail };
}

const click = async (el: HTMLElement) =>
  act(async () => {
    fireEvent.click(el);
  });

const openSettings = (sidebar: () => HTMLElement, tab: string) => {
  fireEvent.click(within(sidebar()).getByRole('button', { name: 'Server settings' }));
  fireEvent.click(screen.getByRole('tab', { name: tab }));
  return screen.getByRole('tabpanel');
};

const openMinecraft = async (rail: () => HTMLElement) => {
  fireEvent.click(within(rail()).getByRole('button', { name: /^Minecraft IL/ }));
  await screen.findByRole('heading', { name: 'Minecraft IL' });
};

describe('rename (owner)', () => {
  it('saves a new name and the sidebar shows it', async () => {
    const { sidebar } = await setup();
    const panel = openSettings(sidebar, 'General');
    const field = within(panel).getByLabelText('Server name');
    fireEvent.change(field, { target: { value: '  Gamers HQ  ' } });
    await click(within(panel).getByRole('button', { name: 'Save name' }));
    expect(await within(panel).findByText('Saved.')).toBeTruthy();
    await waitFor(() => expect(within(sidebar()).getByRole('heading', { name: 'Gamers HQ' })).toBeTruthy());
  });

  it('says when the core refuses the name', async () => {
    const { sidebar } = await setup();
    const panel = openSettings(sidebar, 'General');
    fireEvent.change(within(panel).getByLabelText('Server name'), { target: { value: 'evil‮name' } });
    await click(within(panel).getByRole('button', { name: 'Save name' }));
    expect(within(panel).getByRole('alert').textContent).toBe("The name contains characters that can't be used in names.");
  });
});

describe('close (owner)', () => {
  it('needs the typed name, then leaves the server read-only with a banner', async () => {
    const { sidebar } = await setup();
    const panel = openSettings(sidebar, 'Danger zone');
    fireEvent.click(within(panel).getByRole('button', { name: 'Close server…' }));
    const dialog = screen.getByRole('dialog', { name: /^Close .* for good\?$/ });
    expect(within(dialog).getByText(/read it for 30 days/)).toBeTruthy();
    const confirm = within(dialog).getByRole('button', { name: 'Close server' }) as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    const name = within(sidebar()).getByRole('heading', { level: 1 }).textContent!;
    fireEvent.change(within(dialog).getByRole('textbox'), { target: { value: 'wrong' } });
    expect(confirm.disabled).toBe(true);
    fireEvent.change(within(dialog).getByRole('textbox'), { target: { value: name } });
    await click(confirm);

    await screen.findByText('The owner closed this server. You can still read it for 30 days, but no new messages can be sent.');
    expect(screen.queryByRole('textbox', { name: /^Message / })).toBeNull();
    expect(within(sidebar()).queryByRole('button', { name: 'Server settings' })).toBeNull();
    expect(within(sidebar()).queryByRole('button', { name: 'Create channel' })).toBeNull();
  });
});

describe('leave (member)', () => {
  it('asks first, then the server is gone with a toast', async () => {
    const { rail } = await setup();
    fireEvent.contextMenu(within(rail()).getByRole('button', { name: /^Minecraft IL/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Leave server' }));
    const dialog = screen.getByRole('dialog', { name: 'Leave ⁨Minecraft IL⁩?' });
    await click(within(dialog).getByRole('button', { name: 'Leave' }));
    await waitFor(() => expect(within(rail()).queryByRole('button', { name: /^Minecraft IL/ })).toBeNull());
    expect(screen.getAllByRole('status').some((s) => s.textContent === 'You left ⁨Minecraft IL⁩.')).toBe(true);
  });

  it('the owner has no Leave item', async () => {
    const { rail } = await setup();
    fireEvent.contextMenu(within(rail()).getAllByRole('button').find((b) => b.getAttribute('aria-current') === 'page')!);
    expect(screen.queryByRole('menuitem', { name: 'Leave server' })).toBeNull();
  });
});

describe('update-needed', () => {
  it('names the members still on an older version and says removing them is the other way', async () => {
    const { sidebar } = await setup({ outdatedMembers: true });
    const before = within(sidebar()).getByRole('heading', { level: 1 }).textContent;
    const panel = openSettings(sidebar, 'General');
    fireEvent.change(within(panel).getByLabelText('Server name'), { target: { value: 'New name' } });
    await click(within(panel).getByRole('button', { name: 'Save name' }));
    const alert = within(panel).getByRole('alert');
    expect(alert.textContent).toContain('Some members use an older version of MVS');
    // The names come from getUpdateNeeded.
    await waitFor(() => expect(within(alert).getAllByRole('listitem').length).toBe(2));
    expect(alert.textContent).toContain('or remove them from the server');
    // Nothing happened: the name is unchanged.
    expect(within(sidebar()).getByRole('heading', { level: 1 }).textContent).toBe(before);
  });

  it('in the close confirmation, only Close remains once the gate answers', async () => {
    const { sidebar } = await setup({ outdatedMembers: true });
    const panel = openSettings(sidebar, 'Danger zone');
    fireEvent.click(within(panel).getByRole('button', { name: 'Close server…' }));
    const dialog = screen.getByRole('dialog', { name: /^Close .* for good\?$/ });
    const name = within(sidebar()).getByRole('heading', { level: 1 }).textContent!;
    fireEvent.change(within(dialog).getByRole('textbox'), { target: { value: name } });
    await click(within(dialog).getByRole('button', { name: 'Close server' }));
    expect(within(dialog).getByRole('alert').textContent).toContain('or remove them from the server');
    expect(within(dialog).queryByRole('button', { name: 'Close server' })).toBeNull();
    expect(within(dialog).getAllByRole('button', { name: 'Close' }).length).toBeGreaterThan(0);
  });

  it('a leave is never gated', async () => {
    const { rail } = await setup({ outdatedMembers: true });
    fireEvent.contextMenu(within(rail()).getByRole('button', { name: /^Minecraft IL/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Leave server' }));
    await click(within(screen.getByRole('dialog', { name: 'Leave ⁨Minecraft IL⁩?' })).getByRole('button', { name: 'Leave' }));
    await waitFor(() => expect(within(rail()).queryByRole('button', { name: /^Minecraft IL/ })).toBeNull());
  });
});

describe('a closed server', () => {
  it('stays listed, read-only, with no menu and no composer', async () => {
    const { rail } = await setup({ closedServer: true });
    expect(document.querySelector('.voice-room')).not.toBeNull();
    const button = within(rail()).getByRole('button', { name: /^Minecraft IL, Closed/ });
    fireEvent.contextMenu(button);
    expect(screen.queryByRole('menu')).toBeNull();
    await openMinecraft(rail);
    expect(screen.getByRole('status', { name: '' }).textContent).toContain('The owner closed this server');
    expect(screen.queryByRole('textbox', { name: /^Message / })).toBeNull();
    // No voice room either (the open servers have one).
    expect(document.querySelector('.voice-room')).toBeNull();
  });

  it('when it is purged after 30 days, the app moves on with a toast', async () => {
    const { core, rail } = await setup({ closedServer: true });
    const id = (await core.listServers()).find((s) => s.name === 'Minecraft IL')!.id;
    act(() => (core as unknown as { emit: (e: string, p: unknown) => void }).emit('server.removed', { serverId: id, reason: 'closed' }));
    await waitFor(() => expect(within(rail()).queryByRole('button', { name: /^Minecraft IL/ })).toBeNull());
    expect(screen.getAllByRole('status').some((s) => s.textContent === '⁨Minecraft IL⁩ was closed 30 days ago and no longer shows in the app.')).toBe(true);
  });
});

describe('a long name from another client', () => {
  it('a 100-character rename (the core allows it) is shown clipped to 48, in the header and the rail', async () => {
    const { core, sidebar, rail } = await setup();
    const s = (await core.listServers())[0];
    const long = 'L'.repeat(100);
    act(() => (core as unknown as { emit: (e: string, p: unknown) => void }).emit('server.updated', { ...s, name: long }));
    const heading = await within(sidebar()).findByRole('heading', { name: 'L'.repeat(48) });
    expect(heading.querySelector('bdi')).not.toBeNull();
    expect(within(rail()).getByRole('button', { name: /^L{48}(,|$)/ })).toBeTruthy();
  });
});
