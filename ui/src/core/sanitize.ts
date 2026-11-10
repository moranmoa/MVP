// Everything that reaches the UI from the core may have been written by another
// person: names, server and channel names, message text, bios, colours, avatar
// images, invite links. `guardCore` wraps the core once (in App) so every
// command result and every event passes through the cleaners below before any
// screen sees it. React already escapes text, so this is not about HTML; it is
// about text that lies about itself (direction overrides, invisible letters,
// look-alike names), values that reach CSS or <img>, and sizes that freeze the
// renderer.

import { cleanStunUrl } from '../voice/sdp';
import type { CoreApi, CoreEventName, CoreEvents } from './api';
import type {
  AgeRestriction,
  Channel,
  Identity,
  Invite,
  JoinStatus,
  Member,
  Message,
  MessageRange,
  ServerSummary,
  CallState,
  VoiceRoom,
  Id,
} from './types';

export const NAME_MAX = 32;
export const SERVER_NAME_MAX = 48;
export const CHANNEL_NAME_MAX = 32;
/** Message length limit, in code points (an emoji counts as one, QA-17). */
export const BODY_MAX = 4000;
/** The core's limit for a message record body (`records`, Kind::Message), in UTF-8 bytes. */
export const BODY_MAX_BYTES = 16 * 1024;
export const BODY_MAX_LINES = 100;
export const BIO_MAX = 300;
export const BIO_MAX_LINES = 8;
export const EXCERPT_MAX = 140;
export const LINK_MAX = 512;
/** About 512 KB of image once base64-decoded. */
export const AVATAR_DATA_URL_MAX = 700_000;
/** Combining marks kept on one letter: enough for Hebrew vowels and cantillation, not for "zalgo". */
const MARKS_PER_LETTER = 4;
const FALLBACK_COLOR = '#868e96';

/** Embeddings, overrides and isolates (U+202A–202E, U+2066–2069): they reorder text around them. */
const BIDI_CONTROLS = /[\u202A-\u202E\u2066-\u2069]/g;
/** Direction marks: fine inside a message, never needed in a name. */
const BIDI_MARKS = /[\u200E\u200F\u061C]/g;
/**
 * Invisible or "blank" characters that let a name look empty or identical to
 * another one: C0/C1 controls (except tab and newline), soft hyphen, fillers,
 * zero-width spaces, word joiner, invisible operators, BOM, interlinear
 * annotations, tag characters and most variation selectors.
 */
const INVISIBLE =
  /[\u0000-\u0008\u000B-\u001F\u007F-\u009F\u00AD\u034F\u115F\u1160\u17B4\u17B5\u180B-\u180F\u200B\u200C\u2028\u2029\u2060-\u2064\u206A-\u206F\u2800\u3164\uFE00-\uFE0D\uFEFF\uFFA0\uFFF9-\uFFFB]|[\u{E0000}-\u{E007F}\u{E0100}-\u{E01EF}]/gu;
/** A zero-width joiner is only kept inside an emoji sequence (👩‍💻). */
const STRAY_ZWJ = /(?<!\p{Extended_Pictographic}\uFE0F?)\u200D|\u200D(?!\p{Extended_Pictographic})/gu;
const MARK_RUN = new RegExp(`(\\p{M}{${MARKS_PER_LETTER}})\\p{M}+`, 'gu');
const ODD_SPACES = /[\s\u00A0\u1680\u2000-\u200A\u202F\u205F\u3000]+/gu;
/** Something that looks like an identity code (K7QM-2XPA) inside a name. */
const CODE_LIKE = /[\p{Lu}\d]{4}[-\u2010-\u2015][\p{Lu}\d]{4}/gu;
const VISIBLE = /[\p{L}\p{N}\p{S}\p{P}]/u;

const segmenter = typeof Intl !== 'undefined' && 'Segmenter' in Intl ? new Intl.Segmenter() : null;

function graphemes(s: string): string[] {
  return segmenter ? Array.from(segmenter.segment(s), (x) => x.segment) : Array.from(s);
}

/** Cuts a UTF-16 string without leaving half a surrogate pair at the end. */
function cut(s: string, max: number): string {
  if (s.length <= max) return s;
  const end = /[\uD800-\uDBFF]/.test(s[max - 1]) ? max - 1 : max;
  return s.slice(0, end);
}

function stripCommon(s: string): string {
  return s.replace(BIDI_CONTROLS, '').replace(INVISIBLE, '').replace(STRAY_ZWJ, '').replace(MARK_RUN, '$1');
}

