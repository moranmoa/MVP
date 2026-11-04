// In-page stand-in for the Rust core. It behaves like the real thing from the
// UI's point of view (async commands, events, ranged message reads) so screens
// can be built and tested before the core exists.
//
// Demo hooks: a message containing "/fail" fails to send; an invite link
// containing "offline" finds nobody online; one containing "adult" is 18+.

import type { CoreApi, CoreEventName, CoreEvents, Unsubscribe } from './api';
import type {
  AgeRestriction,
  AppInfo,
  Channel,
  CoreError,
  Id,
  Identity,
  JoinStatus,
  Member,
  Message,
  MessageRange,
  NewIdentityInput,
  ProfileInput,
  RecoveryCheckResult,
  ServerSummary,
  StorageUsage,
  UpdateChannel,
} from './types';
import {
  AVATAR_COLORS,
  BODIES,
  DEMO_SERVERS,
  JOINABLE_SERVER,
  PEOPLE,
  WORDS,
  type ServerSeed,
} from './mockData';

export interface MockOptions {
  /** Start with an account and the demo servers (skips onboarding). */
  demo?: boolean;
  /** Demo account that has not checked its recovery words yet. */
  demoUnchecked?: boolean;
  /** Multiplier for every simulated delay; 0 makes tests instant. */
  speed?: number;
  /** Other members post now and then. */
  simulateActivity?: boolean;
  /** Announce a ready update after this many ms (undefined = never). */
  updateAfterMs?: number;
  /** Add members and messages written the way an attacker would (see SECURITY.md). */
  hostile?: boolean;
  /** `getRecoveryWords()` fails, to show the load error state. */
  wordsError?: boolean;
  /** Add the mixed-direction messages of the B17 spec (§2) to the first channel. */
  bidi?: boolean;
  /** Keep the account across reloads. */
  storage?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
}

/** Spaces become '-', runs of '-' collapse, at most 32 characters (B11c §4.1). */
export function normalizeChannelName(raw: string): string {
  return raw.trim().replace(/\s+/g, '-').replace(/-{2,}/g, '-').slice(0, 32);
}

const STORAGE_KEY = 'mvp.mock.v1';
/** Failed word checks before the challenge is burned (B19 spec §6). */
const MAX_CHECK_FAILURES = 5;
const MESSAGE_GAP_MS = 47_000;

interface ChannelState {
  channel: Channel;
  baseCount: number;
  baseStart: number;
  seedOffset: number;
  memberIds: Id[];
  /** Edited, deleted or locally written messages, by index. */
  overrides: Map<number, Message>;
  appendedCount: number;
}

interface ServerState {
  summary: ServerSummary;
  channels: ChannelState[];
  members: Member[];
}

/**
 * What the mock keeps in localStorage. Never the recovery words: the real core
 * keeps those in the OS keychain, and the mock rebuilds them from the account id.
 */
interface Persisted {
  identity: Identity;
  createdServers: { id: Id; name: string; ageRestriction: AgeRestriction; inviteLink: string }[];
  joinedDemo: boolean;
  demo: boolean;
}

/** Small deterministic hash so generated content is stable between runs. */
function hash(n: number): number {
  let x = n | 0;
  x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
  x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
  return (x ^ (x >>> 16)) >>> 0;
}

