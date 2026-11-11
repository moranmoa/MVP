// B11 server settings (docs/ux/specs/B11-server-settings.md) and B25 context
// menus (docs/ux/specs/B25-context-menus.md).

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup() {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const channels = () => screen.getByRole('navigation', { name: 'Channels' });
  const sidebar = () => screen.getByRole('complementary', { name: 'Channels' });
  return { core, channels, sidebar };
}

const menuItems = () => within(screen.getByRole('menu')).getAllByRole('menuitem').map((b) => b.textContent);

const openOtherServer = async (channels: () => HTMLElement) => {
  fireEvent.click(screen.getByRole('button', { name: /^Minecraft IL/ }));
  await waitFor(() => expect(within(channels()).getByRole('button', { name: /^general/ })).toBeTruthy());
};

describe('server settings (B11)', () => {
  it('shows the gear to the owner only', async () => {
    const { channels, sidebar } = await setup();
    expect(within(sidebar()).getByRole('button', { name: 'Server settings' })).toBeTruthy();
    await openOtherServer(channels);
    expect(within(sidebar()).queryByRole('button', { name: 'Server settings' })).toBeNull();
  });

  it('opens on General; the arrows move between pages and the heading takes focus', async () => {
    const { sidebar } = await setup();
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Server settings' }));
    const dialog = screen.getByRole('dialog');
    const tabs = within(dialog).getByRole('tablist');
    expect(tabs.getAttribute('aria-orientation')).toBe('vertical');
    const general = within(tabs).getByRole('tab', { name: 'General' });
    expect(general.getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(general);
    expect(within(dialog).getByText('Server name')).toBeTruthy();

    fireEvent.keyDown(general, { key: 'ArrowDown' });
    const channelsTab = within(tabs).getByRole('tab', { name: 'Channels' });
    expect(document.activeElement).toBe(channelsTab);
    fireEvent.click(channelsTab);
    expect(document.activeElement).toBe(within(dialog).getByRole('heading', { name: 'Channels', level: 3 }));
    expect(within(dialog).getByRole('button', { name: 'Edit channel: clips' })).toBeTruthy();

    fireEvent.click(within(tabs).getByRole('tab', { name: 'Invite link' }));
    expect(within(dialog).getByRole('button', { name: 'Copy' })).toBeTruthy();

    fireEvent.click(within(tabs).getByRole('tab', { name: 'Members' }));
    expect(within(dialog).getAllByRole('button', { name: 'Remove from server' }).length).toBeGreaterThan(0);
  });

  it('creates a channel from the Channels page', async () => {
    const { channels, sidebar } = await setup();
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Server settings' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Channels' }));
    fireEvent.click(within(screen.getByRole('tabpanel')).getByRole('button', { name: 'Create channel' }));
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'news' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    });
    await within(channels()).findByRole('button', { name: /^news/ });
  });
});

describe('context menus (B25)', () => {
  it('channel, owner: mark read, rename, copy ID and delete; Esc closes and focus goes back to the row', async () => {
    const { channels } = await setup();
    const row = within(channels()).getByRole('button', { name: /^clips/ });
    expect(row.getAttribute('aria-haspopup')).toBe('menu');
    fireEvent.contextMenu(row, { clientX: 10, clientY: 10 });
    expect(screen.getByRole('menu', { name: 'Actions for ⁨#clips⁩' })).toBeTruthy();
    // G-11: the owner's move items have their own group, before Delete.
    expect(menuItems()).toEqual(['Mark as read', 'Rename', 'Copy channel ID', 'Move up', 'Move down', 'Delete channel']);
    expect(screen.getAllByRole('separator').length).toBe(3);
    expect(document.activeElement?.textContent).toBe('Mark as read');

    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(row));
  });

  it('#general has no delete; the arrows wrap and skip the separators', async () => {
    const { channels } = await setup();
    const row = within(channels()).getByRole('button', { name: /^כללי/ });
    // Shift+F10 opens the same menu by the row.
    fireEvent.keyDown(row, { key: 'F10', shiftKey: true });
    expect(menuItems()).toEqual(['Rename', 'Copy channel ID', 'Move down']); // first in the list: no Move up
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowUp' });
    expect(document.activeElement?.textContent).toBe('Move down');
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    expect(document.activeElement?.textContent).toBe('Rename');
  });

  it('closes on a click outside', async () => {
    const { channels } = await setup();
    fireEvent.contextMenu(within(channels()).getByRole('button', { name: /^clips/ }));
    expect(screen.getByRole('menu')).toBeTruthy();
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('Delete from the menu goes straight to the confirmation', async () => {
    const { channels } = await setup();
    fireEvent.contextMenu(within(channels()).getByRole('button', { name: /^clips/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete channel' }));
    expect(screen.getByText('Delete ⁨#clips⁩?')).toBeTruthy();
    expect(screen.queryByLabelText('Channel name')).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Delete channel' }));
    });
    await waitFor(() => expect(within(channels()).queryByRole('button', { name: /^clips/ })).toBeNull());
  });

  it('copies the channel ID with a toast', async () => {
    const { channels } = await setup();
    let copied = '';
    Object.assign(window, { mvs: { copyText: async (t: string) => ((copied = t), true) } });
    try {
      fireEvent.contextMenu(within(channels()).getByRole('button', { name: /^clips/ }));
      await act(async () => {
        fireEvent.click(screen.getByRole('menuitem', { name: 'Copy channel ID' }));
      });
      expect(copied).toBe('c-clips');
      expect(screen.getAllByRole('status').some((s) => s.textContent === 'Copied')).toBe(true);
    } finally {
      delete (window as { mvs?: unknown }).mvs;
    }
  });

  it('channel, not the owner: no rename or delete', async () => {
    const { channels } = await setup();
    await openOtherServer(channels);
    fireEvent.contextMenu(within(channels()).getByRole('button', { name: /^builds/ }));
    expect(menuItems()).toEqual(['Mark as read', 'Copy channel ID']);
  });

  it('member: the owner can remove others; on myself only profile and code', async () => {
    await setup();
    const list = screen.getByRole('complementary', { name: 'Server members' });
    const rows = within(list).getAllByRole('button', { name: /profile/ });
    const me = rows.find((r) => r.textContent?.includes('(you)'))!;
    const other = rows.find((r) => !r.textContent?.includes('(you)') && !r.textContent?.includes('Owner'))!;

    fireEvent.contextMenu(me);
    expect(menuItems()).toEqual(['Profile', 'Copy identity code']);
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });

    fireEvent.contextMenu(other);
    expect(menuItems()).toEqual(['Profile', 'Copy identity code', 'Block', 'Remove from server']);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Remove from server' }));
    expect(screen.getByRole('alertdialog', { name: 'Remove from server' })).toBeTruthy();
  });

  it('server in the rail: copy invite and settings for the owner', async () => {
    await setup();
    const rail = screen.getByRole('navigation', { name: /servers/i });
    const owned = within(rail).getAllByRole('button').find((b) => b.getAttribute('aria-current') === 'page')!;
    fireEvent.contextMenu(owned);
    expect(menuItems()).toEqual(['Copy invite link', 'Server settings', 'Move down']); // first in the rail (G-01)
    fireEvent.click(screen.getByRole('menuitem', { name: 'Server settings' }));
    expect(screen.getByRole('tab', { name: 'General' })).toBeTruthy();
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });

    fireEvent.contextMenu(within(rail).getByRole('button', { name: /^Minecraft IL/ }));
    expect(menuItems()).toEqual(['Copy invite link', 'Leave server', 'Move up', 'Move down']);
  });
});

