// The contract between the UI and the core. Today `mock.ts` implements it in
// the page; later a thin adapter will forward the same commands over Tauri
// `invoke` (or Electron IPC) and turn core notifications into these events.
// Rules that keep that swap painless:
//   - every command is async and takes/returns plain JSON (see types.ts);
//   - the UI never mutates what it receives; changes come back as events;
//   - large lists (messages) are read in ranges, never all at once.

import type {
  AppInfo,
  Channel,
  CreatedInvite,
  Id,
  Identity,
  Invite,
  JoinStatus,
  Member,
  Message,
  MessageRange,
  NewIdentityInput,
  ProfileInput,
  RecoveryCheckResult,
  AgeRestriction,
  ServerSummary,
  StorageUsage,
  UpdateChannel,
  UpdateInfo,
  CoreStatus,
  CallState,
  CallSignal,
  VoiceRoom,
} from './types';

export interface CoreCommands {
  // Identity
  getIdentity(): Promise<Identity | null>;
  /** Emits `identity.progress` while the account is prepared. */
  createIdentity(input: NewIdentityInput): Promise<{ identity: Identity; words: string[] }>;
  /**
   * The current word check: 3 positions (1-based) the core picked. Calling it
   * again returns the same challenge until it is passed or burned. After 5
   * failed checks it rejects with `too-many-attempts` until
   * `getRecoveryWords()` succeeds again (B19 spec §6).
   */
  getRecoveryChallenge(): Promise<{ positions: number[] }>;
  /**
   * Answers for exactly the current challenge's positions (otherwise rejects
   * with `not-allowed`). Returns the wrong positions; on success the account
   * is marked checked. The 5th failed check burns the challenge and itself
   * rejects with `too-many-attempts`, as does every check after it.
   */
  checkRecoveryWords(answers: Record<number, string>): Promise<RecoveryCheckResult>;
  /**
   * The words again (onboarding resume, "show the words again"). The real core
   * asks for OS authentication first and may reject with `auth-cancelled`.
   * Success also lifts a burned challenge.
   */
  getRecoveryWords(): Promise<string[]>;
  /**
   * The account from its 12 words. `input` (#290) carries what the restore asks
   * again, as for a new account (spec: age, restore asks again): language, country,
   * birth date. The words alone still work.
   */
  restoreIdentity(words: string[], input?: NewIdentityInput): Promise<Identity>;
  updateProfile(input: ProfileInput): Promise<Identity>;
  /**
   * #304: removes this identity from this computer (the account stays on the
   * network). Proof: this identity's 12 words, or `withoutWords` to accept
   * losing it. Rejects invalid-words (with positions), not-allowed (another
   * account's words), keyring-unavailable (nothing changed), bad-request,
   * not-found. On desktop, withoutWords first asks in a native dialog; declining
   * gives cancelled. On ok the core exits and the shell restarts it, and the
   * shell clears the page's storage and its log.
   */
  signOut(words?: string[], withoutWords?: boolean): Promise<null>;

  // Servers and channels
  listServers(): Promise<ServerSummary[]>;
  listChannels(serverId: Id): Promise<Channel[]>;
  /** Owner only (B11c). The core normalises the name; rejects `name-taken` or `bad-request`. */
  createChannel(serverId: Id, input: { name: string; adult: boolean }): Promise<Channel>;
  /** Owner only. Same name rules as createChannel. */
  updateChannel(channelId: Id, input: { name?: string; adult?: boolean }): Promise<Channel>;
  /**
   * G-11 (#323), owner or admin: puts the channel right before `before`, right
   * after `after`, or last. Returns the server's channels in the new order.
   * Rejects not-allowed, not-found, bad-request.
   */
  moveChannel(serverId: Id, channelId: Id, before?: Id, after?: Id): Promise<Channel[]>;
  /** Owner only; `not-allowed` for the default channel. */
  deleteChannel(channelId: Id): Promise<void>;
  listMembers(serverId: Id): Promise<Member[]>;
  createServer(input: { name: string; ageRestriction: AgeRestriction }): Promise<ServerSummary>;
  /** Progress arrives as `join.status` events; resolves with the first status. */
  joinServer(link: string): Promise<JoinStatus>;
  cancelJoin(link: string): Promise<void>;
  /**
   * Server lifecycle (#261). Rename and close answer `update-needed` while a
   * member's MVS is too old (`getUpdateNeeded` says who), and sign nothing then. Rename: owner or admin; the name is
   * trimmed, 1–100 characters, no bidi or control characters (`bad-request`).
   */
  renameServer(serverId: Id, name: string): Promise<ServerSummary>;
  /** Any member but the owner; never `update-needed`. Then `server.removed { reason: 'left' }`. */
  leaveServer(serverId: Id): Promise<void>;
  /** Owner only, for good. Then `server.closed`; 30 days later `server.removed { reason: 'closed' }`. */
  closeServer(serverId: Id): Promise<void>;
  /** Who blocks rename and close: members whose MVS is too old. Empty when the gate is open. */
  getUpdateNeeded(serverId: Id): Promise<Id[]>;
  /**
   * Invites (#214), moderator and up (`not-allowed` below). `expiresMs` is the last
   * moment a join is accepted (ms since epoch; absent = never, `bad-request` if
   * not in the future); `maxUses` absent or 0 = unlimited. Fires `server.updated`.
   */
  createInvite(serverId: Id, input?: { expiresMs?: number; maxUses?: number }): Promise<CreatedInvite>;
  /** Oldest first, revoked ones included. */
  listInvites(serverId: Id): Promise<Invite[]>;
  /** The invite's author or someone ranked above; `not-found` if unknown or already revoked. Fires `server.updated`. */
  revokeInvite(serverId: Id, inviteId: Id): Promise<void>;

