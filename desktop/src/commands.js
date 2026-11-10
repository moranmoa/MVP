// The allow-list of core commands the renderer may call, with the names of
// their parameters in order. It mirrors `CoreCommands` in ui/src/core/api.ts
// (test/commands.test.js keeps the two in step) and the argument names the
// sidecar expects (core/api dispatch: the TS parameter names).
//
// Commands the core has but the UI does not (grantRole, revokeRole, syncNow)
// are not here, so the renderer cannot reach them.

'use strict';

const COMMANDS = Object.freeze({
  // Identity
  getIdentity: [],
  createIdentity: ['input'],
  getRecoveryChallenge: [],
  checkRecoveryWords: ['answers'],
  getRecoveryWords: [],
  restoreIdentity: ['words'],
  updateProfile: ['input'],
  // Servers and channels
  listServers: [],
  listChannels: ['serverId'],
  createChannel: ['serverId', 'input'],
  updateChannel: ['channelId', 'input'],
  deleteChannel: ['channelId'],
  listMembers: ['serverId'],
  createServer: ['input'],
  joinServer: ['link'],
  cancelJoin: ['link'],
  renameServer: ['serverId', 'name'],
  leaveServer: ['serverId'],
  closeServer: ['serverId'],
  getUpdateNeeded: ['serverId'],
  createInvite: ['serverId', 'input'],
  listInvites: ['serverId'],
  revokeInvite: ['serverId', 'inviteId'],
  // Messages
  getMessageCount: ['channelId'],
  getMessages: ['channelId', 'start', 'end'],
  sendMessage: ['channelId', 'body', 'replyToId'],
  editMessage: ['messageId', 'body'],
  deleteMessage: ['messageId'],
  retryMessage: ['messageId'],
  discardMessage: ['messageId'],
  markRead: ['channelId'],
  // Safety
  blockUser: ['userId'],
  kickMember: ['serverId', 'userId'],
  // Voice v1 (#223)
  getVoiceRoom: ['serverId'],
  joinCall: ['channelId'],
  leaveCall: [],
  setMuted: ['muted'],
  sendSignal: ['to', 'kind', 'payload'],
  // App
  getStorageUsage: [],
  setStorageLimit: ['bytes'],
  clearCache: [],
  exportDiagnostics: [],
  getAppInfo: [],
  setUpdateChannel: ['channel'],
  setRelayOnly: ['on'],
  restartToUpdate: [],
});

/** Requests the renderer may send: 1 MiB, the sidecar's own line limit. */
const MAX_REQUEST_BYTES = 1024 * 1024;

/**
 * Checks a renderer request and turns positional args into the sidecar's
 * named args. Returns { cmd, args } or { error }.
 */
function toRequest(cmd, args) {
  if (typeof cmd !== 'string' || !Object.prototype.hasOwnProperty.call(COMMANDS, cmd)) return { error: 'not-allowed' };
  if (!Array.isArray(args)) return { error: 'bad-request' };
  const names = COMMANDS[cmd];
  if (args.length > names.length) return { error: 'bad-request' };
  const named = {};
  names.forEach((name, i) => {
    if (args[i] !== undefined) named[name] = args[i];
  });
  let size;
  try {
    size = Buffer.byteLength(JSON.stringify(named));
  } catch {
    return { error: 'bad-request' };
  }
  if (size > MAX_REQUEST_BYTES) return { error: 'too-large' };
  return { cmd, args: named };
}

module.exports = { COMMANDS, MAX_REQUEST_BYTES, toRequest };
