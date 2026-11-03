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
  /** Positions are 1-based. Resolves true when all answers match. */
  checkRecoveryWords(answers: Record<number, string>): Promise<boolean>;
  /** Returns the words again, for the "show my words" path in settings. */
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
  /** e.g. after a kick: the server's encryption was renewed. */
  'notice': { kind: 'encryption-renewed'; serverId: Id };
}

export type CoreEventName = keyof CoreEvents;
export type Unsubscribe = () => void;

export interface CoreApi extends CoreCommands {
  on<E extends CoreEventName>(event: E, handler: (payload: CoreEvents[E]) => void): Unsubscribe;
}
