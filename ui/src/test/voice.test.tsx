// Voice v1: piece E (#223), and voice rooms (0.4.3, docs/ux/specs/voice-rooms.md):
// the pinned "Join chat" row opens a new room; the live rooms sit under it, with
// who is in each and the host crowned; text channels don't carry calls.

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import { Store } from '../app/store';
import type { Settings } from '../app/settings';
import { MockCore, type MockOptions } from '../core/mock';
import { cleanCall, cleanVoiceRoom, cleanVoiceRooms, guardCore } from '../core/sanitize';

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
  await screen.findByRole('button', { name: 'Join chat' });
  // The first room's row, once the rooms are read.
  if (options.voiceRooms !== 0) await waitFor(() => expect(rows().length).toBeGreaterThan(0));
  const entry = () => rows()[0];
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

/** Text without the bidi isolates the messages wrap values in. */
const plain = (s: string | null | undefined) => (s ?? '').replace(/[⁨⁩]/g, '');
/** A room row: its label is "<name>, n of max[, state]". */
const ROW = (n: string) => /, \d of \d/.test(plain(n));
const rows = () => screen.queryAllByRole('button', { name: ROW });
const label = (el: Element) => plain(el.getAttribute('aria-label'));
const root = () => screen.getByRole('button', { name: 'Join chat' });
const rooms = async (core: MockCore) => core.listVoiceRooms((await core.listServers())[0].id);
const click = async (el: Element) =>
  act(async () => {
    fireEvent.click(el);
  });