function hashString(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

const CODE_ALPHABET = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
export function shortCodeFor(id: string): string {
  let h = hashString(id);
  let out = '';
  for (let i = 0; i < 8; i++) {
    if (i === 4) out += '-';
    out += CODE_ALPHABET[h % CODE_ALPHABET.length];
    h = hash(h + i);
  }
  return out;
}

function initialsOf(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return [...parts[0]].slice(0, 2).join('');
  return [...parts[0]][0] + [...parts[1]][0];
}

function err(e: CoreError): CoreError {
  return e;
}

export class MockCore implements CoreApi {
  private listeners = new Map<CoreEventName, Set<(p: unknown) => void>>();
  private servers = new Map<Id, ServerState>();
  private channelIndex = new Map<Id, ChannelState>();
  private identity: Identity | null = null;
  private words: string[] = [];
  private challenge: { positions: number[]; failures: number; burned: boolean } | null = null;
  private persisted: Persisted | null = null;
  private storageLimit = 5 * 1024 ** 3;
  private releaseChannel: UpdateChannel = 'stable';
  private pendingJoins = new Map<string, ReturnType<typeof setTimeout>>();
  private timers: ReturnType<typeof setTimeout | typeof setInterval>[] = [];
  private speed: number;

  constructor(private options: MockOptions = {}) {
    this.speed = options.speed ?? 1;
    this.load();
    if (options.simulateActivity) {
      this.timers.push(setInterval(() => this.simulateIncoming(), 4500));
      this.timers.push(setInterval(() => this.simulatePresence(), 9000));
    }
    if (options.updateAfterMs !== undefined) {
      this.timers.push(
        setTimeout(() => this.emit('update.ready', { version: '0.2.0', waitingForCall: false }), options.updateAfterMs),
      );
    }
  }

  dispose(): void {
    this.timers.forEach((t) => clearTimeout(t as ReturnType<typeof setTimeout>));
    this.pendingJoins.forEach((t) => clearTimeout(t));
  }

  // ---- events --------------------------------------------------------------

  on<E extends CoreEventName>(event: E, handler: (payload: CoreEvents[E]) => void): Unsubscribe {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(handler as (p: unknown) => void);
    return () => set!.delete(handler as (p: unknown) => void);
  }

  private emit<E extends CoreEventName>(event: E, payload: CoreEvents[E]): void {
    this.listeners.get(event)?.forEach((h) => h(payload));
  }

  private wait(ms: number): Promise<void> {
    const d = ms * this.speed;
    return d <= 0 ? Promise.resolve() : new Promise((r) => setTimeout(r, d));
  }

  private later(ms: number, fn: () => void): void {
    const d = ms * this.speed;
    if (d <= 0) queueMicrotask(fn);
    else setTimeout(fn, d);
  }

  // ---- persistence ---------------------------------------------------------

  private load(): void {
    if (this.options.demo) {
      this.persisted = {
        identity: this.makeIdentity('me-demo', { locale: 'he', country: 'IL', birthDate: '1998-04-12' }),
        createdServers: [],
        joinedDemo: false,
        demo: true,
      };
      this.persisted.identity.displayName = 'מורן';
      this.persisted.identity.recoveryChecked = !this.options.demoUnchecked;
    } else {
      try {
        const raw = this.options.storage?.getItem(STORAGE_KEY);
        if (raw) this.persisted = JSON.parse(raw) as Persisted;
        // Older builds stored the words; drop them.
        if (this.persisted && 'words' in this.persisted) {
          delete (this.persisted as Partial<Persisted> & { words?: unknown }).words;
          this.options.storage?.setItem(STORAGE_KEY, JSON.stringify(this.persisted));
        }
      } catch {
        this.persisted = null;
      }
    }
    if (!this.persisted) return;
    this.identity = this.persisted.identity;
    this.words = this.makeWords(this.identity.id);
    if (this.persisted.demo) DEMO_SERVERS.forEach((s) => this.addSeed(s));
    if (this.persisted.demo && this.options.hostile) this.addHostile(DEMO_SERVERS[0].id);
    if (this.persisted.demo && this.options.bidi) this.addBidi(DEMO_SERVERS[0].id);
    if (this.persisted.joinedDemo) this.addSeed(JOINABLE_SERVER);
    for (const s of this.persisted.createdServers) this.addOwnServer(s.id, s.name, s.ageRestriction, s.inviteLink);
  }

  private save(): void {
    if (!this.persisted || this.options.demo) return;
    this.persisted.identity = this.identity!;
    this.options.storage?.setItem(STORAGE_KEY, JSON.stringify(this.persisted));
  }

  private makeIdentity(id: string, input: NewIdentityInput): Identity {
    return {
      id,
      shortCode: shortCodeFor(id),
      displayName: '',
      avatarColor: AVATAR_COLORS[hashString(id) % AVATAR_COLORS.length],
      country: input.country,
      birthDate: input.birthDate,
      recoveryChecked: false,
    };
  }

  private makeWords(seed: string): string[] {
    // No repeats, so the demo grid never looks broken (BIP39 itself allows them).
    let h = hashString(seed);
    const out: string[] = [];
    for (let i = 0; out.length < 12; i++) {
      h = hash(h + i + 1);
      const w = WORDS[h % WORDS.length];
      if (!out.includes(w)) out.push(w);
    }
    return out;
  }

  // ---- seeding -------------------------------------------------------------

  /**
   * Hostile input for the security demo: look-alike and role names, invisible
   * and "zalgo" names, a colour that tries to load a remote image, and messages
   * with markup, direction overrides and huge sizes. The UI must show all of it
   * as harmless text.
   */
  private addHostile(serverId: Id): void {
    const s = this.servers.get(serverId)!;
    const owner = s.members.find((m) => m.isOwner)!;
    const RLO = String.fromCharCode(0x202e);
    const names = [
      owner.displayName.replace('ר', 'ד'), // מורן -> מודן
      `${String.fromCharCode(0x391)}dmin`, // Greek capital Alpha
      String.fromCharCode(0x3164), // a "blank" Hangul filler
      `Z${String.fromCharCode(0x336).repeat(60)}oe${RLO}`,
    ];
    const added: Member[] = names.map((displayName, i) => ({
      id: `u-hostile-${i}`,
      serverId,
      displayName,
      shortCode: shortCodeFor(`u-hostile-${i}`),
      avatarColor: i === 3 ? 'url(https://tracker.invalid/p.png)' : AVATAR_COLORS[i],
      presence: 'online',
      isOwner: false,
    }));
    s.members.push(...added);
    const ch = s.channels[0];
    ch.memberIds.push(...added.map((m) => m.id));
    // Tallest first, so the short ones are what the chat opens on.
    const bodies = [
      'line\n'.repeat(2000),
      `${'A'.repeat(30)} ${'x'.repeat(200_000)}`,
      `invoice_${RLO}fdp.exe`,
      '<img src=x onerror="alert(1)"> <a href="javascript:alert(1)">click</a> **bold** [link](javascript:alert(1))',
    ];
    bodies.forEach((body, i) => {
      const index = this.count(ch);
      ch.appendedCount++;
      ch.overrides.set(index, {
        id: this.messageId(ch.channel.id, index),
        channelId: ch.channel.id,
        index,
        authorId: added[i].id,
        body,
        createdAt: Date.now() - (bodies.length - i) * 60_000,
      });
    });
  }

  /**
   * The test table of docs/ux/specs/B17-mixed-direction.md §2, as messages:
   * Hebrew that starts with English, links, mentions, an explicit RLM, a tie
   * and two paragraphs in different directions.
   */
  private addBidi(serverId: Id): void {
    const s = this.servers.get(serverId)!;
    const ch = s.channels[0];
    const RLM = String.fromCharCode(0x200f);
    const bodies = [
      'lol זה היה מטורף',
      'GG על המשחק אתמול, 3:1 לנו!',
      '@Itamar תבדוק את ה-patch החדש',
      'version 2.4.1 fixed the crash',
      '🎉🎉',
      '3:1',
      'https://youtu.be/abcdefg תראו את זה',
      `${RLM}ok`,
      'שלום לכולם\nnpm run build',
    ];
    bodies.forEach((body, i) => {
      const index = this.count(ch);
      ch.appendedCount++;
      ch.overrides.set(index, {
        id: this.messageId(ch.channel.id, index),
        channelId: ch.channel.id,
        index,
        authorId: ch.memberIds[i % ch.memberIds.length],
        body,
        createdAt: Date.now() - (bodies.length - i) * 60_000,
      });
    });
  }

  private personId(i: number): Id {
    return `u-${i}`;
  }

  private addSeed(seed: ServerSeed): void {
    const me = this.identity!;
    const members: Member[] = [
      {
        id: me.id,
        serverId: seed.id,
        displayName: me.displayName,
        shortCode: me.shortCode,
        avatarColor: me.avatarColor,
        presence: 'online',
        isOwner: seed.owner === -1,
      },
    ];
    const offset = hashString(seed.id) % PEOPLE.length;
    for (let k = 0; k < seed.members; k++) {
      const p = (offset + k) % PEOPLE.length;
      members.push({
        id: this.personId(p),
        serverId: seed.id,
        displayName: PEOPLE[p],
        shortCode: shortCodeFor(this.personId(p)),
        avatarColor: AVATAR_COLORS[p % AVATAR_COLORS.length],
        presence: k < seed.online ? 'online' : 'offline',
        isOwner: seed.owner === p,
      });
    }
    const summary: ServerSummary = {
      id: seed.id,
      name: seed.name,
      initials: initialsOf(seed.name),
      color: seed.color,
      ageRestriction: seed.ageRestriction,
      ownerId: seed.owner === -1 ? me.id : this.personId(seed.owner),
      inviteLink: `mvp://join/${seed.id.slice(2)}-${shortCodeFor(seed.id).toLowerCase()}`,
      unread: false,
      mentions: 0,
    };
    const state: ServerState = { summary, members, channels: [] };
    const now = Date.now();
    seed.channels.forEach((c, ci) => {
      const ch: ChannelState = {
        channel: { id: c.id, serverId: seed.id, name: c.name, adult: !!c.adult, isDefault: ci === 0, unread: false, mentions: 0 },
        baseCount: c.count,
        baseStart: now - 3_600_000 - c.count * MESSAGE_GAP_MS,
        seedOffset: hashString(c.id),
        memberIds: members.slice(1).map((m) => m.id),
        overrides: new Map(),
        appendedCount: 0,
      };
      // A little unread state so the badges show up in the demo.
      if (ci === 1) ch.channel.unread = true;
      if (ci === 2 && seed.id === 's-gamers') {
        ch.channel.unread = true;
        ch.channel.mentions = 2;
      }
      state.channels.push(ch);
      this.channelIndex.set(c.id, ch);
    });
    this.servers.set(seed.id, state);
    this.refreshServerUnread(state);
  }

  private addOwnServer(id: Id, name: string, ageRestriction: AgeRestriction, inviteLink: string): ServerState {
    const me = this.identity!;
    const state: ServerState = {
      summary: {
        id,
        name,
        initials: initialsOf(name),
        color: AVATAR_COLORS[hashString(id) % AVATAR_COLORS.length],
        ageRestriction,
        ownerId: me.id,
        inviteLink,
        unread: false,
        mentions: 0,
      },
      members: [
        {
          id: me.id,
          serverId: id,
          displayName: me.displayName,
          shortCode: me.shortCode,
          avatarColor: me.avatarColor,
          presence: 'online',
          isOwner: true,
        },
      ],
      channels: [],
    };
    const ch: ChannelState = {
      channel: { id: `${id}-general`, serverId: id, name: 'general', adult: false, isDefault: true, unread: false, mentions: 0 },
      baseCount: 0,
      baseStart: Date.now(),
      seedOffset: 0,
      memberIds: [],
      overrides: new Map(),
      appendedCount: 0,
    };
    state.channels.push(ch);
    this.channelIndex.set(ch.channel.id, ch);
    this.servers.set(id, state);
    return state;
  }

  private refreshServerUnread(s: ServerState): void {
    s.summary = {
      ...s.summary,
      unread: s.channels.some((c) => c.channel.unread),
      mentions: s.channels.reduce((n, c) => n + c.channel.mentions, 0),
    };
  }

  // ---- messages ------------------------------------------------------------

  private messageId(channelId: Id, index: number): Id {
    return `${channelId}#${index}`;
  }

  private parseId(id: Id): { ch: ChannelState; index: number } {
    const at = id.lastIndexOf('#');
    const ch = this.channelIndex.get(id.slice(0, at));
    if (!ch) throw err({ code: 'not-found' });
    return { ch, index: Number(id.slice(at + 1)) };
  }

  private count(ch: ChannelState): number {
    return ch.baseCount + ch.appendedCount;
  }

  private generated(ch: ChannelState, index: number): Message {
    const h = hash(index + ch.seedOffset);
    const authorId = ch.memberIds.length ? ch.memberIds[h % ch.memberIds.length] : this.identity!.id;
    const body = BODIES[hash(h) % BODIES.length];
    const msg: Message = {
      id: this.messageId(ch.channel.id, index),
      channelId: ch.channel.id,
      index,
      authorId,
      body,
      createdAt: ch.baseStart + index * MESSAGE_GAP_MS + (h % 20_000),
    };
    if (index > 3 && h % 11 === 0) {
      const target = index - 1 - (h % 3);
      const t = this.messageAt(ch, target);
      msg.replyTo = { id: t.id, index: target, authorId: t.authorId, excerpt: t.deleted ? '' : t.body.slice(0, 80) };
    }
    if (h % 17 === 0) msg.editedAt = msg.createdAt + 60_000;
    if (h % 401 === 0) msg.mentionsMe = true;
    return msg;
  }

  private messageAt(ch: ChannelState, index: number): Message {
    return ch.overrides.get(index) ?? this.generated(ch, index);
  }

  private put(ch: ChannelState, msg: Message, isNew: boolean): void {
    ch.overrides.set(msg.index, msg);
    this.emit(isNew ? 'message.added' : 'message.updated', msg);
  }

  private serverOf(ch: ChannelState): ServerState {
    return this.servers.get(ch.channel.serverId)!;
  }

  private someoneOnline(s: ServerState): boolean {
    return s.members.some((m) => m.id !== this.identity?.id && m.presence === 'online');
  }

  /** Walks a local message through pending → sent → received. */
  private deliver(ch: ChannelState, index: number): void {
    const s = this.serverOf(ch);
    if (!this.someoneOnline(s)) return; // stays pending until someone connects
    this.later(250, () => {
      const m = ch.overrides.get(index);
      if (!m || m.sendState !== 'pending') return;
      this.put(ch, { ...m, sendState: 'sent' }, false);
      this.later(700, () => {
        const m2 = ch.overrides.get(index);
        if (m2 && m2.sendState === 'sent') this.put(ch, { ...m2, sendState: 'received' }, false);
      });
    });
  }

  // ---- identity ------------------------------------------------------------

  async getIdentity(): Promise<Identity | null> {
    return this.identity;
  }

  async createIdentity(input: NewIdentityInput): Promise<{ identity: Identity; words: string[] }> {
    const id = `me-${Date.now().toString(36)}`;
    const steps: CoreEvents['identity.progress'][] = [
      { step: 'keys', percent: 25 },
      { step: 'storage', percent: 55 },
      { step: 'encryption', percent: 85 },
      { step: 'done', percent: 100 },
    ];
    for (const s of steps) {
      await this.wait(450);
      this.emit('identity.progress', s);
    }
    this.identity = this.makeIdentity(id, input);
    this.words = this.makeWords(id);
    this.persisted = { identity: this.identity, createdServers: [], joinedDemo: false, demo: false };
    this.save();
    return { identity: this.identity, words: [...this.words] };
  }

  async getRecoveryChallenge(): Promise<{ positions: number[] }> {
    if (this.challenge?.burned) throw err({ code: 'too-many-attempts' });
    if (!this.challenge) {
      const all = Array.from({ length: 12 }, (_, i) => i + 1);
      for (let i = all.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [all[i], all[j]] = [all[j], all[i]];
      }
      this.challenge = { positions: all.slice(0, 3).sort((a, b) => a - b), failures: 0, burned: false };
    }
    return { positions: [...this.challenge.positions] };
  }

  async checkRecoveryWords(answers: Record<number, string>): Promise<RecoveryCheckResult> {
    await this.wait(150);
    const c = this.challenge;
    if (c?.burned) throw err({ code: 'too-many-attempts' });
    const given = Object.keys(answers).map(Number).sort((a, b) => a - b);
    if (!c || given.join() !== c.positions.join()) throw err({ code: 'not-allowed' });
    const wrong = c.positions.filter((p) => this.words[p - 1] !== (answers[p] ?? '').trim().toLowerCase());
    if (wrong.length) {
      // The 5th failure locks at once, so nobody types the words a 6th time to find out.
      if (++c.failures >= MAX_CHECK_FAILURES) {
        c.burned = true;
        throw err({ code: 'too-many-attempts' });
      }
      return { ok: false, wrong };
    }
    this.challenge = null;
    if (this.identity) {
      this.identity = { ...this.identity, recoveryChecked: true };
      this.save();
      this.emit('identity.changed', this.identity);
    }
    return { ok: true };
  }

  async getRecoveryWords(): Promise<string[]> {
    if (this.options.wordsError) throw err({ code: 'not-found' });
    // The real core asks for OS authentication here; success lifts a burned challenge.
    if (this.challenge?.burned) this.challenge = null;
    return [...this.words];
  }

  async restoreIdentity(words: string[]): Promise<Identity> {
    await this.wait(600);
    const clean = words.map((w) => w.trim().toLowerCase());
    const bad = clean.flatMap((w, i) => (WORDS.includes(w) ? [] : [i + 1]));
    if (clean.length !== 12 || bad.length) throw err({ code: 'invalid-words', positions: bad });
    const id = `me-${hashString(clean.join(' ')).toString(36)}`;
    this.identity = {
      ...this.makeIdentity(id, { locale: 'he', country: 'IL', birthDate: '1995-01-01' }),
      displayName: '',
      recoveryChecked: true,
    };
    this.words = clean;
    this.persisted = { identity: this.identity, createdServers: [], joinedDemo: false, demo: false };
    this.save();
    return this.identity;
  }

  async updateProfile(input: ProfileInput): Promise<Identity> {
    if (!this.identity) throw err({ code: 'not-found' });
    this.identity = { ...this.identity, ...input };
    for (const s of this.servers.values()) {
      s.members = s.members.map((m) => (m.id === this.identity!.id ? { ...m, displayName: input.displayName } : m));
    }
    this.save();
    this.emit('identity.changed', this.identity);
    return this.identity;
  }

  // ---- servers -------------------------------------------------------------

  async listServers(): Promise<ServerSummary[]> {
    return [...this.servers.values()].map((s) => s.summary);
  }

  async listChannels(serverId: Id): Promise<Channel[]> {
    const s = this.servers.get(serverId);
    if (!s) throw err({ code: 'not-found' });
    return s.channels.map((c) => c.channel);
  }

  // ---- channels (B11c, owner only) ----------------------------------------------

  private ownedServer(serverId: Id): ServerState {
    const s = this.servers.get(serverId);
    if (!s) throw err({ code: 'not-found' });
    if (s.summary.ownerId !== this.identity?.id) throw err({ code: 'not-allowed' });
    return s;
  }

  /** The core's channel-name rule: spaces become '-', runs of '-' collapse. */
  private channelName(s: ServerState, raw: string, except?: Id): string {
    const name = normalizeChannelName(raw);
    if (!name || /[\u0000-\u001f\u200b-\u200f\u202a-\u202e\u2066-\u2069]/.test(name)) throw err({ code: 'bad-request' });
    if (s.channels.some((c) => c.channel.id !== except && c.channel.name === name)) throw err({ code: 'name-taken' });
    return name;
  }

  async createChannel(serverId: Id, input: { name: string; adult: boolean }): Promise<Channel> {
    await this.wait(200);
    const s = this.ownedServer(serverId);
    const name = this.channelName(s, input.name);
    const id = `${serverId}-c${Date.now().toString(36)}${s.channels.length}`;
    const ch: ChannelState = {
      channel: { id, serverId, name, adult: input.adult === true, isDefault: false, unread: false, mentions: 0 },
      baseCount: 0,
      baseStart: Date.now(),
      seedOffset: 0,
      memberIds: s.members.filter((m) => m.id !== this.identity!.id).map((m) => m.id),
      overrides: new Map(),
      appendedCount: 0,
    };
    s.channels.push(ch);
    this.channelIndex.set(id, ch);
    this.emit('channel.added', ch.channel);
    return ch.channel;
  }

  async updateChannel(channelId: Id, input: { name?: string; adult?: boolean }): Promise<Channel> {
    await this.wait(200);
    const ch = this.channelIndex.get(channelId);
    if (!ch) throw err({ code: 'not-found' });
    const s = this.ownedServer(ch.channel.serverId);
    const name = input.name === undefined ? ch.channel.name : this.channelName(s, input.name, channelId);
    ch.channel = { ...ch.channel, name, adult: input.adult ?? ch.channel.adult };
    this.emit('channel.updated', ch.channel);
    return ch.channel;
  }

  async deleteChannel(channelId: Id): Promise<void> {
    await this.wait(200);
    const ch = this.channelIndex.get(channelId);
    if (!ch) throw err({ code: 'not-found' });
    const s = this.ownedServer(ch.channel.serverId);
    if (ch.channel.isDefault) throw err({ code: 'not-allowed' });
    s.channels = s.channels.filter((c) => c !== ch);
    this.channelIndex.delete(channelId);
    this.emit('channel.removed', { serverId: s.summary.id, channelId, name: ch.channel.name });
  }

  async listMembers(serverId: Id): Promise<Member[]> {
    const s = this.servers.get(serverId);
    if (!s) throw err({ code: 'not-found' });
    return s.members;
  }

  async createServer(input: { name: string; ageRestriction: AgeRestriction }): Promise<ServerSummary> {
    if (!this.identity?.recoveryChecked) throw err({ code: 'recovery-check-required' });
    await this.wait(300);
    const id = `s-${Date.now().toString(36)}`;
    const link = `mvp://join/${id.slice(2)}-${shortCodeFor(id).toLowerCase()}`;
    const s = this.addOwnServer(id, input.name.trim(), input.ageRestriction, link);
    this.persisted?.createdServers.push({ id, name: s.summary.name, ageRestriction: input.ageRestriction, inviteLink: link });
    this.save();
    this.emit('server.added', s.summary);
    return s.summary;
  }

  async joinServer(link: string): Promise<JoinStatus> {
    const clean = link.trim();
    if (!/join\/[\w-]+/i.test(clean)) return { phase: 'invalid', link: clean };
    const searching: JoinStatus = { phase: 'searching', link: clean };
    this.emit('join.status', searching);
    const t = setTimeout(() => {
      this.pendingJoins.delete(clean);
      if (/offline/i.test(clean)) {
        this.emit('join.status', { phase: 'nobody-online', link: clean });
      } else if (/adult/i.test(clean) && this.ageOf(this.identity!.birthDate) < 18) {
        this.emit('join.status', { phase: 'age-blocked', link: clean, required: '18+' });
      } else {
        if (!this.servers.has(JOINABLE_SERVER.id)) {
          this.addSeed(JOINABLE_SERVER);
          if (this.persisted) this.persisted.joinedDemo = true;
          this.save();
          this.emit('server.added', this.servers.get(JOINABLE_SERVER.id)!.summary);
        }
        this.emit('join.status', { phase: 'joined', link: clean, serverId: JOINABLE_SERVER.id });
      }
    }, 2200 * this.speed);
    this.pendingJoins.set(clean, t);
    return searching;
  }

  async cancelJoin(link: string): Promise<void> {
    const t = this.pendingJoins.get(link.trim());
    if (t) clearTimeout(t);
    this.pendingJoins.delete(link.trim());
  }

  private ageOf(birthDate: string): number {
    const [y, m, d] = birthDate.split('-').map(Number);
    const now = new Date();
    let age = now.getFullYear() - y;
    if (now.getMonth() + 1 < m || (now.getMonth() + 1 === m && now.getDate() < d)) age--;
    return age;
  }

  // ---- messages ------------------------------------------------------------

  async getMessageCount(channelId: Id): Promise<number> {
    const ch = this.channelIndex.get(channelId);
    if (!ch) throw err({ code: 'not-found' });
    return this.count(ch);
  }

  async getMessages(channelId: Id, start: number, end: number): Promise<MessageRange> {
    const ch = this.channelIndex.get(channelId);
    if (!ch) throw err({ code: 'not-found' });
    const from = Math.max(0, start);
    const to = Math.min(this.count(ch), end);
    const messages: Message[] = [];
    for (let i = from; i < to; i++) messages.push(this.messageAt(ch, i));
    return { channelId, start: from, messages };
  }

  async sendMessage(channelId: Id, body: string, replyToId?: Id): Promise<Message> {
    const ch = this.channelIndex.get(channelId);
    if (!ch || !this.identity) throw err({ code: 'not-found' });
    const index = this.count(ch);
    ch.appendedCount++;
    const msg: Message = {
      id: this.messageId(channelId, index),
      channelId,
      index,
      authorId: this.identity.id,
      body,
      createdAt: Date.now(),
      sendState: 'pending',
    };
    if (replyToId) {
      const { ch: rch, index: ri } = this.parseId(replyToId);
      const t = this.messageAt(rch, ri);
      msg.replyTo = { id: t.id, index: ri, authorId: t.authorId, excerpt: t.body.slice(0, 80) };
    }
    this.put(ch, msg, true);
    if (body.includes('/fail')) {
      this.later(400, () => this.put(ch, { ...ch.overrides.get(index)!, sendState: 'failed' }, false));
    } else {
      this.deliver(ch, index);
    }
    return msg;
  }

  async editMessage(messageId: Id, body: string): Promise<void> {
    const { ch, index } = this.parseId(messageId);
    const m = this.messageAt(ch, index);
    if (m.authorId !== this.identity?.id) throw err({ code: 'not-allowed' });
    this.put(ch, { ...m, body, editedAt: Date.now() }, false);
  }

  async deleteMessage(messageId: Id): Promise<void> {
    const { ch, index } = this.parseId(messageId);
    const m = this.messageAt(ch, index);
    const s = this.serverOf(ch);
    const mine = m.authorId === this.identity?.id;
    if (!mine && s.summary.ownerId !== this.identity?.id) throw err({ code: 'not-allowed' });
    this.put(ch, { ...m, body: '', deleted: true, replyTo: undefined }, false);
  }

  async retryMessage(messageId: Id): Promise<void> {
    const { ch, index } = this.parseId(messageId);
    const m = this.messageAt(ch, index);
    if (m.sendState !== 'failed') return;
    this.put(ch, { ...m, body: m.body.replace('/fail', '').trim() || m.body, sendState: 'pending' }, false);
    this.deliver(ch, index);
  }

  async markRead(channelId: Id): Promise<void> {
    const ch = this.channelIndex.get(channelId);
    if (!ch || (!ch.channel.unread && ch.channel.mentions === 0)) return;
    ch.channel = { ...ch.channel, unread: false, mentions: 0 };
    const s = this.serverOf(ch);
    this.refreshServerUnread(s);
    this.emit('channel.updated', ch.channel);
    this.emit('server.updated', s.summary);
  }

  // ---- safety --------------------------------------------------------------

  async blockUser(userId: Id): Promise<void> {
    for (const s of this.servers.values()) {
      if (!s.members.some((m) => m.id === userId)) continue;
      s.members = s.members.map((m) => (m.id === userId ? { ...m, blocked: !m.blocked } : m));
      this.emit('members.changed', { serverId: s.summary.id, members: s.members });
    }
  }

  async kickMember(serverId: Id, userId: Id): Promise<void> {
    const s = this.servers.get(serverId);
    if (!s) throw err({ code: 'not-found' });
    if (s.summary.ownerId !== this.identity?.id || userId === this.identity.id) throw err({ code: 'not-allowed' });
    await this.wait(200);
    s.members = s.members.filter((m) => m.id !== userId);
    for (const c of s.channels) c.memberIds = c.memberIds.filter((id) => id !== userId);
    this.emit('members.changed', { serverId, members: s.members });
    this.emit('notice', { kind: 'encryption-renewed', serverId });
  }

  // ---- app -----------------------------------------------------------------

  async getStorageUsage(): Promise<StorageUsage> {
    const messagesBytes = [...this.channelIndex.values()].reduce((n, c) => n + this.count(c) * 380, 0);
    const filesBytes = 412 * 1024 ** 2;
    return { usedBytes: messagesBytes + filesBytes, limitBytes: this.storageLimit, messagesBytes, filesBytes };
  }

  async setStorageLimit(bytes: number): Promise<void> {
    this.storageLimit = bytes;
  }

  async clearCache(): Promise<void> {
    await this.wait(300);
  }

  async exportDiagnostics(): Promise<string> {
    const info = await this.getAppInfo();
    return JSON.stringify(
      {
        app: info,
        exportedAt: new Date().toISOString(),
        platform: typeof navigator !== 'undefined' ? navigator.userAgent : 'unknown',
        servers: this.servers.size,
        // Never include message text, words or the birth date.
        log: ['mock core started', 'connections: simulated'],
      },
      null,
      2,
    );
  }

  async getAppInfo(): Promise<AppInfo> {
    return { version: '0.1.0-alpha', updateChannel: this.releaseChannel };
  }

  async setUpdateChannel(channel: UpdateChannel): Promise<void> {
    this.releaseChannel = channel;
  }

  async restartToUpdate(): Promise<void> {
    if (typeof location !== 'undefined') location.reload();
  }

  // ---- simulation ----------------------------------------------------------

  private simulateIncoming(): void {
    const live = [...this.servers.values()].filter((s) => this.someoneOnline(s) && s.channels.some((c) => c.baseCount));
    if (!live.length || !this.identity) return;
    const r = Math.floor(Math.random() * 1e9);
    const s = live[r % live.length];
    const ch = s.channels[hash(r) % s.channels.length];
    const authors = s.members.filter((m) => m.presence === 'online' && m.id !== this.identity!.id);
    const author = authors[hash(r + 1) % authors.length];
    const mention = hash(r + 2) % 7 === 0;
    const index = this.count(ch);
    ch.appendedCount++;
    const body = BODIES[hash(r + 3) % BODIES.length];
    this.put(
      ch,
      {
        id: this.messageId(ch.channel.id, index),
        channelId: ch.channel.id,
        index,
        authorId: author.id,
        body: mention ? `@${this.identity.displayName || this.identity.shortCode} ${body}` : body,
        createdAt: Date.now(),
        mentionsMe: mention,
      },
      true,
    );
    ch.channel = { ...ch.channel, unread: true, mentions: ch.channel.mentions + (mention ? 1 : 0) };
    this.refreshServerUnread(s);
    this.emit('channel.updated', ch.channel);
    this.emit('server.updated', s.summary);
  }

  private simulatePresence(): void {
    for (const s of this.servers.values()) {
      if (s.summary.id === 's-friends') continue; // the "nobody online" example
      const others = s.members.filter((m) => m.id !== this.identity?.id);
      if (!others.length) continue;
      const m = others[Math.floor(Math.random() * others.length)];
      s.members = s.members.map((x) =>
        x.id === m.id ? { ...x, presence: x.presence === 'online' ? 'offline' : 'online' } : x,
      );
      this.emit('members.changed', { serverId: s.summary.id, members: s.members });
    }
  }
}
