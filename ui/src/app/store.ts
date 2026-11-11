// Client-side cache of what the core told us. Everything a screen needs to
// draw is read synchronously from here, so switching servers or channels
// never waits on the core (target: server switch < 100 ms). The core's events
// keep it fresh.

import { useSyncExternalStore } from 'react';
import type { CoreApi } from '../core/api';
import type { Channel, Id, Identity, Member, Message, ServerSummary, UpdateInfo, CallState, VoiceRoom } from '../core/types';
import { liveMutes, loadMutes, MUTE_DURATIONS, MUTE_FOREVER, nextExpiry, saveMutes, type MuteDuration, type MuteKey, type Mutes } from './mute';

const PAGE = 100;

/** A sparse window over one channel's messages, filled page by page. */
export class MessageWindow {
  count = 0;
  ready = false;
  private messages = new Map<number, Message>();
  private loadingPages = new Set<number>();
  private loadedPages = new Set<number>();

  constructor(
    private core: CoreApi,
    readonly channelId: Id,
    private notify: () => void,
  ) {}

  async init(): Promise<void> {
    this.count = await this.core.getMessageCount(this.channelId);
    this.ready = true;
    this.notify();
  }

  get(index: number): Message | undefined {
    return this.messages.get(index);
  }

  /** Makes sure [start, end) is loaded; cheap to call on every render. */
  ensure(start: number, end: number): void {
    const first = Math.floor(Math.max(0, start) / PAGE);
    const last = Math.floor(Math.max(0, Math.min(end, this.count) - 1) / PAGE);
    for (let p = first; p <= last; p++) {
      if (this.loadingPages.has(p) || this.loadedPages.has(p)) continue;
      this.loadingPages.add(p);
      void this.core.getMessages(this.channelId, p * PAGE, (p + 1) * PAGE).then((r) => {
        r.messages.forEach((m) => {
          // A live event may already have delivered a newer copy.
          if (!this.messages.has(m.index)) this.messages.set(m.index, m);
        });
        this.loadingPages.delete(p);
        this.loadedPages.add(p);
        this.notify();
      });
    }
  }

  apply(m: Message): void {
    if (m.replaces) return this.replace(m);
    this.messages.set(m.index, m);
    if (m.index >= this.count) this.count = m.index + 1;
    this.notify();
  }

  /**
   * A held message was sent (#288): drop the `pending-…` entry wherever it
   * sits, put the real one at its index, and re-read what may have moved. The
   * real index can differ, since the key may have brought other messages first.
   */
  private replace(m: Message): void {
    const { replaces, ...real } = m;
    let from = m.index;
    for (const [i, x] of this.messages) if (x.id === replaces) from = Math.min(from, i);
    this.forgetFrom(from, m.index);
    this.messages.set(m.index, real);
    if (m.index >= this.count) this.count = m.index + 1;
    this.notify();
    void this.recount();
  }

  /**
   * A held message was discarded (#288, discardMessage fires no event): it
   * leaves the list, and what came after it moves up one.
   */
  removeHeld(id: Id): void {
    const at = [...this.messages].find(([, x]) => x.id === id)?.[0];
    if (at === undefined) return void this.recount();
    this.forgetFrom(at);
    this.count = Math.max(0, this.count - 1);
    this.notify();
    void this.recount();
  }

  /** Drops what is cached from `from` on (but `keep`), to be loaded again. */
  private forgetFrom(from: number, keep?: number): void {
    for (const i of [...this.messages.keys()]) if (i >= from && i !== keep) this.messages.delete(i);
    for (const p of [...this.loadedPages]) if ((p + 1) * PAGE > from) this.loadedPages.delete(p);
  }

  private async recount(): Promise<void> {
    this.count = await this.core.getMessageCount(this.channelId);
    this.notify();
  }
}

export type UpdateNote =
  | { kind: 'installed'; version: string; notes: string | null }
  | { kind: 'rolledBack'; failed: string; running: string };

