// Voice v1: piece E (#223) and E2, the voice room amendment. Every server has
// one voice room in the channel sidebar ("Join chat"); everyone sees who is in
// it, can join, mute and leave; text channels don't carry calls.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import { Store } from '../app/store';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';
import { cleanCall, cleanVoiceRoom, guardCore } from '../core/sanitize';

// jsdom has no WebRTC: the engine (piece D) has its own tests in src/voice. Here it
// is an idle engine with a working mic; ?voiceSim drives the failure states.
const fakeEngine = vi.hoisted(() => ({ peerFailed: {} as Record<string, 'failed' | 'refused'>, stunUrls: [] as unknown[] }));
vi.mock('../voice', () => ({
  useVoiceEngine: (_call: unknown, _me: unknown, options?: { stunUrl?: string | null }) => (fakeEngine.stunUrls.push(options?.stunUrl), {
    ready: true,
    speaking: {},
    peerFailed: fakeEngine.peerFailed,
    setMicMuted: () => {},
    inputs: [],
    outputs: [],
    setInput: () => {},
    setOutput: () => {},
  }),
}));

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

/** `ipNoticeSeen: false` starts as on a computer that never joined a call. */
async function setup(options: MockOptions = {}, { ipNoticeSeen = true } = {}) {
  if (ipNoticeSeen) localStorage.setItem('mvs.notices.voiceIp.v1', '1');
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false, ...options });
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const ENTRY = /^(Join chat|You're in the chat|The chat is full)/;
  const entry = () => screen.getByRole('button', { name: ENTRY });
  await screen.findByRole('button', { name: ENTRY });
  const join = async () =>
    act(async () => {
      fireEvent.click(entry());
    });
  return { core, entry, join };
}

describe('STUN for the voice engine (#222)', () => {
  it('passes the shell STUN URL from core.status to the engine', async () => {
    fakeEngine.stunUrls = [];
    await setup({ stunUrl: 'stun:stun.example.test:3478' });
    await waitFor(() => expect(fakeEngine.stunUrls.at(-1)).toBe('stun:stun.example.test:3478'));
  });

  it.each([undefined, 'turn:turn.example.test:3478', 'stun:user:pass@stun.example.test', 'stun:stun.example.test/x'])(
    'passes null without one or for anything but a plain stun:/stuns: URL (%s)',
    async (stunUrl) => {
      fakeEngine.stunUrls = [];
      await setup(stunUrl === undefined ? {} : { stunUrl });
      // Let the shell's core.status (replayed on a timer) arrive.
      await act(async () => {
        await new Promise((r) => setTimeout(r, 10));
      });
      expect(fakeEngine.stunUrls.length).toBeGreaterThan(0);
      expect(fakeEngine.stunUrls.every((u) => u === null)).toBe(true);
    },
  );
});

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

  it('joins from the sidebar entry: call bar, me in the roster, names shown once', async () => {
    const { entry, join } = await setup();
    await join();
    const bar = screen.getByRole('region', { name: 'Voice call' });
    expect(bar.textContent).toContain('In the voice chat');
    expect(within(bar).queryAllByRole('listitem')).toHaveLength(0); // names only in the roster
    expect(entry().getAttribute('aria-label')).toBe("You're in the chat, 3 of 5 inside");
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    expect(within(screen.getByRole('list', { name: 'In the voice chat' })).getAllByRole('listitem')).toHaveLength(3);
  });

  it('mutes and unmutes me, and leaves', async () => {
    const { join } = await setup();
    await join();
    const bar = screen.getByRole('region', { name: 'Voice call' });
    const roster = screen.getByRole('list', { name: 'In the voice chat' });
    const before = within(roster).queryAllByRole('img', { name: 'Microphone off' }).length;
    const mute = within(bar).getByRole('button', { name: 'Mute' });
    expect(mute.getAttribute('aria-pressed')).toBe('false');
    await act(async () => {
      fireEvent.click(mute);
    });
    expect(within(bar).getByRole('button', { name: 'Mute' }).getAttribute('aria-pressed')).toBe('true');
    // The roster everyone sees follows the mute.
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
    expect(entry().getAttribute('aria-label')).toBe('The chat is full, 5 of 5 inside');
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
    ['in-call-elsewhere', "You're already in this call on another computer. Leave it there to join from here."],
  ] as const)('says why a join failed: %s', async (code, text) => {
    const { join } = await setup({ callError: code });
    await join();
    expect(document.querySelector('.call-blocked')?.textContent).toContain(text);
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('a join refused with a code the UI does not know shows the generic reason, never the code', async () => {
    const { join } = await setup({ callError: 'some-future-code' as unknown as 'call-full' });
    await join();
    const line = document.querySelector('.call-blocked')?.textContent ?? '';
    expect(line).toContain("Couldn't join the call. You can try again.");
    expect(line).not.toContain('some-future-code');
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
      expect(mute.getAttribute('title')).toMatch(/^No microphone/);
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
      const roster = screen.getByRole('list', { name: 'In the voice chat' });
      const failed = within(roster).getAllByText('No voice connection');
      expect(failed).toHaveLength(1);
      expect(failed[0].getAttribute('title')).toBe("You can't hear each other.");
      expect(within(roster).getAllByRole('listitem')).toHaveLength(3);
    } finally {
      window.history.pushState({}, '', '/');
    }
  });

  it('a connection refused by a security check says so, with its own hint', async () => {
    window.history.pushState({}, '', '/?voiceSim=peerRefused');
    try {
      const { join } = await setup();
      await join();
      const roster = screen.getByRole('list', { name: 'In the voice chat' });
      const refused = within(roster).getAllByText('Connection blocked for security');
      expect(refused).toHaveLength(1);
      expect(refused[0].getAttribute('title')).toMatch(/security check/);
      expect(within(roster).queryByText('No voice connection')).toBeNull();
    } finally {
      window.history.pushState({}, '', '/');
    }
  });

  it('shows the per-person state the engine reports (#245), by reason', async () => {
    const { core, join } = await setup();
    const [server] = await core.listServers();
    const others = (await core.getVoiceRoom(server.id)).participants.map((p) => p.userId);
    fakeEngine.peerFailed = { [others[0]]: 'refused', [others[1]]: 'failed' };
    try {
      await join();
      const roster = screen.getByRole('list', { name: 'In the voice chat' });
      expect(within(roster).getAllByText('Connection blocked for security')).toHaveLength(1);
      expect(within(roster).getAllByText('No voice connection')).toHaveLength(1);
    } finally {
      fakeEngine.peerFailed = {};
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
    // The core ends my call when relay-only goes on, and says why (#272).
    await store.setRelayOnly(true);
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

  it('cleanVoiceRoom and cleanCall read relayOnly (#272): only a boolean, passed on only when true', () => {
    const id = (c: string) => c.repeat(64);
    const room = { serverId: id('a'), roomId: id('b'), participants: [] };
    // #272 adds the field to every room: it must not get the room dropped.
    expect(cleanVoiceRoom({ ...room, relayOnly: true })).toEqual({ ...room, relayOnly: true });
    expect(cleanVoiceRoom({ ...room, relayOnly: false })).toEqual(room);
    expect(cleanVoiceRoom({ ...room, relayOnly: 'yes' })).toBeNull();
    const call = { channelId: id('b'), participants: [], full: false };
    expect(cleanCall({ ...call, relayOnly: true }).relayOnly).toBe(true);
    expect(cleanCall({ ...call, relayOnly: 'yes' as unknown as boolean }).relayOnly).toBeUndefined();
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
