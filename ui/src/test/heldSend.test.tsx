// #288: a member who just joined has no channel key yet. A message sent then is
// held ("Sending…"), replaced by the real record when the key comes, or fails
// after the wait, with "Try again" (discard, then send the text again).

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MessageWindow } from '../app/store';
import type { CoreApi } from '../core/api';
import { MockCore } from '../core/mock';
import { cleanMessage, guardCore } from '../core/sanitize';
import type { Message } from '../core/types';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const HELD = 'pending-0123456789abcdef';

const msg = (index: number, over: Partial<Message> = {}): Message => ({
  id: `m${index}`,
  channelId: 'c',
  index,
  authorId: 'me',
  body: `message ${index}`,
  createdAt: 0,
  ...over,
});

/** jsdom draws only the first rows: a short #general, so a new message is on screen. */
function shortChannel(core: MockCore): MockCore {
  (core as unknown as { channelIndex: Map<string, { baseCount: number }> }).channelIndex.get('c-general')!.baseCount = 3;
  return core;
}

async function send(text: string) {
  const box = await screen.findByRole('textbox', { name: /^Message / });
  fireEvent.change(box, { target: { value: text } });
  await act(async () => {
    fireEvent.keyDown(box, { key: 'Enter' });
  });
}

describe('messages held for their key: sanitizer', () => {
  it('keeps a well-formed replaces and drops any other', () => {
    expect(cleanMessage(msg(1, { replaces: HELD })).replaces).toBe(HELD);
    for (const bad of ['pending-XYZ', 'pending-0123', `${HELD}0`, 'm7', 42 as unknown as string]) {
      expect('replaces' in cleanMessage(msg(1, { replaces: bad }))).toBe(false);
    }
  });

  it('a pending- id passes through message.added and message.updated', () => {
    const handlers = new Map<string, (p: unknown) => void>();
    const bare = { on: (e: string, h: (p: unknown) => void) => (handlers.set(e, h), () => {}) } as unknown as CoreApi;
    const seen: Message[] = [];
    const core = guardCore(bare);
    core.on('message.added', (m) => seen.push(m));
    core.on('message.updated', (m) => seen.push(m));
    handlers.get('message.added')!(msg(5, { id: HELD, sendState: 'pending' }));
    handlers.get('message.updated')!(msg(6, { id: 'real', sendState: 'sent', replaces: HELD }));
    expect(seen.map((m) => [m.id, m.sendState, m.replaces])).toEqual([
      [HELD, 'pending', undefined],
      ['real', 'sent', HELD],
    ]);
  });

  it('notice: rekey-waiting gets through, an unknown kind is dropped', () => {
    const handlers = new Map<string, (p: unknown) => void>();
    const bare = { on: (e: string, h: (p: unknown) => void) => (handlers.set(e, h), () => {}) } as unknown as CoreApi;
    const seen: unknown[] = [];
    guardCore(bare).on('notice', (n) => seen.push(n));
    handlers.get('notice')!({ kind: 'rekey-waiting', serverId: 's' });
    handlers.get('notice')!({ kind: 'surprise', serverId: 's' });
    handlers.get('notice')!({ kind: 'rekey-waiting', serverId: 'x'.repeat(500) });
    expect(seen).toEqual([
      { kind: 'rekey-waiting', serverId: 's' },
      { kind: 'rekey-waiting', serverId: 'x'.repeat(128) }, // through id()
    ]);
  });
});