export interface StoreState {
  identity: Identity | null;
  /** A server left this device (left, closed and purged, or a kick): its name, for a toast. */
  serverGone: { name: string; reason: 'left' | 'closed' | 'kicked'; at: number } | null;
  /** Muted servers and channels on this device (G-03, G-17), by mute key; see app/mute.ts. */
  mutes: Mutes;
  servers: ServerSummary[];
  channels: Record<Id, Channel[]>;
  members: Record<Id, Member[]>;
  update: UpdateInfo | null;
  notice: { kind: 'encryption-renewed'; serverId: Id; at: number } | null;
  /** Shown once: a new version was installed, or the previous one is back. */
  updateNote: UpdateNote | null;
  /** My current call (voice v1), or null. */
  call: CallState | null;
  /** Why the last joinCall failed, until the next try or dismissal. */
  callError: string | null;
  /** Each server's voice room (who is in it), by server id. */
  voiceRooms: Record<Id, VoiceRoom>;
  /**
   * Relay-only is on (#272): calls are off. Read up front from AppInfo and every
   * voice room, and also learned from a call ended or a join refused for it.
   */
  relayOnly: boolean;
  /** Something asked for Settings at a section (e.g. "Change in Settings" under the voice room). */
  settingsRequest: { section: string; at: number } | null;
  /** A short confirmation toast (an i18n id), e.g. "Copied" after a menu copy. */
  flash: { id: string; at: number } | null;
  /** Bumped when any message window changes, so readers re-render. */
  messagesVersion: number;
}

export class Store {
  private state: StoreState = {
    identity: null,
    servers: [],
    channels: {},
    members: {},
    update: null,
    notice: null,
    updateNote: null,
    call: null,
    callError: null,
    voiceRooms: {},
    relayOnly: false,
    flash: null,
    settingsRequest: null,
    serverGone: null,
    mutes: loadMutes(),
    messagesVersion: 0,
  };
  private listeners = new Set<() => void>();
  private windows = new Map<Id, MessageWindow>();
  private unsubs: (() => void)[] = [];
  /** While my own leaveCall runs: that end of the call isn't news. */
  private leaving = false;

  /** Fires when the next timed mute ends, so muted marks go away on time. */
  private muteTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(readonly core: CoreApi) {
    this.scheduleMuteExpiry();
  }

