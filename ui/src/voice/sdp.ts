// SDP helpers for the voice engine: the DTLS fingerprint check and the STUN
// setting. Pure functions, so they are tested on their own.

/** `sha-256 AB:CD:…` → `sha-256 ab:cd:…`; null when it isn't one. */
export function normalizeFingerprint(algorithm: string, value: string): string | null {
  const alg = algorithm.trim().toLowerCase();
  const hex = value.trim().toLowerCase();
  if (!/^sha-(1|224|256|384|512)$/.test(alg)) return null;
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
export function iceServersFor(stunUrl: string | null | undefined): RTCIceServer[] {
  if (typeof stunUrl !== 'string') return [];
  const url = stunUrl.trim();
  if (!/^stuns?:[^\s@/?#]+$/i.test(url)) return [];
  return [{ urls: url }];
}