/**
 * A display name, server or channel name: one line, no direction tricks, no
 * invisible letters, no fake identity code, at most `max` user-perceived
 * characters. Returns '' when nothing visible is left (callers fall back to the
 * identity code).
 */
export function cleanName(raw: unknown, max = NAME_MAX): string {
  if (typeof raw !== 'string') return '';
  // Cap before the regexes so a megabyte "name" costs nothing.
  let s = cut(raw, max * 16).normalize('NFKC');
  s = stripCommon(s).replace(BIDI_MARKS, '').replace(CODE_LIKE, '').replace(ODD_SPACES, ' ').trim();
  if (!VISIBLE.test(s)) return '';
  const g = graphemes(s);
  return g.length > max ? g.slice(0, max).join('').trim() : s;
}

/** The first `max` code points of a string, and whether anything was left out. */
function cutCodePoints(s: string, max: number): { head: string; cut: boolean } {
  if (s.length <= max) return { head: s, cut: false };
  // A code point is at most 2 UTF-16 units, so 2·max units always hold max code points.
  const head = cut(s, max * 2);
  const points = Array.from(head);
  if (points.length <= max && head.length === s.length) return { head: s, cut: false };
  return { head: points.slice(0, max).join(''), cut: true };
}

/** Multi-line text (messages, bios): keeps newlines and direction marks, drops the rest. Limits are in code points. */
export function cleanText(raw: unknown, maxChars = BODY_MAX, maxLines = BODY_MAX_LINES): { text: string; clipped: boolean } {
  if (typeof raw !== 'string') return { text: '', clipped: false };
  const capped = cutCodePoints(raw, maxChars);
  let clipped = capped.cut;
  let s = stripCommon(capped.head.replace(/\r\n?/g, '\n'));
  let lines = 0;
  for (let i = s.indexOf('\n'); i !== -1; i = s.indexOf('\n', i + 1)) {
    if (++lines === maxLines) {
      s = s.slice(0, i);
      clipped = true;
      break;
    }
  }
  return { text: s, clipped };
}

/** One-line excerpt of a message, for reply previews. */
export function cleanExcerpt(raw: unknown): string {
  const { text } = cleanText(raw, EXCERPT_MAX * 4, Infinity);
  const one = text.replace(ODD_SPACES, ' ').trim();
  const g = graphemes(one);
  return g.length > EXCERPT_MAX ? `${g.slice(0, EXCERPT_MAX).join('')}…` : one;
}

/** Only `#rrggbb`: these values end up in inline CSS, where `url(...)` would load a remote image. */
export function safeColor(raw: unknown): string {
  return typeof raw === 'string' && /^#[0-9a-fA-F]{6}$/.test(raw) ? raw : FALLBACK_COLOR;
}

/**
 * Avatars are inline raster images only. A remote URL would tell its owner
 * the IP address of everyone who scrolls past; SVG can carry scripts when
 * opened on its own.
 */
export function safeImageSrc(raw: unknown): string | undefined {
  if (typeof raw !== 'string' || raw.length > AVATAR_DATA_URL_MAX) return undefined;
  return /^data:image\/(?:png|jpeg|webp|gif);base64,[A-Za-z0-9+/]+={0,2}$/.test(raw) ? raw : undefined;
}

/** Invite links are shown and copied as plain text, so they must stay one honest line. */
export function cleanLink(raw: unknown): string {
  if (typeof raw !== 'string') return '';
  return stripCommon(cut(raw, LINK_MAX)).replace(BIDI_MARKS, '').replace(/\s+/g, '');
}

function cleanCode(raw: unknown): string {
  return typeof raw === 'string' ? raw.replace(/[^A-Z0-9-]/g, '').slice(0, 9) : '';
}

function count(raw: unknown): number {
  return typeof raw === 'number' && Number.isFinite(raw) ? Math.max(0, Math.min(9999, Math.floor(raw))) : 0;
}

function oneOf<T extends string>(raw: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(raw as T) ? (raw as T) : fallback;
}

// Unknown age limits are treated as the strictest one.
const age = (raw: unknown): AgeRestriction => oneOf(raw, ['none', '16+', '18+'] as const, '18+');

export function cleanIdentity(i: Identity): Identity {
  return {
    ...i,
    shortCode: cleanCode(i.shortCode),
    displayName: cleanName(i.displayName),
    avatarColor: safeColor(i.avatarColor),
    avatarDataUrl: safeImageSrc(i.avatarDataUrl),
    bio: i.bio === undefined ? undefined : cleanText(i.bio, BIO_MAX, BIO_MAX_LINES).text,
  };
}