  // Messages
  getMessageCount(channelId: Id): Promise<number>;
  getMessages(channelId: Id, start: number, end: number): Promise<MessageRange>;
  sendMessage(channelId: Id, body: string, replyToId?: Id): Promise<Message>;
  editMessage(messageId: Id, body: string): Promise<void>;
  deleteMessage(messageId: Id): Promise<void>;
  retryMessage(messageId: Id): Promise<void>;
  /**
   * #288: drops a held or failed `pending-…` message and wipes its text. No
   * event: the caller removes it. `not-found` for any other id. "Send again" is
   * this plus a new sendMessage.
   */
  discardMessage(messageId: Id): Promise<null>;
  markRead(channelId: Id): Promise<void>;

  // Safety
  blockUser(userId: Id): Promise<void>;
  kickMember(serverId: Id, userId: Id): Promise<void>;
  // Voice v1 (#223)
  /** A server's voice room and who is in it. Rejects not-found, not-allowed (not a member, kicked). */
  getVoiceRoom(serverId: Id): Promise<VoiceRoom>;
  /**
   * Joins a voice room (leaving any other call first); `channelId` is the room's
   * id. Rejects not-found, not-allowed (a text channel), call-full, relay-only, in-call-elsewhere (#294: I'm already in it from another computer), not-implemented.
   */
  joinCall(channelId: Id): Promise<CallState>;
  /** Idempotent. */
  leaveCall(): Promise<null>;
  /** `not-allowed` when not in a call. */
  setMuted(muted: boolean): Promise<null>;
  /** For the voice engine only (ui/src/voice). Payload at most 12 KiB. */
  sendSignal(to: Id, kind: CallSignal['kind'], payload: string): Promise<null>;

  // App
  getStorageUsage(): Promise<StorageUsage>;
  setStorageLimit(bytes: number): Promise<void>;
  clearCache(): Promise<void>;
  /** A JSON text the alpha testers attach to bug reports. */
  exportDiagnostics(): Promise<string>;
  getAppInfo(): Promise<AppInfo>;
  setUpdateChannel(channel: UpdateChannel): Promise<void>;
  /**
   * Relay-only (GAP-6r, #272), per device: no direct connections, so members
   * never see my IP; calls are off. On ends my call (a `call.state` with no
   * participants and `relayOnly`), and `joinCall` answers `relay-only` until off.
   * Shown up front as `AppInfo.relayOnly` and `VoiceRoom.relayOnly`.
   */
  setRelayOnly(on: boolean): Promise<void>;
  restartToUpdate(): Promise<void>;
}

export interface CoreEvents {
  'identity.progress': { step: 'keys' | 'storage' | 'encryption' | 'done'; percent: number };
  'identity.changed': Identity;
  'server.added': ServerSummary;
  'server.updated': ServerSummary;
  /** `reason` is absent for a kick. */
  'server.removed': { serverId: Id; reason?: 'left' | 'closed' };
  /** The owner closed it: it stays listed, read-only (`ServerSummary.closed`). */
  'server.closed': { serverId: Id };
  'channel.updated': Channel;
  'channel.added': Channel;
  'channel.removed': { serverId: Id; channelId: Id; name: string };
  'members.changed': { serverId: Id; members: Member[] };
  'message.added': Message;
  'message.updated': Message;
  'join.status': JoinStatus;
  'update.ready': UpdateInfo;
  /** Once, after a new version committed. `notes` is plain text: render as text only. */
  'update.installed': { version: string; notes: string | null };
  /** Once, after the launcher went back to the previous version. */
  'update.rolledBack': { failed: string; running: string };
  /** e.g. after a kick: the server's encryption was renewed. */
  /** rekey-waiting: a message held for its key failed; sending there waits for a moderator's re-key. */
  'notice': { kind: 'encryption-renewed' | 'rekey-waiting'; serverId: Id };
  /**
   * Desktop app only: an mvs:// invite link the OS handed to the app (a click
   * in a browser or chat). The UI opens the join dialog with it and joins only
   * when the person presses Join; nothing joins on its own.
   */
  'link.open': { link: string };
  /** Desktop shell (T8-01): sent on every change and on every page load. */
  'core.status': CoreStatus;
  /** Desktop shell (G-61): a desktop notification was clicked; open its channel. */
  'notify.click': { channelId: Id; messageId: Id | null };
  /** Every change to my call; participants [] after leaving. */
  'call.state': CallState;
  'call.signal': CallSignal;
  /** A voice room's roster changed (join, leave, mute, timeout), mine or not. A hint: re-read with getVoiceRoom. */
  'voice.room': VoiceRoom;
}

export type CoreEventName = keyof CoreEvents;
export type Unsubscribe = () => void;

export interface CoreApi extends CoreCommands {
  on<E extends CoreEventName>(event: E, handler: (payload: CoreEvents[E]) => void): Unsubscribe;
}
