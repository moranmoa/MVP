// Data shapes shared by the UI and the core. They mirror what the Rust core
// will send over IPC, so they are plain JSON: no classes, no Dates, no Maps.

export type Id = string;
/** Milliseconds since the Unix epoch. */
export type Timestamp = number;

/** ISO date `YYYY-MM-DD`. Stays on the device; servers only get an age claim. */
export type BirthDate = string;

export type Locale = 'he' | 'en';

export interface Identity {
  id: Id;
  /** Short code derived from the public identity, shown next to names. */
  shortCode: string;
  displayName: string;
  /** CSS colour for the generated avatar; real avatars come later. */
  avatarColor: string;
  avatarDataUrl?: string;
  /** #359: my chosen colour (#rrggbb), shown to others; falls back to avatarColor. */
  color?: string;
  bio?: string;
  country: string;
  birthDate: BirthDate;
  /**
   * B7 §9 (#306): a correction that raises my age, waiting its 30 days. Until
   * `effectiveAt` my current age stays. Absent otherwise; identity.changed
   * fires when it takes effect.
   */
  pendingBirthDate?: { date: BirthDate; effectiveAt: Timestamp };
  /** False until the person proved they wrote the 12 words down. */
  recoveryChecked: boolean;
}

export interface NewIdentityInput {
  locale: Locale;
  country: string;
  birthDate: BirthDate;
}

export interface ProfileInput {
  displayName: string;
  bio?: string;
  avatarDataUrl?: string;
  /** #359: a 48×48 WebP/PNG (≤ 2,560 bytes) that others see. Absent = unchanged, '' = removed. */
  avatarSmall?: string;
  /** #359: #rrggbb. Absent = unchanged, '' = cleared. */
  color?: string;
}

export type AgeRestriction = 'none' | '16+' | '18+';

export interface ServerSummary {
  id: Id;
  name: string;
  /** Up to two letters drawn in the rail when there is no icon. */
  initials: string;
  color: string;
  ageRestriction: AgeRestriction;
  ownerId: Id;
  inviteLink: string;
  unread: boolean;
  mentions: number;
  /** Closed by its owner (#261): read-only until it is purged 30 days later. */
  closed?: boolean;
}

export interface Channel {
  id: Id;
  serverId: Id;
  name: string;
  /** Adults-only channel: needs 18+ even on a server without a limit. */
  adult: boolean;
  /** The server's first channel (#general): it can be renamed but not deleted. */
  isDefault: boolean;
  unread: boolean;
  mentions: number;
  /**
   * G-11 (#323): the shared order key (base-62, compared as a string). Only
   * informative: the order is the core's list order, never a re-sort by this.
   */
  position?: string;
  /**
   * G-21: the index of the first message I haven't read, from the core's read
   * marker (my own messages don't count). Absent when everything is read.
   */
  firstUnreadIndex?: number;
  /** G-21: how many messages from others I haven't read. */
  unreadCount?: number;
}

export type Presence = 'online' | 'offline';

/** How we reach an online member: directly (e.g. same network) or through the relay. */
export type Connection = 'direct' | 'relay';

export interface Member {
  id: Id;
  serverId: Id;
  displayName: string;
  shortCode: string;
  avatarColor: string;
  presence: Presence;
  /** Online members only (#196); `null` for me and for members who are offline. */
  connection?: Connection | null;
  isOwner: boolean;
  /** Blocked by me: their messages are collapsed on this device only. */
  blocked?: boolean;
  bio?: string;
  /** #359: the member's small picture (data:image/webp|png), from their profile post. */
  avatarDataUrl?: string;
  /** #359: the member's chosen colour (#rrggbb). */
  color?: string;
}

/** `keyring`: the OS keychain is missing or unusable (QA-20; core code `keyring-unavailable`). */
export type CoreDownDetail = 'missing' | 'start-failed' | 'crashed' | 'crash-loop' | 'keyring';

/** The desktop shell's view of the core process (T8-01). */
export interface CoreStatus {
  state: 'starting' | 'up' | 'down';
  detail?: CoreDownDetail;
  /** Our STUN server for voice (#222), from the shell's config; absent without one. */
  stunUrl?: string;
}