export function cleanServer(s: ServerSummary): ServerSummary {
  const name = cleanName(s.name, SERVER_NAME_MAX);
  return {
    ...s,
    name,
    initials: cleanName(s.initials, 2) || graphemes(name)[0] || '?',
    color: safeColor(s.color),
    ageRestriction: age(s.ageRestriction),
    inviteLink: cleanLink(s.inviteLink),
    unread: s.unread === true,
    mentions: count(s.mentions),
    closed: s.closed === true,
  };
}

const when = (raw: unknown): number | undefined => (typeof raw === 'number' && Number.isFinite(raw) && raw > 0 ? raw : undefined);
const id = (raw: unknown): string => String(raw ?? '').slice(0, 128);

export function cleanInvite(i: Invite): Invite {
  return {
    id: id(i.id),
    createdBy: id(i.createdBy),
    expiresAt: when(i.expiresAt),
    maxUses: count(i.maxUses),
    uses: count(i.uses),
    revoked: i.revoked === true,
  };
}

export function cleanChannel(c: Channel): Channel {
  return {
    ...c,
    name: cleanName(c.name, CHANNEL_NAME_MAX) || '…',
    adult: c.adult === true,
    isDefault: c.isDefault === true,
    unread: c.unread === true,
    mentions: count(c.mentions),
  };
}

export function cleanMember(m: Member): Member {
  return {
    ...m,
    displayName: cleanName(m.displayName),
    shortCode: cleanCode(m.shortCode),
    avatarColor: safeColor(m.avatarColor),
    presence: oneOf(m.presence, ['online', 'offline'] as const, 'offline'),
    connection: m.presence === 'online' ? (['direct', 'relay'] as const).find((c) => c === m.connection) ?? null : null,
    isOwner: m.isOwner === true,
    blocked: m.blocked === true,
    bio: m.bio === undefined ? undefined : cleanText(m.bio, BIO_MAX, BIO_MAX_LINES).text,
  };
}

export function cleanMessage(m: Message): Message {
  const { text, clipped } = cleanText(m.body);
  return {
    ...m,
    body: text,
    clipped: clipped || undefined,
    replyTo: m.replyTo && { ...m.replyTo, excerpt: cleanExcerpt(m.replyTo.excerpt) },
    sendState: m.sendState && oneOf(m.sendState, ['pending', 'sent', 'received', 'failed'] as const, 'failed'),
    mentionsMe: m.mentionsMe === true,
  };
}

function cleanJoin(j: JoinStatus): JoinStatus {
  const link = cleanLink(j.link);
  return j.phase === 'age-blocked' ? { ...j, link, required: age(j.required) } : { ...j, link };
}

const cleanRange = (r: MessageRange): MessageRange => ({ ...r, messages: r.messages.map(cleanMessage) });

/**
 * mvs://join/<server id>/<invite record id>/<server secret>, each 32 bytes in
 * hex (GAP-4: the secret is required to sync the server). The secret is a
 * bearer credential: the link is never logged, only shown, copied and passed
 * to joinServer.
 */
const INVITE_LINK = /^mvs:\/\/join\/([0-9a-f]{64})\/([0-9a-f]{64})\/([0-9a-f]{64})\/?$/;

/** The canonical invite link, or '' when it is not exactly that format. */
export function cleanInviteLink(raw: unknown): string {
  if (typeof raw !== 'string' || raw.length > 256) return '';
  const m = INVITE_LINK.exec(raw.trim().toLowerCase());
  return m ? `mvs://join/${m[1]}/${m[2]}/${m[3]}` : '';
}

/** A member / identity id from the core: 64 lower-case hex, or '' (demo ids pass as cleaned names). */
function cleanHexId(raw: unknown): Id {
  if (typeof raw !== 'string') return '';
  return /^[0-9a-f]{64}$/.test(raw) ? raw : cleanName(raw, 64);
}

/** At most 5 people in a call (#223); anything else is dropped. */
export function cleanCall(c: CallState): CallState {
  const participants = Array.isArray(c?.participants) ? c.participants : [];
  return {
    channelId: cleanHexId(c?.channelId),
    participants: participants
      .slice(0, 5)
      .map((p) => ({ identity: cleanHexId(p?.identity), muted: p?.muted === true }))
      .filter((p) => p.identity),
    full: c?.full === true,
  };
}