describe('invites page (#214)', () => {
  async function openInvites() {
    const { sidebar } = await setup();
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Server settings' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Invite link' }));
    const panel = screen.getByRole('tabpanel');
    await within(panel).findByText(/^3 uses/);
    return panel;
  }
  const currentLink = (panel: HTMLElement) => panel.querySelector('.invite-link')?.textContent;

  it('creates a link with an expiry and a use limit; it becomes the current link', async () => {
    const panel = await openInvites();
    const before = currentLink(panel);
    fireEvent.change(within(panel).getByLabelText('Expires after'), { target: { value: 'day' } });
    fireEvent.change(within(panel).getByLabelText('Max uses'), { target: { value: '5' } });
    await act(async () => {
      fireEvent.click(within(panel).getByRole('button', { name: 'Create link' }));
    });
    await within(panel).findByText(/0 of 5 uses/);
    expect(within(panel).getAllByText(/^Expires/).length).toBe(1);
    await waitFor(() => expect(currentLink(panel)).not.toBe(before));
  });

  it('revokes a link after a confirmation', async () => {
    const panel = await openInvites();
    fireEvent.click(within(panel).getByRole('button', { name: 'Revoke' }));
    const confirm = within(panel).getByRole('group', { name: 'Revoke this link? No one can join with it after this.' });
    await act(async () => {
      fireEvent.click(within(confirm).getByRole('button', { name: 'Revoke' }));
    });
    await within(panel).findByText('Revoked');
    await waitFor(() => expect(within(panel).getByText('No invite link is open. Create one below.')).toBeTruthy());
  });
});

describe('message menu (B25 §2)', () => {
  const rowOf = (text: string) => {
    const row = screen.getAllByRole('article').find((a) => a.textContent?.includes(text));
    if (!row) throw new Error(`no row with ${text}`);
    return row;
  };

  it('my message: reply, copy, edit and delete; copy takes the stored text', async () => {
    const { channels } = await setup();
    // An empty channel, so the new message is the only (rendered) row.
    fireEvent.click(within(channels()).getByRole('button', { name: 'Create channel' }));
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'menu-test' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    });
    const box = await screen.findByRole('textbox', { name: /menu-test/ });
    fireEvent.change(box, { target: { value: 'menu test שלום' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    });
    const row = await waitFor(() => rowOf('menu test שלום'));
    expect(row.getAttribute('aria-haspopup')).toBe('menu');
    fireEvent.contextMenu(row);
    expect(menuItems()).toEqual(['Reply', 'Copy text', 'Copy message ID', 'Edit', 'Delete']);

    let copied = '';
    Object.assign(window, { mvs: { copyText: async (t: string) => ((copied = t), true) } });
    try {
      await act(async () => {
        fireEvent.click(screen.getByRole('menuitem', { name: 'Copy text' }));
      });
      expect(copied).toBe('menu test שלום');
    } finally {
      delete (window as { mvs?: unknown }).mvs;
    }
  });

  it('someone else\'s message: the owner can delete it, others only reply and copy', async () => {
    const { channels } = await setup();
    const myName = screen.getByRole('complementary', { name: 'Channels' }).querySelector('.me-name')!.textContent!;
    const others = () => screen.getAllByRole('article').filter((a) => a.getAttribute('aria-haspopup') && !a.textContent?.includes(myName));
    // The rows render after the first fetch, and after a server switch the old
    // server's rows can linger for a moment: retry until this server's menu shows.
    await waitFor(() => {
      fireEvent.contextMenu(others()[0]);
      expect(menuItems()).toEqual(['Reply', 'Copy text', 'Copy message ID', 'Delete']);
    });
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });

    await openOtherServer(channels);
    await waitFor(() => {
      fireEvent.contextMenu(others()[0]);
      expect(menuItems()).toEqual(['Reply', 'Copy text', 'Copy message ID']);
    });
  });
});
