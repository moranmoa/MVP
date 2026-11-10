// Voice v1, piece E (#223): join a call from a channel, see who is in it,
// mute, leave, and the reasons a join can fail.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';
import { cleanCall, guardCore } from '../core/sanitize';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup(options: MockOptions = {}) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const join = async () =>
    act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Join call' }));
    });
  return { core, join };
}

describe('calls', () => {
  it('joins from the channel header: call bar, speaker icon on the channel, no second Join', async () => {
    const { join } = await setup();
    await join();
    const bar = screen.getByRole('region', { name: 'Voice call' });
    expect(bar.textContent).toContain('In a call in ⁨#כללי⁩');
    expect(within(bar).getAllByRole('listitem')).toHaveLength(3); // me + two online members
    expect(screen.getByRole('img', { name: 'Call in progress here' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Join call' })).toBeNull();
  });

  it('mutes and unmutes me, and leaves', async () => {
    const { join } = await setup();
    await join();
    const bar = screen.getByRole('region', { name: 'Voice call' });
    const before = within(bar).queryAllByRole('img', { name: 'Microphone off' }).length;
    const mute = within(bar).getByRole('button', { name: 'Mute' });
    expect(mute.getAttribute('aria-pressed')).toBe('false');
    await act(async () => {
      fireEvent.click(mute);
    });
    expect(within(bar).getByRole('button', { name: 'Mute' }).getAttribute('aria-pressed')).toBe('true');
    expect(within(bar).getAllByRole('img', { name: 'Microphone off' })).toHaveLength(before + 1);
    await act(async () => {
      fireEvent.click(within(bar).getByRole('button', { name: 'Leave call' }));
    });
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
    expect(screen.queryByRole('img', { name: 'Call in progress here' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Join call' })).toBeTruthy();
  });

  it.each([
    ['relay-only', 'Calls are off in relay-only mode: in a call, computers connect directly'],
    ['call-full', 'The call is full: up to 5 people.'],
    ['not-implemented', "Calls aren't available in this version yet."],
  ] as const)('says why a join failed: %s', async (code, text) => {
    const { join } = await setup({ callError: code });
    await join();
    expect(document.querySelector('.call-blocked')?.textContent).toContain(text);
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('cleanCall keeps at most 5 people and drops anything malformed', () => {
    const id = (c: string) => c.repeat(64);
    const c = cleanCall({
      channelId: id('a'),
      participants: [
        { identity: id('1'), muted: true },
        { identity: '', muted: false },
        ...['2', '3', '4', '5', '6', '7'].map((d) => ({ identity: id(d), muted: 'yes' as unknown as boolean })),
      ],
      full: 'true' as unknown as boolean,
    });
    expect(c.participants).toHaveLength(4); // 5 kept, then the empty identity dropped
    expect(c.participants[0]).toEqual({ identity: id('1'), muted: true });
    expect(c.participants.every((p, i) => i === 0 || p.muted === false)).toBe(true);
    expect(c.full).toBe(false);
  });

  it('drops a malformed call.signal instead of passing on a cleaned-up guess', () => {
    const handlers = new Map<string, (p: unknown) => void>();
    const bare = { on: (e: string, h: (p: unknown) => void) => (handlers.set(e, h), () => {}) } as unknown as Parameters<typeof guardCore>[0];
    const seen: unknown[] = [];
    guardCore(bare).on('call.signal', (p) => seen.push(p));
    const send = handlers.get('call.signal')!;
    const from = 'b'.repeat(64);
    send({ from, kind: 'offer', payload: 'v=0', fingerprintVerified: true }); // fine
    send({ from, kind: 'ice', payload: 'candidate', fingerprintVerified: true }); // fine
    send({ from, kind: 'hello', payload: 'x', fingerprintVerified: true }); // unknown kind
    send({ from, kind: 'ice', payload: 'x'.repeat(12 * 1024 + 1), fingerprintVerified: true }); // too big
    send({ from: 'not-hex', kind: 'ice', payload: 'x', fingerprintVerified: true }); // bad sender
    send({ from, kind: 'answer', payload: 'v=0', fingerprintVerified: false }); // unverified
    expect(seen.map((s) => (s as { kind: string }).kind)).toEqual(['offer', 'ice']);
  });
});
