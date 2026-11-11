// G-61 (docs/ux/GAPS.md §2.4): desktop notifications through the shell's
// `window.mvs.notify`, levels per server and channel, mutes (app/mute.ts,
// G-03 / G-17), preview, grouping.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import { serverMuteKey } from '../app/mute';
import { cleanNotifyPrefs, createGrouper, defaultNotifyPrefs, GROUP_MS, shouldNotify, withLevel, type NotifyContext, type NotifyPrefs } from '../app/notify';
import type { Settings } from '../app/settings';
import type { CoreEvents } from '../core/api';
import { MockCore } from '../core/mock';
import type { Message } from '../core/types';

const NOW = 1_800_000_000_000;
const msg = (over: Partial<Message> = {}): Message => ({
  id: 'm-1',
  channelId: 'c-general',
  index: 1,
  authorId: 'u-noa',
  body: 'Anyone up tonight?',
  createdAt: NOW,
  ...over,
});
const ctx = (prefs: NotifyPrefs, over: Partial<NotifyContext> = {}): NotifyContext => ({
  prefs,
  serverId: 's-gamers',
  myId: 'u-me',
  authorBlocked: false,
  muted: false,
  watching: false,
  now: NOW,
  ...over,
});
const all = withLevel(defaultNotifyPrefs(), 'servers', 's-gamers', 'all');