/**
 * An id the UI accepts in a voice room: 64 lower-case hex from the real core, or a
 * short (≤ 40) word-character id from the mock. Nothing else gets through.
 */
const ROOM_ID = /^(?:[0-9a-f]{64}|[A-Za-z0-9_-]{1,40})$/;
const isRoomId = (v: unknown): v is Id => typeof v === 'string' && ROOM_ID.test(v);
const onlyKeys = (o: object, keys: string[]) => Object.keys(o).every((k) => keys.includes(k));

/**
 * A voice room from the core, or null to drop it: a bad id, more than 5 people,
 * a person twice, a non-boolean `muted` or any unknown field (voice-room contract).
 */
export function cleanVoiceRoom(r: unknown): VoiceRoom | null {
  if (!r || typeof r !== 'object' || !onlyKeys(r, ['serverId', 'roomId', 'participants'])) return null;
  const { serverId, roomId, participants } = r as Record<string, unknown>;
  if (!isRoomId(serverId) || !isRoomId(roomId) || !Array.isArray(participants) || participants.length > 5) return null;
  const out: VoiceRoom['participants'] = [];
  for (const p of participants as unknown[]) {
    if (!p || typeof p !== 'object' || !onlyKeys(p, ['userId', 'muted'])) return null;
    const { userId, muted } = p as Record<string, unknown>;
    if (!isRoomId(userId) || typeof muted !== 'boolean' || out.some((x) => x.userId === userId)) return null;
    out.push({ userId, muted });
  }
  return { serverId, roomId, participants: out };
}

/** A cleaner may return null to drop the event altogether. */
const EVENT_CLEANERS: { [E in CoreEventName]?: (p: CoreEvents[E]) => CoreEvents[E] | null } = {
  'identity.changed': cleanIdentity,
  'server.added': cleanServer,
  'server.updated': cleanServer,
  'server.removed': (p) => ({ serverId: id(p.serverId), reason: p.reason === 'left' || p.reason === 'closed' ? p.reason : undefined }),
  'server.closed': (p) => ({ serverId: id(p.serverId) }),
  'channel.updated': cleanChannel,
  'channel.added': cleanChannel,
  'channel.removed': (p) => ({ serverId: p.serverId, channelId: p.channelId, name: cleanName(p.name, CHANNEL_NAME_MAX) || '…' }),
  'members.changed': (p) => ({ ...p, members: p.members.map(cleanMember) }),
  'message.added': cleanMessage,
  'message.updated': cleanMessage,
  'join.status': cleanJoin,
  'link.open': (p) => ({ link: cleanInviteLink(p?.link) }),
  'call.state': (c) => cleanCall(c),
  'voice.room': (r) => cleanVoiceRoom(r),
  // For the voice engine: anything malformed is dropped, never coerced.
  'call.signal': (s) => {
    const kind = (['offer', 'answer', 'ice'] as const).find((k) => k === s?.kind);
    if (!kind || typeof s?.from !== 'string' || !/^[0-9a-f]{64}$/.test(s.from)) return null;
    if (typeof s.payload !== 'string' || s.payload.length > 12 * 1024) return null;
    if (kind !== 'ice' && s.fingerprintVerified !== true) return null;
    return { from: s.from, kind, payload: s.payload, fingerprintVerified: s.fingerprintVerified === true };
  },
  'core.status': (s) => {
    // The STUN URL only if it is a plain stun:/stuns: one (TURN, credentials: dropped).
    const stunUrl = cleanStunUrl(s?.stunUrl);
    return {
      state: s?.state === 'up' || s?.state === 'down' ? s.state : 'starting',
      detail: (['missing', 'start-failed', 'crashed', 'crash-loop', 'keyring'] as const).find((d) => d === s?.detail),
      ...(stunUrl ? { stunUrl } : {}),
    };
  },
  'update.ready': (u) => ({ version: cleanName(u.version, 32), waitingForCall: u.waitingForCall === true }),
  // Release notes come from the signed manifest, but they are still shown as
  // plain text only, clipped like a message.
  'update.installed': (u) => ({
    version: cleanName(u.version, 32),
    notes: typeof u.notes === 'string' ? cleanText(u.notes, 2000, 30).text : null,
  }),
  'update.rolledBack': (u) => ({ failed: cleanName(u.failed, 32), running: cleanName(u.running, 32) }),
};

