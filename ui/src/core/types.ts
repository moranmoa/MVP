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
  bio?: string;
  country: string;
  birthDate: BirthDate;
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
}

export interface Channel {
  id: Id;
  serverId: Id;
  name: string;
  /** Adults-only channel: needs 18+ even on a server without a limit. */
  adult: boolean;
  unread: boolean;
  mentions: number;
}

export type Presence = 'online' | 'offline';

export interface Member {
  id: Id;
  serverId: Id;
  displayName: string;
  shortCode: string;
  avatarColor: string;
  presence: Presence;
  isOwner: boolean;
  /** Blocked by me: their messages are collapsed on this device only. */
  blocked?: boolean;
  bio?: string;
}

/** Commands reject with this shape; `code` maps to a translated message. */
export interface CoreError {
  code: 'invalid-words' | 'not-allowed' | 'not-found' | 'recovery-check-required';
  /** 1-based word positions that were not recognised (restore). */
  positions?: number[];
}

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
  /** Set by the UI when the text was too long or had too many lines to show in full. */
  clipped?: boolean;
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
}