describe('which messages notify', () => {
  it('"All messages" notifies; never for me, a blocked author, the channel I am watching, or old catch-up', () => {
    expect(shouldNotify(msg(), ctx(all))).toBe(true);
    expect(shouldNotify(msg({ authorId: 'u-me' }), ctx(all))).toBe(false);
    expect(shouldNotify(msg(), ctx(all, { authorBlocked: true }))).toBe(false);
    expect(shouldNotify(msg(), ctx(all, { watching: true }))).toBe(false);
    expect(shouldNotify(msg({ createdAt: NOW - 11 * 60_000 }), ctx(all))).toBe(false);
    expect(shouldNotify(msg(), ctx({ ...all, desktop: false }))).toBe(false);
  });

  it('"Only mentions" (the default) needs a mention of me', () => {
    const prefs = defaultNotifyPrefs();
    expect(shouldNotify(msg(), ctx(prefs))).toBe(false);
    expect(shouldNotify(msg({ mentionsMe: true }), ctx(prefs))).toBe(true);
  });

  it('a mute silences everything but a mention of me (unless the level is "Nothing")', () => {
    expect(shouldNotify(msg(), ctx(all, { muted: true }))).toBe(false);
    expect(shouldNotify(msg({ mentionsMe: true }), ctx(all, { muted: true }))).toBe(true);
    const nothing = withLevel(all, 'channels', 'c-general', 'none');
    expect(shouldNotify(msg({ mentionsMe: true }), ctx(nothing, { muted: true }))).toBe(false);
  });

  it("a channel's level overrides its server's", () => {
    const quiet = withLevel(all, 'channels', 'c-general', 'none');
    expect(shouldNotify(msg(), ctx(quiet))).toBe(false);
    expect(shouldNotify(msg({ channelId: 'c-clips' }), ctx(quiet))).toBe(true);
    // Back to "same as the server": the scope goes away.
    expect(withLevel(quiet, 'channels', 'c-general', null).channels).toEqual({});
  });

  it('stored prefs are checked field by field', () => {
    expect(cleanNotifyPrefs(undefined)).toEqual(defaultNotifyPrefs());
    expect(
      cleanNotifyPrefs({ desktop: 'yes', preview: false, servers: { a: { level: 'loud' }, b: { level: 'all' }, c: 5 }, channels: [] }),
    ).toEqual({ desktop: true, preview: false, servers: { b: { level: 'all' } }, channels: {} });
  });

  it('3 messages within 10 s: the first at once, then one "3 new messages"', () => {
    vi.useFakeTimers();
    try {
      const shown: string[] = [];
      const g = createGrouper<string>(
        (m) => shown.push(m),
        (m, n) => shown.push(`${n}:${m}`),
      );
      g.add('c', 'a');
      g.add('c', 'b');
      g.add('c', 'c');
      g.add('d', 'x');
      expect(shown).toEqual(['a', 'x']);
      vi.advanceTimersByTime(GROUP_MS);
      expect(shown).toEqual(['a', 'x', '3:c']);
      // Opening the channel drops what was waiting.
      g.add('c', 'e');
      g.add('c', 'f');
      g.drop('c');
      vi.advanceTimersByTime(GROUP_MS);
      expect(shown).toEqual(['a', 'x', '3:c', 'e']);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('desktop notifications in the app', () => {
  const notify = vi.fn(async () => true);
  beforeEach(() => {
    notify.mockClear();
    (window as { mvs?: unknown }).mvs = { notify };
    vi.spyOn(document, 'hasFocus').mockReturnValue(false);
  });
  afterEach(() => {
    delete (window as { mvs?: unknown }).mvs;
    vi.restoreAllMocks();
  });

  async function setup(notifyPrefs: NotifyPrefs) {
    const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15, notify: notifyPrefs };
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    render(<App core={core} initialSettings={settings} />);
    await screen.findByRole('textbox', { name: /^Message / });
    // Channels and members are prefetched for every server; wait for them.
    await screen.findByRole('button', { name: /^clips/ });
    const members = await core.listMembers('s-gamers');
    const author = members.find((m) => m.presence === 'online' && !m.blocked && !m.isOwner)!;
    const emit = <E extends keyof CoreEvents>(e: E, p: CoreEvents[E]) =>
      act(() => (core as unknown as { emit: (e: E, p: CoreEvents[E]) => void }).emit(e, p));
    return { author, emit };
  }

  it('sends the sender, channel and server, once; with the preview off, no text', async () => {
    const { author, emit } = await setup(all);
    emit('message.added', msg({ id: 'm-x', channelId: 'c-clips', authorId: author.id, createdAt: Date.now() }));
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith({
      title: `${author.displayName} · #clips · גיימרים ישראל`,
      body: 'Anyone up tonight?',
      channelId: 'c-clips',
      messageId: 'm-x',
    });
  });

  it('with the preview off the body is "New message"', async () => {
    const { author, emit } = await setup({ ...all, preview: false });
    emit('message.added', msg({ channelId: 'c-clips', authorId: author.id, createdAt: Date.now() }));
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ body: 'New message' }));
    expect(JSON.stringify(notify.mock.calls)).not.toContain('Anyone up tonight?');
  });

  it('a muted server (app/mute.ts) stays quiet; a mention of me still comes through', async () => {
    localStorage.setItem('mvs.mute.v1', JSON.stringify({ [serverMuteKey('s-gamers')]: 0 }));
    try {
      const { author, emit } = await setup(all);
      emit('message.added', msg({ id: 'm-a', channelId: 'c-clips', authorId: author.id, createdAt: Date.now() }));
      expect(notify).not.toHaveBeenCalled();
      emit('message.added', msg({ id: 'm-b', channelId: 'c-lfg', authorId: author.id, createdAt: Date.now(), mentionsMe: true }));
      expect(notify).toHaveBeenCalledTimes(1);
    } finally {
      localStorage.removeItem('mvs.mute.v1');
    }
  });

  it('nothing for the channel on screen while the window has focus', async () => {
    const { author, emit } = await setup(all);
    vi.mocked(document.hasFocus).mockReturnValue(true);
    emit('message.added', msg({ channelId: 'c-general', authorId: author.id, createdAt: Date.now() }));
    expect(notify).not.toHaveBeenCalled();
  });

  it('a click opens the channel', async () => {
    const { emit } = await setup(all);
    emit('notify.click', { channelId: 'c-clips', messageId: 'm-x' });
    expect(await screen.findByRole('textbox', { name: /^Message .*clips/ })).toBeTruthy();
  });

  it('Settings → Notifications has the two switches, saved locally', async () => {
    await setup(all);
    fireEvent.click(screen.getAllByRole('button', { name: 'Settings' })[0]);
    fireEvent.click(await screen.findByRole('button', { name: 'Notifications' }));
    const preview = screen.getByRole('switch', { name: 'Show message preview in notifications' });
    expect((preview as HTMLInputElement).checked).toBe(true);
    fireEvent.click(preview);
    expect((preview as HTMLInputElement).checked).toBe(false);
    fireEvent.click(screen.getByRole('switch', { name: 'Desktop notifications' }));
    expect((preview as HTMLInputElement).disabled).toBe(true);
  });

  it("a channel's menu opens its notification level", async () => {
    await setup(defaultNotifyPrefs());
    const channels = screen.getByRole('complementary', { name: /channels/i });
    fireEvent.contextMenu(within(channels).getByRole('button', { name: /^clips/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Notifications' }));
    const dialog = await screen.findByRole('dialog', { name: /Notifications for/ });
    expect((within(dialog).getByRole('radio', { name: 'Same as the server' }) as HTMLInputElement).checked).toBe(true);
    fireEvent.click(within(dialog).getByRole('radio', { name: 'Nothing' }));
    expect((within(dialog).getByRole('radio', { name: 'Nothing' }) as HTMLInputElement).checked).toBe(true);
  });
});
