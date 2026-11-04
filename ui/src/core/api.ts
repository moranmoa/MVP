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
  Id,
  Identity,
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
  restoreIdentity(words: string[]): Promise<Identity>;
  updateProfile(input: ProfileInput): Promise<Identity>;

  // Servers and channels
  listServers(): Promise<ServerSummary[]>;
  listChannels(serverId: Id): Promise<Channel[]>;
  listMembers(serverId: Id): Promise<Member[]>;
  createServer(input: { name: string; ageRestriction: AgeRestriction }): Promise<ServerSummary>;
  /** Progress arrives as `join.status` events; resolves with the first status. */
  joinServer(link: string): Promise<JoinStatus>;
  cancelJoin(link: string): Promise<void>;

  // Messages
  getMessageCount(channelId: Id): Promise<number>;
  getMessages(channelId: Id, start: number, end: number): Promise<MessageRange>;
  sendMessage(channelId: Id, body: string, replyToId?: Id): Promise<Message>;
  editMessage(messageId: Id, body: string): Promise<void>;
  deleteMessage(messageId: Id): Promise<void>;
  retryMessage(messageId: Id): Promise<void>;
  markRead(channelId: Id): Promise<void>;

  // Safety
  blockUser(userId: Id): Promise<void>;
  kickMember(serverId: Id, userId: Id): Promise<void>;

  // App
  getStorageUsage(): Promise<StorageUsage>;
  setStorageLimit(bytes: number): Promise<void>;
  clearCache(): Promise<void>;
  /** A JSON text the alpha testers attach to bug reports. */
  exportDiagnostics(): Promise<string>;
  getAppInfo(): Promise<AppInfo>;
  setUpdateChannel(channel: UpdateChannel): Promise<void>;
  restartToUpdate(): Promise<void>;
}

export interface CoreEvents {
  'identity.progress': { step: 'keys' | 'storage' | 'encryption' | 'done'; percent: number };
  'identity.changed': Identity;
  'server.added': ServerSummary;
  'server.updated': ServerSummary;
  'server.removed': { serverId: Id };
  'channel.updated': Channel;
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
  'notice': { kind: 'encryption-renewed'; serverId: Id };
  /**
   * Desktop app only: an mvs:// invite link the OS handed to the app (a click
   * in a browser or chat). The UI opens the join dialog with it and joins only
   * when the person presses Join; nothing joins on its own.
   */
  'link.open': { link: string };
}

export type CoreEventName = keyof CoreEvents;
export type Unsubscribe = () => void;

export interface CoreApi extends CoreCommands {
  on<E extends CoreEventName>(event: E, handler: (payload: CoreEvents[E]) => void): Unsubscribe;
}
