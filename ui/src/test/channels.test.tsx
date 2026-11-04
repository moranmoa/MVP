// B11c (spec docs/ux/specs/B11a-d-server-management.md §4): the owner creates,
// edits and deletes channels; #general can be renamed but not deleted.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup(core = new MockCore({ demo: true, speed: 0, simulateActivity: false })) {
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const channels = () => screen.getByRole('navigation', { name: 'Channels' });
  return { core, channels };
}

const submit = async (name: string) =>
  act(async () => {
    fireEvent.click(screen.getByRole('button', { name }));
  });

describe('channels (owner)', () => {
  it('creates a channel: spaces become dashes, and it opens', async () => {
    const { channels } = await setup();
    fireEvent.click(within(channels()).getByRole('button', { name: 'Create channel' }));
    const field = screen.getByLabelText('Channel name') as HTMLInputElement;
    fireEvent.change(field, { target: { value: 'חדשות  משחקים' } });
    expect(field.value).toBe('חדשות-משחקים');
    expect(document.getElementById('channel-name-preview')?.textContent).toBe('#חדשות-משחקים');
    await submit('Create');
    const created = await within(channels()).findByRole('button', { name: /^חדשות-משחקים/ });
    await waitFor(() => expect(created.getAttribute('aria-current')).toBe('page'));
  });

  it('says when the name is taken', async () => {
    const { channels } = await setup();
    fireEvent.click(within(channels()).getByRole('button', { name: 'Create channel' }));
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'clips' } });
    await submit('Create');
    expect(screen.getByRole('alert').textContent).toBe("There's already a channel with this name.");
  });

  it('says when the core refuses the name (B23)', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    core.createChannel = async () => {
      throw { code: 'bad-request' };
    };
    const { channels } = await setup(core);
    fireEvent.click(within(channels()).getByRole('button', { name: 'Create channel' }));
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'ok' } });
    await submit('Create');
    expect(screen.getByRole('alert').textContent).toBe("The name contains characters that can't be used in names.");
  });

  it('lets #general be renamed but not deleted', async () => {
    const { channels } = await setup();
    fireEvent.click(within(channels()).getByRole('button', { name: 'Edit channel: כללי' }));
    expect(screen.queryByRole('button', { name: 'Delete channel' })).toBeNull();
    expect(screen.getByText("The server's first channel can't be deleted, but you can rename it.")).toBeTruthy();
  });

  it('deletes the open channel: back to #general, with a notice', async () => {
    const { channels } = await setup();
    fireEvent.click(within(channels()).getByRole('button', { name: /^clips/ }));
    fireEvent.click(within(channels()).getByRole('button', { name: 'Edit channel: clips' }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete channel' }));
    expect(screen.getByText('Delete #⁨clips⁩?')).toBeTruthy();
    await submit('Delete channel');
    await waitFor(() => expect(within(channels()).queryByRole('button', { name: /^clips/ })).toBeNull());
    expect(screen.getByRole('status').textContent).toBe('#⁨clips⁩ was deleted.');
    expect(within(channels()).getByRole('button', { name: /^כללי/ }).getAttribute('aria-current')).toBe('page');
  });

  it('shows no channel controls to someone who is not the owner', async () => {
    const { channels } = await setup();
    fireEvent.click(screen.getByRole('button', { name: /^Minecraft IL/ }));
    await waitFor(() => expect(within(channels()).getByRole('button', { name: /^general/ })).toBeTruthy());
    expect(within(channels()).queryByRole('button', { name: 'Create channel' })).toBeNull();
    expect(within(channels()).queryByRole('button', { name: /^Edit channel/ })).toBeNull();
  });
});
