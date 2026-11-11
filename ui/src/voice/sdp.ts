// SDP helpers for the voice engine: the DTLS fingerprint check and the STUN
// setting. Pure functions, so they are tested on their own.

/** `sha-256 AB:CD:…` → `sha-256 ab:cd:…`; null when it isn't one. */
export function normalizeFingerprint(algorithm: string, value: string): string | null {
  const alg = algorithm.trim().toLowerCase();
  const hex = value.trim().toLowerCase();
  // sha-256 or stronger: the signed binding is only as strong as the hash.
  if (!/^sha-(256|384|512)$/.test(alg)) return null;
  if (!/^[0-9a-f]{2}(:[0-9a-f]{2})+$/.test(hex)) return null;
  return `${alg} ${hex}`;
}

/** Every `a=fingerprint:` line of an SDP, normalised; unparseable ones as ''. */
export function fingerprints(sdp: string): string[] {
  const out: string[] = [];
  for (const raw of sdp.split('\n')) {
    const line = raw.replace(/\r$/, '');
    if (!line.startsWith('a=fingerprint:')) continue;
    const [alg = '', value = '', ...rest] = line.slice('a=fingerprint:'.length).trim().split(/\s+/);
    out.push(rest.length ? '' : (normalizeFingerprint(alg, value) ?? ''));
  }
  return out;
}

/**
 * The one fingerprint an SDP commits to, or null when it has none, an
 * unparseable one, or two that differ (a session and a media section could
 * otherwise name different certificates).
 */
export function soleFingerprint(sdp: string): string | null {
  const all = fingerprints(sdp);
  if (!all.length || all.some((f) => !f || f !== all[0])) return null;
  return all[0]!;
}

/**
 * ICE servers from the config's optional STUN URL. Only a plain `stun:` or
 * `stuns:` URL is accepted; anything else (TURN, credentials, junk) gives no
 * server at all. There is no public default: without one, only host
 * candidates are gathered and calls work on the same network.
 */
export function iceServersFor(stunUrl: string | null | undefined, turn?: TurnCredentials | null): RTCIceServer[] {
  const url = cleanStunUrl(stunUrl);
  const servers: RTCIceServer[] = url ? [{ urls: url }] : [];
  if (turn) servers.push({ urls: [...turn.urls], username: turn.username, credential: turn.credential });
  return servers;
}

/** TURN credentials from the core (`getTurnCredentials`, G-81), once cleaned. */
export interface TurnCredentials {
  urls: string[];
  username: string;
  credential: string;
  /** Seconds the credentials are valid for. */
  ttl: number;
  /** When they stop working, ms since the epoch. */
  expiresAt: number;
}

/** TURN ports we accept (docs/design/voice-across-networks.md). */
export const TURN_PORTS = [3478, 5349, 443];

/**
 * TURN credentials, checked (G-81, Security Lead's conditions on #333); null
 * when anything is off, and the call then goes on without TURN.
 * - every URL is `turn:`/`turns:` on exactly `host` (the configured relay
 *   host), an allowed port, and at most `?transport=udp|tcp`;
 * - the username is `<expiry>:<random>` (never a peer ID), the credential a
 *   plain token, each at most 256 characters;
 * - ttl 60..86 400 s, and expiresAt in the future, within ttl of now.
 */
export function cleanTurnCredentials(raw: unknown, host: string | null | undefined, now = Date.now()): TurnCredentials | null {
  if (!raw || typeof raw !== 'object' || typeof host !== 'string' || !host) return null;
  const c = raw as Record<string, unknown>;
  const want = host.toLowerCase();
  if (!Array.isArray(c.urls) || c.urls.length < 1 || c.urls.length > 4) return null;
  const urls: string[] = [];
  for (const u of c.urls) {
    if (typeof u !== 'string' || u.length > 256) return null;
    const m = /^(turns?):([A-Za-z0-9.-]{1,253}):(\d{1,5})(\?transport=(udp|tcp))?$/.exec(u);
    if (!m || m[2]!.toLowerCase() !== want || !TURN_PORTS.includes(Number(m[3]))) return null;
    urls.push(u);
  }
  const { username, credential, ttl, expiresAt } = c;
  if (typeof username !== 'string' || username.length > 256 || !/^\d{9,12}:[A-Za-z0-9_-]{8,128}$/.test(username)) return null;
  // The random part must not look like an identity (64-hex member id) or a
  // libp2p peer id (12D3Koo…, Qm…): the relay must never name the user (N8b).
  const random = username.slice(username.indexOf(':') + 1);
  if (/^[0-9a-f]{64}$/i.test(random) || /^(12D3Koo|Qm)[1-9A-HJ-NP-Za-km-z]{40,}$/.test(random)) return null;
  if (typeof credential !== 'string' || !credential || credential.length > 256 || !/^[A-Za-z0-9+/=_-]+$/.test(credential)) return null;
  if (typeof ttl !== 'number' || !Number.isInteger(ttl) || ttl < 60 || ttl > 86_400) return null;
  if (typeof expiresAt !== 'number' || expiresAt <= now || expiresAt > now + ttl * 1000 + 60_000) return null;
  return { urls, username, credential, ttl, expiresAt };
}

/**
 * The STUN URL if it is a plain `stun:`/`stuns:` one (trimmed, at most 256
 * characters), else null. The desktop shell checks the config with the same
 * rule (desktop/src/config.js validStun).
 */
export function cleanStunUrl(stunUrl: unknown): string | null {
  if (typeof stunUrl !== 'string') return null;
  const url = stunUrl.trim();
  return url.length <= 256 && /^stuns?:[^\s@/?#]+$/i.test(url) ? url : null;
}