describe('voice rooms (0.4.3)', () => {
  it('shows the rooms before joining: name, count, who is in it, the host crowned', async () => {
    const { entry } = await setup();
    expect(label(entry())).toMatch(/^.+'s room, 2 of 5$/);
    expect(entry().getAttribute('aria-disabled')).toBeNull();
    // The pinned row is there with its hint, since a room exists.
    expect(document.getElementById(root().getAttribute('aria-describedby')!)?.textContent).toBe('Opens a new room');
    const roster = await screen.findByRole('list', { name: 'In the voice chat' });
    expect(within(roster).getAllByRole('listitem')).toHaveLength(2);
    expect(within(roster).getAllByRole('img', { name: 'Microphone off' })).toHaveLength(1);
    expect(within(roster).getAllByRole('img', { name: 'Room host' })).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Join call' })).toBeNull();
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('no rooms: only the pinned row, without the hint; a click opens my room with me as host', async () => {
    const { core } = await setup({ voiceRooms: 0 });
    expect(rows()).toHaveLength(0);
    expect(root().getAttribute('aria-describedby')).toBeNull();
    await click(root());
    const [mine] = await rooms(core);
    const me = (await core.getIdentity())!;
    expect(mine.masterId).toBe(me.id);
    expect(rows()).toHaveLength(1);
    expect(label(rows()[0])).toBe(`${me.displayName}'s room, 1 of 5, You're in the chat`);
    expect(within(screen.getByRole('list', { name: 'In the voice chat' })).getAllByRole('img', { name: 'Room host' })).toHaveLength(1);
    expect(screen.getByRole('region', { name: 'Voice call' })).toBeTruthy();
    expect(root().getAttribute('aria-describedby')).toBeTruthy();
  });

  it('the pinned row always opens a new room, leaving the one I was in (one room at a time)', async () => {
    const { core, join } = await setup();
    await join();
    expect((await rooms(core))[0].members).toHaveLength(3);
    await click(root());
    const list = await rooms(core);
    expect(list).toHaveLength(2);
    expect(list[0].members).toHaveLength(2); // I left the first
    expect(list[1].members).toHaveLength(1);
    // And clicking the first room again moves me back: my room empties and goes.
    await click(rows()[0]);
    expect(await rooms(core)).toHaveLength(1);
    expect((await rooms(core))[0].members).toHaveLength(3);
  });

  describe('the one-time IP notice (docs/ux/specs/voice-ip-notice.md)', () => {
    it('shows before the first join; joinCall waits for "Got it", which is remembered', async () => {
      const { core, join } = await setup({}, { ipNoticeSeen: false });
      const inRoom = async () => (await rooms(core))[0].members.length;
      await join();
      const dialog = screen.getByRole('dialog', { name: 'Before your first call' });
      expect(document.getElementById(dialog.getAttribute('aria-describedby')!)?.textContent).toContain('can see your IP address');
      expect(document.activeElement?.textContent).toBe('Got it');
      expect(await inRoom()).toBe(2); // joinCall not called yet
      await click(within(dialog).getByRole('button', { name: 'Got it' }));
      expect(await inRoom()).toBe(3);
      expect(localStorage.getItem('mvs.notices.voiceIp.v1')).toBe('1');
      const bar = screen.getByRole('region', { name: 'Voice call' });
      await waitFor(() => expect(document.activeElement).toBe(within(bar).getByRole('button', { name: 'Mute' })));
      // Leave and open a room: no notice the second time.
      await click(within(bar).getByRole('button', { name: 'Leave call' }));
      await click(root());
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(await rooms(core)).toHaveLength(2);
    });

    it('also before the first create', async () => {
      const { core } = await setup({ voiceRooms: 0 }, { ipNoticeSeen: false });
      await click(root());
      await click(within(screen.getByRole('dialog', { name: 'Before your first call' })).getByRole('button', { name: 'Got it' }));
      expect(await rooms(core)).toHaveLength(1);
    });

    it('Cancel: no join, nothing saved, focus back on the row, and the notice comes back next time', async () => {
      const { join, entry } = await setup({}, { ipNoticeSeen: false });
      await join();
      await click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
      expect(localStorage.getItem('mvs.notices.voiceIp.v1')).toBeNull();
      expect(document.activeElement).toBe(entry());
      await join();
      expect(screen.getByRole('dialog', { name: 'Before your first call' })).toBeTruthy();
    });

    it('a full room shows no notice: the row is disabled', async () => {
      const { join } = await setup({ roomFull: true }, { ipNoticeSeen: false });
      await join();
      expect(screen.queryByRole('dialog')).toBeNull();
    });
  });

  it('joins from a room row: call bar, me in the roster, names shown once', async () => {
    const { entry, join } = await setup();
    await join();
    const bar = screen.getByRole('region', { name: 'Voice call' });
    expect(bar.textContent).toContain('In the voice chat');
    expect(within(bar).queryAllByRole('listitem')).toHaveLength(0); // names only in the roster
    expect(label(entry())).toMatch(/'s room, 3 of 5, You're in the chat$/);
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    expect(entry().className).toContain('in-room');
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
    await click(mute);
    expect(within(bar).getByRole('button', { name: 'Mute' }).getAttribute('aria-pressed')).toBe('true');
    // The roster everyone sees follows the mute.
    expect(within(roster).getAllByRole('img', { name: 'Microphone off' })).toHaveLength(before + 1);
    await click(within(bar).getByRole('button', { name: 'Leave call' }));
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
    expect(label(rows()[0])).toMatch(/'s room, 2 of 5$/);
  });

  it('a full room is aria-disabled, says so in its name and tooltip, and a click does nothing', async () => {
    const { entry, join } = await setup({ roomFull: true });
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    expect(label(entry())).toMatch(/'s room, 5 of 5, The chat is full$/);
    expect(entry().getAttribute('title')).toBe('The chat is full');
    expect(entry().querySelector('.voice-room-count.full')).toBeTruthy();
    await join();
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('relay-only: the refused join disables the pinned row and every room, and says why', async () => {
    const { entry, join } = await setup({ callError: 'relay-only' });
    await join();
    expect(root().getAttribute('aria-disabled')).toBe('true');
    expect(entry().getAttribute('aria-disabled')).toBe('true');
    const reason = document.getElementById(entry().getAttribute('aria-describedby')!);
    expect(reason?.textContent).toContain('Calls are off in relay-only mode: in a call, computers connect directly');
    expect(root().getAttribute('aria-describedby')).toContain(reason!.id);
    expect(screen.queryByRole('region', { name: 'Voice call' })).toBeNull();
  });

  it('10 rooms: the pinned row is off with the cap reason; 8 are listed and "2 more rooms" shows the rest', async () => {
    await setup({ voiceRooms: 10 });
    expect(root().getAttribute('aria-disabled')).toBe('true');
    const reason = root()
      .getAttribute('aria-describedby')!
      .split(' ')
      .map((id) => document.getElementById(id)?.textContent)
      .join(' | ');
    expect(reason).toContain('This server already has many rooms. Join one of them.');
    expect(rows()).toHaveLength(8);
    const more = screen.getByRole('button', { name: (n) => plain(n) === '2 more rooms' });
    await click(more);
    expect(rows()).toHaveLength(10);
    expect(screen.queryByRole('button', { name: (n) => plain(n) === '2 more rooms' })).toBeNull();
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

  it('a refused create (room-cap from the core) says so under the pinned row', async () => {
    await setup({ callError: 'room-cap' });
    await click(root());
    expect(document.querySelector('.call-blocked')?.textContent).toContain('This server already has many rooms.');
  });

  it('a join refused with a code the UI does not know shows the generic reason, never the code', async () => {
    const { join } = await setup({ callError: 'some-future-code' as unknown as 'call-full' });
    await join();
    const line = document.querySelector('.call-blocked')?.textContent ?? '';
    expect(line).toContain("Couldn't join the call. You can try again.");
    expect(line).not.toContain('some-future-code');
  });

  it('host hand-off: the next earliest gets the crown, the default name follows, and the room hears it', async () => {
    const { core, join } = await setup();
    await join();
    const [before] = await rooms(core);
    const [, second] = before.members;
    const secondName = (await core.listMembers(before.serverId)).find((m) => m.id === second.userId)!.displayName;
    act(() => core.simulateVoiceLeave(before.roomId, before.masterId));
    const [after] = await rooms(core);
    expect(after.masterId).toBe(second.userId);
    await waitFor(() => expect(label(rows()[0])).toBe(`${secondName}'s room, 2 of 5, You're in the chat`));
    const live = document.querySelector('.voice-rooms [aria-live="polite"]');
    expect(plain(live?.textContent)).toBe(`${secondName} is now the room host`);
  });

  it('hand-off keeps a name of its own', async () => {
    const { core, join } = await setup();
    await join();
    const [room] = await rooms(core);
    // I own the first demo server, so I may rename any room in it.
    await act(async () => {
      await core.editVoiceRoom(room.roomId, { name: 'Raid night' });
    });
    act(() => core.simulateVoiceLeave(room.roomId, room.masterId));
    await waitFor(() => expect(label(rows()[0])).toBe("Raid night, 2 of 5, You're in the chat"));
  });

  it('the last one out: the room disappears for everyone', async () => {
    const { core } = await setup();
    const [room] = await rooms(core);
    act(() => room.members.forEach((p) => core.simulateVoiceLeave(room.roomId, p.userId)));
    await waitFor(() => expect(rows()).toHaveLength(0));
    expect(await rooms(core)).toHaveLength(0);
  });

  it('the room menu: rename and limit only for the host and the server owner', async () => {
    const { core, entry } = await setup();
    // First demo server: I own it, so I see them on a room I'm not in.
    fireEvent.contextMenu(entry());
    let menu = screen.getByRole('menu');
    expect(within(menu).getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Join', 'Copy room name', 'Rename', 'Member limit']);
    fireEvent.keyDown(menu, { key: 'Escape' });
    // Another server, owned by someone else: no Rename, no Member limit (not in the DOM).
    const [, other] = await core.listServers();
    expect(other.ownerId).not.toBe((await core.getIdentity())!.id);
    const rail = [...document.querySelectorAll<HTMLButtonElement>('button.rail-button')].find((b) => (b.getAttribute('aria-label') ?? '').includes(other.name))!;
    await click(rail);
    await waitFor(() => expect(document.querySelector('.voice-room-row')?.id).toContain(other.id));
    fireEvent.contextMenu(rows()[0]);
    menu = screen.getByRole('menu');
    expect(within(menu).getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Join', 'Copy room name']);
  });

  it('the host renames inline: Enter saves, Esc cancels, empty restores the default', async () => {
    const { core } = await setup({ voiceRooms: 0 });
    await click(root());
    const me = (await core.getIdentity())!;
    const rename = async () => {
      fireEvent.contextMenu(rows()[0]);
      await click(within(screen.getByRole('menu')).getByRole('menuitem', { name: 'Rename' }));
      return screen.getByRole('textbox', { name: 'Rename' }) as HTMLInputElement;
    };
    let field = await rename();
    expect(document.activeElement).toBe(field);
    expect(field.maxLength).toBe(32);
    fireEvent.change(field, { target: { value: 'Late night' } });
    fireEvent.keyDown(field, { key: 'Escape' });
    expect(screen.queryByRole('textbox', { name: 'Rename' })).toBeNull();
    expect((await rooms(core))[0].name).toBeUndefined();
    field = await rename();
    fireEvent.change(field, { target: { value: '  Late night ' } });
    await act(async () => {
      fireEvent.keyDown(field, { key: 'Enter' });
    });
    expect((await rooms(core))[0].name).toBe('Late night');
    await waitFor(() => expect(label(rows()[0])).toBe("Late night, 1 of 5, You're in the chat"));
    field = await rename();
    expect(field.value).toBe('Late night');
    fireEvent.change(field, { target: { value: '' } });
    await act(async () => {
      fireEvent.keyDown(field, { key: 'Enter' });
    });
    await waitFor(() => expect(label(rows()[0])).toBe(`${me.displayName}'s room, 1 of 5, You're in the chat`));
  });

  it('the limit: 2–5, never below the people inside', async () => {
    const { core, join } = await setup();
    await join(); // 3 inside
    fireEvent.contextMenu(rows()[0]);
    await click(within(screen.getByRole('menu')).getByRole('menuitem', { name: 'Member limit' }));
    const dialog = screen.getByRole('dialog', { name: 'Member limit' });
    const option = (n: number) => within(dialog).getByRole('button', { name: (x) => plain(x) === `Up to ${n}` }) as HTMLButtonElement;
    expect(option(2).disabled).toBe(true);
    expect(option(3).disabled).toBe(false);
    expect(option(5).getAttribute('aria-pressed')).toBe('true');
    await click(option(3));
    expect((await rooms(core))[0].limit).toBe(3);
    await waitFor(() => expect(label(rows()[0])).toMatch(/3 of 3, You're in the chat$/));
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
      await click(mute); // can't unmute without a microphone
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
    const others = (await rooms(core))[0].members.map((p) => p.userId);
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
    const [room] = await rooms(core);
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
    const [room] = await rooms(core);
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

  describe('the voice rooms sanitizer (design §5)', () => {
    const id = (c: string) => c.repeat(64);
    const p = (d: string, muted = false, joinedAt = 1000) => ({ userId: id(d), muted, joinedAt });
    const ok = { serverId: id('a'), roomId: id('b'), creatorId: id('1'), createdAt: 1000, limit: 5, masterId: id('1'), members: [p('1', true), p('2', false, 2000)] };

    it('keeps a good room; a name is cleaned, and one that cleans to nothing is the default', () => {
      expect(cleanVoiceRoom(ok)).toEqual(ok);
      expect(cleanVoiceRoom({ ...ok, name: '  Raid‮ night ' })).toEqual({ ...ok, name: 'Raid night' });
      expect(cleanVoiceRoom({ ...ok, name: '‮‏' })).toEqual(ok);
      expect(cleanVoiceRoom({ ...ok, name: 'x'.repeat(40) })?.name).toHaveLength(32);
      // #272: relayOnly is passed on only when true, and never gets the room dropped.
      expect(cleanVoiceRoom({ ...ok, relayOnly: true })).toEqual({ ...ok, relayOnly: true });
      expect(cleanVoiceRoom({ ...ok, relayOnly: false })).toEqual(ok);
    });

    it('drops a room with a bad id, limit, roster or host, or an unknown field', () => {
      for (const bad of [
        null,
        'room',
        { ...ok, roomId: 'b‮'.repeat(3) }, // not an id
        { ...ok, serverId: id('A') }, // upper-case hex
        { ...ok, masterId: id('9') }, // host not in the room
        { ...ok, creatorId: '<img>' },
        { ...ok, createdAt: -1 },
        { ...ok, createdAt: '1000' },
        { ...ok, limit: 1 },
        { ...ok, limit: 6 },
        { ...ok, limit: 2.5 },
        { ...ok, limit: 2, members: [p('1'), p('2'), p('3')] }, // more than the limit
        { ...ok, members: [] }, // an empty room is gone
        { ...ok, members: ['1', '2', '3', '4', '5', '6'].map((d) => p(d)) }, // 6 people
        { ...ok, members: [p('1'), p('1')] }, // twice
        { ...ok, members: [{ ...p('1'), muted: 'no' }] },
        { ...ok, members: [{ ...p('1'), joinedAt: Number.NaN }] },
        { ...ok, members: [{ ...p('1'), addr: '10.0.0.1' }] }, // unknown field
        { ...ok, name: 7 },
        { ...ok, relayOnly: 'yes' },
        { ...ok, extra: 1 }, // unknown field
        { ...ok, members: 'x' },
      ]) {
        expect(cleanVoiceRoom(bad)).toBeNull();
      }
    });

    it('a list: at most 10 rooms, one server, each room once; a bad room alone is left out; oldest first', () => {
      const room = (d: string, createdAt = 1000) => ({ ...ok, roomId: id(d), createdAt });
      expect(cleanVoiceRooms(id('a'), [room('c', 3000), room('b', 2000)])?.map((r) => r.roomId)).toEqual([id('b'), id('c')]);
      expect(cleanVoiceRooms(id('a'), [room('b'), { ...room('c'), limit: 9 }])).toHaveLength(1);
      expect(cleanVoiceRooms(id('a'), 'x')).toBeNull();
      expect(cleanVoiceRooms('<x>', [])).toBeNull();
      expect(cleanVoiceRooms(id('a'), [room('b'), room('b')])).toBeNull();
      expect(cleanVoiceRooms(id('a'), [{ ...room('b'), serverId: id('f') }])).toBeNull();
      expect(cleanVoiceRooms(id('a'), '0123456789a'.split('').map((d) => room(d)))).toBeNull(); // 11
    });

    it('a malformed voice.rooms event is dropped; a good one passes', () => {
      const handlers = new Map<string, (p: unknown) => void>();
      const bare = { on: (e: string, h: (p: unknown) => void) => (handlers.set(e, h), () => {}) } as unknown as Parameters<typeof guardCore>[0];
      const seen: unknown[] = [];
      guardCore(bare).on('voice.rooms', (r) => seen.push(r));
      const send = handlers.get('voice.rooms')!;
      send({ serverId: id('a'), rooms: [ok] });
      send({ serverId: id('a'), rooms: [ok, ok] });
      send({ serverId: '<img>', rooms: [] });
      send(null);
      expect(seen).toEqual([{ serverId: id('a'), rooms: [ok] }]);
    });
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