  /** Subscribes to core events; returns the matching cleanup. */
  start(): () => void {
    const on = this.core.on.bind(this.core);
    this.unsubs.push(
      on('identity.changed', (identity) => this.set({ identity })),
      on('server.added', (s) => {
        this.set({ servers: [...this.state.servers.filter((x) => x.id !== s.id), s] });
        void this.loadServer(s.id);
      }),
      on('server.updated', (s) =>
        this.set({ servers: this.state.servers.map((x) => (x.id === s.id ? s : x)) }),
      ),
      on('server.removed', ({ serverId, reason }) => {
        const gone = this.state.servers.find((x) => x.id === serverId);
        this.set({
          servers: this.state.servers.filter((x) => x.id !== serverId),
          serverGone: gone ? { name: gone.name, reason: reason ?? 'kicked', at: Date.now() } : this.state.serverGone,
        });
      }),
      on('server.closed', ({ serverId }) =>
        this.set({ servers: this.state.servers.map((x) => (x.id === serverId ? { ...x, closed: true } : x)) }),
      ),
      on('channel.updated', (c) => {
        const list = this.state.channels[c.serverId] ?? [];
        const moved = list.find((x) => x.id === c.id)?.position !== c.position;
        this.set({ channels: { ...this.state.channels, [c.serverId]: list.map((x) => (x.id === c.id ? c : x)) } });
        // G-11: the order is the core's list order; a new position means re-read it.
        if (moved) this.reloadChannels(c.serverId);
      }),
      on('channel.added', (c) =>
        this.set({
          channels: {
            ...this.state.channels,
            [c.serverId]: [...(this.state.channels[c.serverId] ?? []).filter((x) => x.id !== c.id), c],
          },
        }),
      ),
      on('channel.removed', ({ serverId, channelId }) =>
        this.set({
          channels: { ...this.state.channels, [serverId]: (this.state.channels[serverId] ?? []).filter((x) => x.id !== channelId) },
        }),
      ),
      on('members.changed', ({ serverId, members }) =>
        this.set({ members: { ...this.state.members, [serverId]: members } }),
      ),
      on('message.added', (m) => this.windows.get(m.channelId)?.apply(m)),
      on('message.updated', (m) => this.windows.get(m.channelId)?.apply(m)),
      on('update.ready', (update) => this.set({ update })),
      on('update.installed', (u) => this.set({ updateNote: { kind: 'installed', ...u } })),
      on('call.state', (c) => {
        const ended = !c.participants.length && !!this.state.call && !this.leaving;
        const relayOnly = this.state.relayOnly || c.relayOnly === true;
        this.set({
          call: c.participants.length ? c : null,
          relayOnly,
          // The core ends my call when relay-only goes on (#232 V3.4): say why.
          ...(ended && relayOnly ? { callError: 'relay-only' } : {}),
        });
      }),
      on('voice.room', (r) => this.setRoom(r)),
      on('update.rolledBack', (u) => this.set({ updateNote: { kind: 'rolledBack', ...u } })),
      // rekey-waiting: the failed message's own line says why; no toast (UX, #289).
      on('notice', (n) => n.kind !== 'rekey-waiting' && this.set({ notice: { ...n, kind: n.kind, at: Date.now() } })),
    );
    return () => {
      this.unsubs.forEach((u) => u());
      this.unsubs = [];
    };
  }

  async init(): Promise<void> {
    const identity = await this.core.getIdentity();
    this.set({ identity });
    // Relay-only is per device and needs no identity (#272).
    const info = await this.core.getAppInfo().catch(() => null);
    if (info) this.set({ relayOnly: info.relayOnly === true });
    if (identity) await this.loadServers();
  }

  async loadServers(): Promise<void> {
    const servers = await this.core.listServers();
    this.set({ servers });
    // Prefetch every server so the first switch is already instant.
    await Promise.all(servers.map((s) => this.loadServer(s.id)));
  }

  private async loadServer(id: Id): Promise<void> {
    const [channels, members] = await Promise.all([this.core.listChannels(id), this.core.listMembers(id)]);
    this.set({
      channels: { ...this.state.channels, [id]: channels },
      members: { ...this.state.members, [id]: members },
    });
    await this.loadVoiceRoom(id);
  }

  /** Re-reads a server's voice room (events are hints): on start and on server switch. */
  async loadVoiceRoom(serverId: Id): Promise<void> {
    const room = await this.core.getVoiceRoom(serverId).catch(() => null);
    if (room) this.setRoom(room);
  }

  /** Every room carries the device's relay-only flag (#272), so "Join" is off before trying. */
  private setRoom(room: VoiceRoom): void {
    this.set({ voiceRooms: { ...this.state.voiceRooms, [room.serverId]: room }, relayOnly: room.relayOnly === true });
  }

  private reloadTimers = new Map<Id, ReturnType<typeof setTimeout>>();
  /** Re-reads a server's channel list (coalesced: one move updates several channels). */
  reloadChannels(serverId: Id): void {
    clearTimeout(this.reloadTimers.get(serverId));
    this.reloadTimers.set(
      serverId,
      setTimeout(() => {
        this.reloadTimers.delete(serverId);
        void this.core
          .listChannels(serverId)
          .then((channels) => this.set({ channels: { ...this.state.channels, [serverId]: channels } }))
          .catch(() => {});
      }, 0),
    );
  }

  /** G-11: sets the order the core answered a move with. */
  setChannels(serverId: Id, channels: Channel[]): void {
    this.set({ channels: { ...this.state.channels, [serverId]: channels } });
  }

