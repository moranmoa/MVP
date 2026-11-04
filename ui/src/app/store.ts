// Client-side cache of what the core told us. Everything a screen needs to
// draw is read synchronously from here, so switching servers or channels
// never waits on the core (target: server switch < 100 ms). The core's events
// keep it fresh.

import { useSyncExternalStore } from 'react';
import type { CoreApi } from '../core/api';
import type { Channel, Id, Identity, Member, Message, ServerSummary, UpdateInfo } from '../core/types';

const PAGE = 100;

/** A sparse window over one channel's messages, filled page by page. */
export class MessageWindow {
  count = 0;
  ready = false;
  private messages = new Map<number, Message>();
  private loadingPages = new Set<number>();

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
      if (this.loadingPages.has(p) || this.messages.has(p * PAGE)) continue;
      this.loadingPages.add(p);
      void this.core.getMessages(this.channelId, p * PAGE, (p + 1) * PAGE).then((r) => {
        r.messages.forEach((m) => {
          // A live event may already have delivered a newer copy.
          if (!this.messages.has(m.index)) this.messages.set(m.index, m);
        });
        this.loadingPages.delete(p);
        this.notify();
      });
    }
  }

  apply(m: Message): void {
    this.messages.set(m.index, m);
    if (m.index >= this.count) this.count = m.index + 1;
    this.notify();
  }
}

export interface StoreState {
  identity: Identity | null;
  servers: ServerSummary[];
  channels: Record<Id, Channel[]>;
  members: Record<Id, Member[]>;
  update: UpdateInfo | null;
  notice: { kind: 'encryption-renewed'; serverId: Id; at: number } | null;
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
    messagesVersion: 0,
  };
  private listeners = new Set<() => void>();
  private windows = new Map<Id, MessageWindow>();
  private unsubs: (() => void)[] = [];

  constructor(readonly core: CoreApi) {}

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
      on('server.removed', ({ serverId }) =>
        this.set({ servers: this.state.servers.filter((x) => x.id !== serverId) }),
      ),
      on('channel.updated', (c) =>
        this.set({
          channels: {
            ...this.state.channels,
            [c.serverId]: (this.state.channels[c.serverId] ?? []).map((x) => (x.id === c.id ? c : x)),
          },
        }),
      ),
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
      on('notice', (n) => this.set({ notice: { ...n, at: Date.now() } })),
    );
    return () => {
      this.unsubs.forEach((u) => u());
      this.unsubs = [];
    };
  }

  async init(): Promise<void> {
    const identity = await this.core.getIdentity();
    this.set({ identity });
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

  setIdentity(identity: Identity | null): void {
    this.set({ identity });
  }

  clearNotice(): void {
    this.set({ notice: null });
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