/** Commands reject with this shape; `code` maps to a translated message. */
export interface CoreError {
  code:
    | 'invalid-words'
    | 'not-allowed'
    | 'not-found'
    | 'recovery-check-required'
    | 'auth-cancelled'
    | 'too-many-attempts'
    | 'name-taken'
    // From the real core (core/api) and the desktop shell (desktop/src/sidecar.js):
    | 'not-implemented'
    | 'bad-request'
    | 'internal'
    | 'timeout'
    | 'core-unavailable'
    | 'core-restarted'
    /** #304, desktop: the native "sign out without the words" dialog was declined. */
    | 'cancelled'
    | 'too-large'
    | 'keyring-unavailable'
    // Voice v1 (#223)
    | 'call-full'
    | 'relay-only'
    /** #294: my own identity is already in this room from another computer (clears ~30 s after it stops). */
    | 'in-call-elsewhere'
    // In-app update (core/api CONFORMANCE "Updates")
    | 'offline'
    | 'invalid-update'
    | 'check-clock'
    | 'nothing-to-download'
    | 'nothing-staged'
    | 'disk-full'
    // Server lifecycle (#261): a member's MVS is too old for rename, leave or close; nothing was signed.
    | 'update-needed';
  /** With `core-unavailable` (desktop shell, T8-01): why the core is not answering. */
  detail?: CoreDownDetail;
  /** 1-based word positions that were not recognised (restore). */
  positions?: number[];
}

/** Result of checking the words of the current recovery challenge. */
export type RecoveryCheckResult = { ok: true } | { ok: false; wrong: number[] };

export function isCoreError(e: unknown): e is CoreError {
  return typeof e === 'object' && e !== null && 'code' in e;
}

/**
 * pending  – stored locally, nobody from the server is connected yet
 * sent     – handed to at least one connection
 * received – at least one other member confirmed it
 * failed   – could not be signed or stored; can be retried
 */
export type SendState = 'pending' | 'sent' | 'received' | 'failed';

export interface Message {
  id: Id;
  channelId: Id;
  /** Position in the merged channel view; stable because deletes are tombstones. */
  index: number;
  authorId: Id;
  body: string;
  createdAt: Timestamp;
  editedAt?: Timestamp;
  deleted?: boolean;
  replyTo?: { id: Id; index: number; authorId: Id; excerpt: string };
  /** Only set on messages written on this device. */
  sendState?: SendState;
  mentionsMe?: boolean;
  /**
   * G-24 (#330): the users this message mentions who are members now, as
   * 64-hex ids. The body holds them as <@id> tokens; the UI shows names from the
   * member list, and a token whose id isn't here as a former member.
   */
  mentions?: string[];
  /** Set by the UI when the text was too long or had too many lines to show in full. */
  clipped?: boolean;
  /**
   * Only on the one message.updated that sends a message held for its channel
   * key (#288): the `pending-…` id of the held entry this message replaces.
   */
  replaces?: Id;
}

export interface MessageRange {
  channelId: Id;
  start: number;
  messages: Message[];
}

export type JoinStatus =
  | { phase: 'searching'; link: string }
  | { phase: 'nobody-online'; link: string }
  | { phase: 'joined'; link: string; serverId: Id }
  | { phase: 'invalid'; link: string }
  | { phase: 'age-blocked'; link: string; required: AgeRestriction };

export interface StorageUsage {
  usedBytes: number;
  limitBytes: number;
  messagesBytes: number;
  filesBytes: number;
}

export interface UpdateInfo {
  version: string;
  /** While in a voice call the update waits until the call ends. */
  waitingForCall: boolean;
}

export type UpdateChannel = 'stable' | 'beta';

export interface AppInfo {
  version: string;
  updateChannel: UpdateChannel;
  /** Relay-only (GAP-6r, #272): this device is reached only through the relay; calls are off. */
  relayOnly?: boolean;
}

/** An invite to a server (#214). `maxUses` 0 is unlimited; no `expiresAt` is never. */
export interface Invite {
  id: Id;
  createdBy: Id;
  expiresAt?: number;
  maxUses: number;
  uses: number;
  revoked: boolean;
}

export interface CreatedInvite {
  id: Id;
  link: string;
  expiresAt?: number;
  maxUses?: number;
}

/** Voice v1 (#223). `identity` is a member id (64 hex). */
export interface CallParticipant {
  identity: Id;
  muted: boolean;
}

/** My current call. After leaveCall, `participants` is empty. `full` = 5 in. */
export interface CallState {
  channelId: Id;
  participants: CallParticipant[];
  full: boolean;
  /** Relay-only is on (#272): with no participants, my call was ended for it. */
  relayOnly?: boolean;
}

/** Someone in a server's voice room. `userId` is a member id. */
export interface VoiceParticipant {
  userId: Id;
  muted: boolean;
}

/**
 * The voice room of a server (voice v1 amendment): one per server, open to every
 * member. `roomId` is what joinCall takes. At most 5 people, me included when in.
 */
export interface VoiceRoom {
  serverId: Id;
  roomId: Id;
  participants: VoiceParticipant[];
  /** Relay-only is on here (#272): "Join" is off before trying. */
  relayOnly?: boolean;
}

/** WebRTC signalling between call members; for the voice engine (ui/src/voice). */
export interface CallSignal {
  from: Id;
  kind: 'offer' | 'answer' | 'ice';
  payload: string;
  fingerprintVerified: boolean;
}
