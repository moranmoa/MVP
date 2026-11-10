// peerFailed: who in the call I have no voice connection with.

import { describe, expect, it } from 'vitest';
import type { Id } from '../core/types';
import { nextPeerFailed } from './useVoiceEngine';

const A: Id = 'a'.repeat(64);
const B: Id = 'b'.repeat(64);
const C: Id = 'c'.repeat(64);

describe('nextPeerFailed', () => {
  it('adds a peer on a per-peer error, once; ignores mic errors', () => {
    let s = nextPeerFailed([], { error: { code: 'peer-failed', peer: B } });
    expect(s).toEqual([B]);
    s = nextPeerFailed(s, { error: { code: 'fingerprint-mismatch', peer: C } });
    s = nextPeerFailed(s, { error: { code: 'peer-failed', peer: B } });
    expect(s).toEqual([B, C]);
    expect(nextPeerFailed(s, { error: { code: 'mic-denied' } })).toBe(s);
  });

  it('clears a peer that connects again, keeps the others', () => {
    const s = nextPeerFailed([B, C], {
      participants: [
        { identity: A, state: 'connected' },
        { identity: B, state: 'connected' },
      ],
    });
    expect(s).toEqual([C]);
    const same = nextPeerFailed(s, { participants: [{ identity: A, state: 'connected' }, { identity: C, state: 'connecting' }] });
    expect(same).toBe(s); // unchanged: no re-render
  });

  it('clears everything when the call ends', () => {
    expect(nextPeerFailed([B, C], { participants: [] })).toEqual([]);
  });
});