  window(channelId: Id): MessageWindow {
    let w = this.windows.get(channelId);
    if (!w) {
      w = new MessageWindow(this.core, channelId, () => this.set({ messagesVersion: this.state.messagesVersion + 1 }));
      this.windows.set(channelId, w);
      void w.init();
    }
    return w;
  }

  /** #304: this computer forgot the identity. Back to "create or restore". */
  signedOut(): void {
    this.windows.clear();
    this.set({ identity: null, servers: [], channels: {}, members: {}, call: null, callError: null, voiceRooms: {}, notice: null });
  }

  setIdentity(identity: Identity | null): void {
    this.set({ identity });
  }

  clearNotice(): void {
    this.set({ notice: null });
  }

  showFlash(id: string): void {
    this.set({ flash: { id, at: Date.now() } });
  }

  openSettings(section: string): void {
    this.set({ settingsRequest: { section, at: Date.now() } });
  }

  clearFlash(): void {
    this.set({ flash: null });
  }

  /** Mutes a server or channel (a key from app/mute.ts), on this device only. */
  mute(key: MuteKey, duration: MuteDuration): void {
    const ms = MUTE_DURATIONS[duration];
    this.setMutes({ ...liveMutes(this.state.mutes), [key]: ms === MUTE_FOREVER ? MUTE_FOREVER : Date.now() + ms });
  }

  unmute(key: MuteKey): void {
    const { [key]: _gone, ...rest } = this.state.mutes;
    this.setMutes(liveMutes(rest));
  }

  private setMutes(mutes: Mutes): void {
    saveMutes(mutes);
    this.set({ mutes });
    this.scheduleMuteExpiry();
  }

  private scheduleMuteExpiry(): void {
    if (this.muteTimer) clearTimeout(this.muteTimer);
    this.muteTimer = null;
    const next = nextExpiry(this.state.mutes);
    if (next === null) return;
    // setTimeout caps at about 24.8 days; a 24-hour mute is well inside.
    this.muteTimer = setTimeout(() => this.setMutes(liveMutes(this.state.mutes)), Math.max(0, next - Date.now()) + 50);
  }

  clearServerGone(): void {
    this.set({ serverGone: null });
  }

  clearUpdateNote(): void {
    this.set({ updateNote: null });
  }

  /**
   * restartToUpdate was refused. In a call (not-allowed) the update waits for
   * the call to end, and the core sends update.ready again then. With
   * nothing staged there is nothing to restart into.
   */
  async joinCall(channelId: Id): Promise<void> {
    this.set({ callError: null });
    try {
      const call = await this.core.joinCall(channelId);
      this.set({ call: call.participants.length ? call : null });
    } catch (e) {
      const code = (e as { code?: string })?.code ?? 'internal';
      this.set({ callError: code, ...(code === 'relay-only' ? { relayOnly: true } : {}) });
    }
  }

  async leaveCall(): Promise<void> {
    this.leaving = true;
    try {
      await this.core.leaveCall().catch(() => null);
    } finally {
      this.leaving = false;
    }
    this.set({ call: null });
  }

  /** Settings → Privacy (#272). The core ends a call in progress when it goes on. */
  async setRelayOnly(on: boolean): Promise<void> {
    await this.core.setRelayOnly(on);
    this.set({ relayOnly: on, ...(on ? {} : { callError: this.state.callError === 'relay-only' ? null : this.state.callError }) });
  }

  clearCallError(): void {
    this.set({ callError: null });
  }

  updateRefused(code: string): void {
    if (code === 'not-allowed' && this.state.update) this.set({ update: { ...this.state.update, waitingForCall: true } });
    else if (code === 'nothing-staged') this.set({ update: null });
  }

  getState = (): StoreState => this.state;

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  private set(patch: Partial<StoreState>): void {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }
}

export function useStore<T>(store: Store, select: (s: StoreState) => T): T {
  return useSyncExternalStore(store.subscribe, () => select(store.getState()));
}