/** The core as the UI sees it: same API, every peer-supplied value cleaned. */
export function guardCore(core: CoreApi): CoreApi {
  const wrap =
    <A extends unknown[], R>(fn: (...a: A) => Promise<R>, clean: (r: R) => R) =>
    async (...a: A) =>
      clean(await fn.apply(core, a));
  const maybe = <T>(clean: (x: T) => T) => (x: T | null) => (x ? clean(x) : x);

  const overrides: Partial<CoreApi> = {
    getIdentity: wrap(core.getIdentity, maybe(cleanIdentity)),
    createIdentity: wrap(core.createIdentity, (r) => ({ ...r, identity: cleanIdentity(r.identity) })),
    restoreIdentity: wrap(core.restoreIdentity, cleanIdentity),
    updateProfile: wrap(core.updateProfile, cleanIdentity),
    listServers: wrap(core.listServers, (l) => l.map(cleanServer)),
    listChannels: wrap(core.listChannels, (l) => l.map(cleanChannel)),
    createChannel: wrap(core.createChannel, cleanChannel),
    updateChannel: wrap(core.updateChannel, cleanChannel),
    listMembers: wrap(core.listMembers, (l) => l.map(cleanMember)),
    createServer: wrap(core.createServer, cleanServer),
    renameServer: wrap(core.renameServer, cleanServer),
    getUpdateNeeded: wrap(core.getUpdateNeeded, (l) => (Array.isArray(l) ? l.filter((x): x is string => typeof x === 'string').map(id) : [])),
    createInvite: wrap(core.createInvite, (r) => ({ ...r, id: id(r.id), link: cleanLink(r.link), expiresAt: when(r.expiresAt) })),
    listInvites: wrap(core.listInvites, (l) => l.map(cleanInvite)),
    joinServer: wrap(core.joinServer, cleanJoin),
    joinCall: wrap(core.joinCall, cleanCall),
    getVoiceRoom: wrap(core.getVoiceRoom, (r) => {
      const room = cleanVoiceRoom(r);
      if (!room) throw { code: 'internal' };
      return room;
    }),
    getMessages: wrap(core.getMessages, cleanRange),
    sendMessage: wrap(core.sendMessage, cleanMessage),
    on: <E extends CoreEventName>(event: E, handler: (p: CoreEvents[E]) => void) => {
      const clean = EVENT_CLEANERS[event] as ((p: CoreEvents[E]) => CoreEvents[E] | null) | undefined;
      return core.on(
        event,
        clean
          ? (p) => {
              const c = clean(p);
              if (c !== null) handler(c);
            }
          : handler,
      );
    },
  };
  // Everything else (commands that only send) goes straight to the core.
  return new Proxy(core, {
    get(target, prop, receiver) {
      if (Object.hasOwn(overrides, prop)) return overrides[prop as keyof CoreApi];
      const v = Reflect.get(target, prop, receiver);
      return typeof v === 'function' ? v.bind(target) : v;
    },
  });
}

/**
 * Reduces a name to how it looks rather than what it is, so "Μοrаn" (Greek
 * Mu, Cyrillic a) and "Moran" compare equal. Used to warn about members whose
 * name copies the owner's; the identity code next to every name is the real
 * proof of who someone is.
 */
export function nameSkeleton(name: string): string {
  let s = name.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase();
  s = Array.from(s, (ch) => CONFUSABLE[ch] ?? ch).join('');
  return s
    .replace(/[^\p{L}\p{N}]/gu, '')
    .replace(/rn/g, 'm')
    .replace(/vv/g, 'w')
    .replace(/cl/g, 'd');
}

const CONFUSABLE: Record<string, string> = {
  // Cyrillic
  а: 'a', в: 'b', е: 'e', ё: 'e', з: '3', і: 'l', ї: 'l', ј: 'j', к: 'k', м: 'm', н: 'h', о: 'o', р: 'p',
  с: 'c', т: 't', у: 'y', х: 'x', ѕ: 's', һ: 'h', ԁ: 'd', ԛ: 'q', ԝ: 'w', ь: 'b', ӏ: 'l',
  // Greek
  α: 'a', β: 'b', γ: 'y', ε: 'e', ζ: 'z', η: 'n', ι: 'l', κ: 'k', μ: 'm', ν: 'v', ο: 'o', ρ: 'p',
  τ: 't', υ: 'u', χ: 'x', ω: 'w',
  // Latin and digits that pass for each other
  i: 'l', '1': 'l', '|': 'l', '0': 'o',
  // Hebrew letters that are easy to mix up at small sizes
  ן: 'ו', ר: 'ד', ח: 'ה', ם: 'ס',
};
