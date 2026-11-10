// Relay-only (GAP-6r, #272): the Settings → Privacy switch, "Join chat" off up
// front (from AppInfo and the voice room, not only after a refused join), and
// the sanitizer for the new `relayOnly` fields.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { cleanCall, cleanVoiceRoom } from '../core/sanitize';
import { MockCore, type MockOptions } from '../core/mock';

// jsdom has no WebRTC: an idle engine, as in voice.test.tsx.
vi.mock('../voice', () => ({
  useVoiceEngine: () => ({ ready: true, speaking: {}, peerFailed: {}, setMicMuted: () => {}, inputs: [], outputs: [], setInput: () => {}, setOutput: () => {} }),
}));

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  return { core };
}

const roomEntry = () => screen.getByRole('button', { name: /^Join chat/ });
const reason = 'Calls are off in relay-only mode: in a call, computers connect directly and participants can see your IP address.';

describe('relay-only', () => {
  it('"Join chat" is off before any join when the device is relay-only', async () => {
    await setup({ relayOnly: true });
    await waitFor(() => expect(roomEntry().getAttribute('aria-disabled')).toBe('true'));
    expect(screen.getByText(reason)).toBeTruthy();
  });

  it('the Privacy switch turns it on and off, and "Join chat" follows', async () => {
    await setup();
    await waitFor(() => expect(roomEntry().getAttribute('aria-disabled')).toBeNull());
    fireEvent.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    fireEvent.click(screen.getByRole('button', { name: 'Privacy' }));
    const toggle = screen.getByRole('switch', { name: 'Relay only' }) as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    expect(toggle.getAttribute('aria-describedby')).toBe('relay-only-hint relay-only-calls relay-only-restart');

    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(toggle.checked).toBe(true);
    expect(roomEntry().getAttribute('aria-disabled')).toBe('true');

    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(toggle.checked).toBe(false);
    expect(roomEntry().getAttribute('aria-disabled')).toBeNull();
  });

  it('"Change in Settings" under the reason opens Privacy with focus on the switch', async () => {
    await setup({ relayOnly: true });
    fireEvent.click(await screen.findByRole('button', { name: 'Change in Settings' }));
    const toggle = await screen.findByRole('switch', { name: 'Relay only' });
    await waitFor(() => expect(document.activeElement).toBe(toggle));
  });

  it('in a call, turning it on asks first; confirming ends the call', async () => {
    const { core } = await setup();
    const [server] = await core.listServers();
    const room = await core.getVoiceRoom(server.id);
    await act(async () => {
      await core.joinCall(room.roomId);
    });
    fireEvent.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    fireEvent.click(screen.getByRole('button', { name: 'Privacy' }));
    const toggle = screen.getByRole('switch', { name: 'Relay only' }) as HTMLInputElement;
    fireEvent.click(toggle);
    const ask = screen.getByRole('alertdialog', { name: 'Your call will end. Turn it on?' });
    expect(toggle.checked).toBe(false);
    fireEvent.click(within(ask).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('alertdialog')).toBeNull();
    fireEvent.click(toggle);
    await act(async () => {
      fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Turn on' }));
    });
    expect(toggle.checked).toBe(true);
    // The call ended: the room is back to "Join chat", now off.
    await waitFor(() => expect(roomEntry().getAttribute('aria-disabled')).toBe('true'));
  });

  it('the switch shows the stored value on open', async () => {
    await setup({ relayOnly: true });
    fireEvent.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    fireEvent.click(screen.getByRole('button', { name: 'Privacy' }));
    await waitFor(() => expect((screen.getByRole('switch', { name: 'Relay only' }) as HTMLInputElement).checked).toBe(true));
  });
});

describe('sanitizer', () => {
  const room = { serverId: 's-1', roomId: 's-1-voice', participants: [] };

  it('a voice room may carry relayOnly; anything but a boolean drops the room', () => {
    expect(cleanVoiceRoom({ ...room, relayOnly: true })).toEqual({ ...room, relayOnly: true });
    expect(cleanVoiceRoom({ ...room, relayOnly: false })).toEqual(room);
    expect(cleanVoiceRoom({ ...room, relayOnly: 'yes' })).toBeNull();
  });

  it('a call keeps relayOnly only when it is exactly true', () => {
    const call = { channelId: 's-1-voice', participants: [], full: false };
    expect(cleanCall({ ...call, relayOnly: true }).relayOnly).toBe(true);
    expect(cleanCall({ ...call, relayOnly: 'yes' as unknown as boolean }).relayOnly).toBeUndefined();
  });
});