describe('messages held for their key: the message window', () => {
  function windowWith(countAfter: number) {
    const core = { getMessageCount: async () => countAfter } as unknown as CoreApi;
    const w = new MessageWindow(core, 'c', () => {});
    return w;
  }

  it('the real message may land at another index: the held entry goes, nothing is shown twice', async () => {
    const w = windowWith(12);
    w.apply(msg(9));
    w.apply(msg(10, { id: HELD, sendState: 'pending' }));
    // The key made someone else's message readable first; it takes index 10.
    w.apply(msg(10, { id: 'theirs', authorId: 'them' }));
    w.apply(msg(11, { id: 'mine', sendState: 'sent', replaces: HELD }));
    await Promise.resolve();
    expect(w.get(9)?.id).toBe('m9');
    expect(w.get(10)?.id).toBe('theirs');
    expect(w.get(11)).toMatchObject({ id: 'mine', sendState: 'sent' });
    expect('replaces' in w.get(11)!).toBe(false);
    expect(w.count).toBe(12); // re-read from the core
  });

  it('a held entry that is still there is dropped, and what follows it is re-read', async () => {
    const w = windowWith(12);
    w.apply(msg(10, { id: HELD, sendState: 'pending' }));
    w.apply(msg(11, { id: 'pending-ffffffffffffffff', sendState: 'pending' }));
    w.apply(msg(10, { id: 'mine', sendState: 'sent', replaces: HELD }));
    await Promise.resolve();
    expect(w.get(10)?.id).toBe('mine');
    expect(w.get(11)).toBeUndefined(); // the other held one may have moved: loaded again
  });

  it('a discarded held message leaves; what followed is re-read and the count drops', async () => {
    const w = windowWith(11);
    w.apply(msg(9));
    w.apply(msg(10, { id: HELD, sendState: 'failed' }));
    w.apply(msg(11, { id: 'after' }));
    w.removeHeld(HELD);
    expect(w.get(9)?.id).toBe('m9');
    expect(w.get(10)).toBeUndefined();
    expect(w.get(11)).toBeUndefined();
    expect(w.count).toBe(11);
  });
});

describe('messages held for their key: in the chat', () => {
  it('shows "Sending…", then the real message replaces it', async () => {
    const core = shortChannel(new MockCore({ demo: true, speed: 0, simulateActivity: false, keyPending: 'arrive' }));
    let added: Message | undefined;
    core.on('message.added', (m) => (added = m));
    render(<App core={core} initialSettings={en} />);
    await send('hello before the key');
    expect(added?.id).toMatch(/^pending-[0-9a-f]{16}$/);
    // speed 0: the key comes right away; the held entry is gone, the real one is sent.
    await screen.findByText('hello before the key');
    expect(screen.queryByText('Sending…')).toBeNull();
    expect(document.querySelector('.send-state.held')).toBeNull();
    const last = (await core.getMessages('c-general', 3, 4)).messages[0];
    expect(last.id).not.toMatch(/^pending-/);
  });

  it('fails with no key: says why, offers Try again, and no toast on top of the composer', async () => {
    const core = shortChannel(new MockCore({ demo: true, speed: 0, simulateActivity: false, keyPending: 'fail' }));
    render(<App core={core} initialSettings={en} />);
    await send('no key yet');
    await screen.findByText("Not sent: you don't have access to this channel yet – this usually happens just after joining");
    // No toast: the line on the message says why (UX).
    expect(document.querySelector('.toast')).toBeNull();
    // No reply, edit or delete on a message that never reached the network.
    expect(document.querySelector('.msg.failed .msg-actions')).toBeNull();
    const before = await core.getMessageCount('c-general');
    const failedId = (await core.getMessages('c-general', before - 1, before)).messages[0].id;
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    });
    // Discarded, then sent as a new message: still one copy, not two.
    expect(await core.getMessageCount('c-general')).toBe(before);
    const listed = (await core.getMessages('c-general', 0, before)).messages;
    expect(listed.filter((m) => m.body === 'no key yet')).toHaveLength(1);
    expect(listed.some((m) => m.id === failedId)).toBe(false);
    // After a reload the old failed one doesn't come back.
    cleanup();
    render(<App core={core} initialSettings={en} />);
    await screen.findByRole('textbox', { name: /^Message / });
    expect((await screen.findAllByText('no key yet')).length).toBe(1);
  });

  it('discardMessage: not-found for anything but a held message', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    await expect(core.discardMessage('c-general#3')).rejects.toMatchObject({ code: 'not-found' });
  });
});
