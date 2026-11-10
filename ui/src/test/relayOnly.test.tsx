// Relay-only (GAP-6r, #272): the Settings → Privacy switch, "Join chat" off up
// front (from AppInfo and the voice room, not only after a refused join), and
// the sanitizer for the new `relayOnly` fields.

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { cleanCall, cleanVoiceRoom } from '../core/sanitize';
import { MockCore, type MockOptions } from '../core/mock';

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
    expect(toggle.getAttribute('aria-describedby')).toBe('relay-only-hint relay-only-calls');

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
