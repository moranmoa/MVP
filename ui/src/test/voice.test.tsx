// Voice v1: piece E (#223) and E2, the voice room amendment. Every server has
// one voice room in the channel sidebar ("Join chat"); everyone sees who is in
// it, can join, mute and leave; text channels don't carry calls.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import { Store } from '../app/store';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';
import { cleanCall, cleanVoiceRoom, guardCore } from '../core/sanitize';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

/** `ipNoticeSeen: false` starts as on a computer that never joined a call. */
async function setup(options: MockOptions = {}, { ipNoticeSeen = true } = {}) {
  if (ipNoticeSeen) localStorage.setItem('mvs.notices.voiceIp.v1', '1');
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const entry = () => screen.getByRole('button', { name: /^(Join chat|You're in the chat)/ });
  await screen.findByRole('button', { name: /^Join chat/ });
  const join = async () =>
    act(async () => {
      fireEvent.click(entry());
    });
  return { core, entry, join };
}

describe('voice room', () => {
  it('shows who is in the room before joining, and no Join in the channel header', async () => {
    const { entry } = await setup();
    expect(entry().getAttribute('aria-label')).toBe('Join chat, 2 of 5 inside');
    expect(entry().getAttribute('aria-disabled')).toBeNull();
    const roster = await screen.findByRole('list', { name: 'In the voice chat' });
    expect(within(roster).getAllByRole('listitem')).toHaveLength(2);
    expect(within(roster).getAllByRole('img', { name: 'Microphone off' })).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Join call' })).toBeNull();
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  describe('the one-time IP notice (docs/ux/specs/voice-ip-notice.md)', () => {
    it('shows before the first join; joinCall waits for "Got it", which is remembered', async () => {
      const { core, join } = await setup({}, { ipNoticeSeen: false });
      const [server] = await core.listServers();
      const inRoom = async () => (await core.getVoiceRoom(server.id)).participants.length;
      await join();
      const dialog = screen.getByRole('dialog', { name: 'Before your first call' });
      expect(document.getElementById(dialog.getAttribute('aria-describedby')!)?.textContent).toContain('can see your IP address');
      expect(document.activeElement?.textContent).toBe('Got it');
      expect(await inRoom()).toBe(2); // joinCall not called yet
      expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
      await act(async () => {
        fireEvent.click(within(dialog).getByRole('button', { name: 'Got it' }));
      });
      expect(await inRoom()).toBe(3);
      expect(localStorage.getItem('mvs.notices.voiceIp.v1')).toBe('1');
      const bar = screen.getByRole('region', { name: 'Voice call' });
      await waitFor(() => expect(document.activeElement).toBe(within(bar).getByRole('button', { name: 'Mute' })));
      // Leave and join again: no notice the second time.
      await act(async () => {
        fireEvent.click(within(bar).getByRole('button', { name: 'Leave call' }));
      });
      await join();
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(await inRoom()).toBe(3);
      expect(screen.getByRole('region', { name: 'Voice call' })).toBeTruthy();
    });

    it('Cancel: no join, nothing saved, and the notice comes back next time', async () => {
      const { join, entry } = await setup({}, { ipNoticeSeen: false });
      await join();
      await act(async () => {
        fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
      });
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
      expect(localStorage.getItem('mvs.notices.voiceIp.v1')).toBeNull();
      expect(document.activeElement).toBe(entry());
      await join();
      expect(screen.getByRole('dialog', { name: 'Before your first call' })).toBeTruthy();
    });

    it('a full room shows no notice: the entry is disabled', async () => {
      const { join } = await setup({ roomFull: true }, { ipNoticeSeen: false });
      await join();
      expect(screen.queryByRole('dialog')).toBeNull();
    });
  });

  it('joins from the sidebar entry: call bar, me in the roster', async () => {
    const { entry, join } = await setup();
    await join();
    const bar = screen.getByRole('region', { name: 'Voice call' });
    expect(bar.textContent).toContain('In the voice chat');
    expect(within(bar).getAllByRole('listitem')).toHaveLength(3); // me + the two already inside
    expect(entry().getAttribute('aria-label')).toBe("You're in the chat, 3 of 5 inside");
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    expect(within(screen.getByRole('list', { name: 'In the voice chat' })).getAllByRole('listitem')).toHaveLength(3);
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
    // The roster everyone sees follows the mute too.
    const roster = screen.getByRole('list', { name: 'In the voice chat' });
    expect(within(roster).getAllByRole('img', { name: 'Microphone off' })).toHaveLength(before + 1);
    await act(async () => {
      fireEvent.click(within(bar).getByRole('button', { name: 'Leave call' }));
    });
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Join chat, 2 of 5 inside' })).toBeTruthy();
  });

  it('a full room is disabled, with the reason, and a click does nothing', async () => {
    const { entry, join } = await setup({ roomFull: true });
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    const reason = document.getElementById(entry().getAttribute('aria-describedby')!);
    expect(reason?.textContent).toBe('The call is full: up to 5 people.');
    await join();
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('relay-only: the refused join disables the entry and says why', async () => {
    const { entry, join } = await setup({ callError: 'relay-only' });
    await join();
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    const reason = document.getElementById(entry().getAttribute('aria-describedby')!);
    expect(reason?.textContent).toContain('Calls are off in relay-only mode: in a call, computers connect directly');
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it.each([
    ['call-full', 'The call is full: up to 5 people.'],
    ['not-implemented', "Calls aren't available in this version yet."],
  ] as const)('says why a join failed: %s', async (code, text) => {
    const { join } = await setup({ callError: code });
    await join();
    expect(document.querySelector('.call-blocked')?.textContent).toContain(text);
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('no microphone: stays in the call to listen, muted for everyone, and says so', async () => {
    window.history.pushState({}, '', '/?voiceSim=noMic');
    try {
      const { join } = await setup();
      await join();
      const bar = screen.getByRole('region', { name: 'Voice call' });
      await within(bar).findByText(/^No microphone: you can listen/);
      const mute = within(bar).getByRole('button', { name: 'Mute' });
      await waitFor(() => expect(mute.getAttribute('aria-pressed')).toBe('true'));
      expect(mute.getAttribute('aria-disabled')).toBe('true');
      await act(async () => {
        fireEvent.click(mute); // can't unmute without a microphone
      });
      expect(within(bar).getByRole('button', { name: 'Mute' }).getAttribute('aria-pressed')).toBe('true');
      // The others see me muted in the room roster too.
      const roster = screen.getByRole('list', { name: 'In the voice chat' });
      expect(within(roster).getAllByRole('img', { name: 'Microphone off' })).toHaveLength(2);
    } finally {
      window.history.pushState({}, '', '/');
    }
  });

  it('a mic picked after joining without one: still listen-only, "leave and join again"', async () => {
    window.history.pushState({}, '', '/?voiceSim=rejoin');
    try {
      const { join } = await setup();
      await join();
      const bar = screen.getByRole('region', { name: 'Voice call' });
      expect(within(bar).getByRole('status').textContent).toBe('To talk, leave and join again.');
      await waitFor(() => expect(within(bar).getByRole('button', { name: 'Mute' }).getAttribute('aria-pressed')).toBe('true'));
    } finally {
      window.history.pushState({}, '', '/');
    }
  });

  it("someone the engine can't reach shows \"couldn't connect directly\" and stays in the call", async () => {
    window.history.pushState({}, '', '/?voiceSim=peerFailed');
    try {
      const { join } = await setup();
      await join();
      const bar = screen.getByRole('region', { name: 'Voice call' });
      expect(within(bar).getAllByText("can't be heard: no direct connection")).toHaveLength(1);
      expect(within(bar).getAllByRole('listitem')).toHaveLength(3);
    } finally {
      window.history.pushState({}, '', '/');
    }
  });

  it('a call the core ends while relay-only is on shows the relay-only reason', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    const store = new Store(core);
    store.start();
    await store.init();
    const [server] = await core.listServers();
    const room = await core.getVoiceRoom(server.id);
    await store.joinCall(room.roomId);
    expect(store.getState().call).not.toBeNull();
    // My own Leave isn't news.
    await store.leaveCall();
    expect(store.getState().callError).toBeNull();
    await store.joinCall(room.roomId);
    store.setRelayOnly(true);
    await core.leaveCall(); // as the core does when relay-only goes on
    expect(store.getState().call).toBeNull();
    expect(store.getState().callError).toBe('relay-only');
  });

  it('text channels no longer carry calls', async () => {
    const { core } = await setup();
    const [server] = await core.listServers();
    const [channel] = await core.listChannels(server.id);
    await expect(core.joinCall(channel.id)).rejects.toMatchObject({ code: 'not-allowed' });
    const room = await core.getVoiceRoom(server.id);
    await expect(core.joinCall(room.roomId)).resolves.toMatchObject({ channelId: room.roomId });
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

  it('cleanVoiceRoom drops a room with a bad id, too many people or unknown fields', () => {
    const id = (c: string) => c.repeat(64);
    const p = (d: string, muted = false) => ({ userId: id(d), muted });
    const ok = { serverId: id('a'), roomId: id('b'), participants: [p('1', true), p('2')] };
    expect(cleanVoiceRoom(ok)).toEqual(ok);
    expect(cleanVoiceRoom({ ...ok, participants: [] })).toEqual({ ...ok, participants: [] });
    for (const bad of [
      null,
      'room',
      { ...ok, roomId: 'b‮'.repeat(3) }, // not an id
      { ...ok, serverId: id('A') }, // upper-case hex
      { ...ok, participants: ['1', '2', '3', '4', '5', '6'].map((d) => p(d)) }, // 6 people
      { ...ok, participants: [p('1'), p('1')] }, // twice
      { ...ok, participants: [{ ...p('1'), muted: 'no' }] },
      { ...ok, participants: [{ ...p('1'), addr: '10.0.0.1' }] }, // unknown field
      { ...ok, extra: 1 }, // unknown field
      { ...ok, participants: 'x' },
    ]) {
      expect(cleanVoiceRoom(bad)).toBeNull();
    }
  });

  it('a malformed voice.room event is dropped; a good one updates the roster', async () => {
    const handlers = new Map<string, (p: unknown) => void>();
    const bare = { on: (e: string, h: (p: unknown) => void) => (handlers.set(e, h), () => {}) } as unknown as Parameters<typeof guardCore>[0];
    const seen: unknown[] = [];
    guardCore(bare).on('voice.room', (r) => seen.push(r));
    const send = handlers.get('voice.room')!;
    send({ serverId: 'a'.repeat(64), roomId: 'b'.repeat(64), participants: [{ userId: 'c'.repeat(64), muted: false }] });
    send({ serverId: 'a'.repeat(64), roomId: '<img>', participants: [] });
    expect(seen).toHaveLength(1);
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
