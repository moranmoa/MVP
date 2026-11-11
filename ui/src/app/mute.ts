// G-03 / G-17 (docs/ux/GAPS.md §2.4): mute a server or a channel, on this
// device only. Nothing is sent to anyone. Notifications (G-61) read the same
// state: a message is quiet when `isMuted(mutes, serverId, channelId)`.

import type { Id } from '../core';

/** `server:<id>` or `channel:<id>`. */
export type MuteKey = string;
/** Until when, in ms since the epoch, or MUTE_FOREVER ("until I turn it back on"). */
export type Mutes = Record<MuteKey, number>;

export const MUTE_FOREVER = 0;
const HOUR = 3_600_000;
export const MUTE_DURATIONS = { '1h': HOUR, '8h': 8 * HOUR, '24h': 24 * HOUR, forever: MUTE_FOREVER } as const;
export type MuteDuration = keyof typeof MUTE_DURATIONS;

export const serverMuteKey = (serverId: Id): MuteKey => `server:${serverId}`;
export const channelMuteKey = (channelId: Id): MuteKey => `channel:${channelId}`;

const STORAGE_KEY = 'mvs.mute.v1';

export function isMutedKey(mutes: Mutes, key: MuteKey, now = Date.now()): boolean {
  const until = mutes[key];
  return until !== undefined && (until === MUTE_FOREVER || until > now);
}

/** A server's mute covers its channels; a channel can also be muted on its own. */
export function isMuted(mutes: Mutes, serverId: Id, channelId?: Id, now = Date.now()): boolean {
  return isMutedKey(mutes, serverMuteKey(serverId), now) || (!!channelId && isMutedKey(mutes, channelMuteKey(channelId), now));
}

/** Until when a key is muted (MUTE_FOREVER for good), or null when it isn't. */
export function mutedUntil(mutes: Mutes, key: MuteKey, now = Date.now()): number | null {
  return isMutedKey(mutes, key, now) ? mutes[key] : null;
}

/** Drops expired entries. */
export function liveMutes(mutes: Mutes, now = Date.now()): Mutes {
  return Object.fromEntries(Object.entries(mutes).filter(([key]) => isMutedKey(mutes, key, now)));
}

/** The next moment a mute ends, or null. */
export function nextExpiry(mutes: Mutes): number | null {
  const ends = Object.values(mutes).filter((u) => u !== MUTE_FOREVER);
  return ends.length ? Math.min(...ends) : null;
}

export function loadMutes(): Mutes {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
    if (!raw || typeof raw !== 'object') return {};
    const out: Mutes = {};
    for (const [key, until] of Object.entries(raw as Record<string, unknown>)) {
      if (/^(server|channel):[\w-]{1,128}$/.test(key) && typeof until === 'number' && Number.isFinite(until) && until >= 0) out[key] = until;
    }
    return liveMutes(out);
  } catch {
    return {};
  }
}

export function saveMutes(mutes: Mutes): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(mutes));
  } catch {
    // Private mode or full storage: the mute lasts this session.
  }
}
