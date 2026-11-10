// peerFailed: who in the call I have no voice connection with, and why.

import { describe, expect, it } from 'vitest';
import type { Id } from '../core/types';
import { nextPeerFailed } from './useVoiceEngine';

const A: Id = 'a'.repeat(64);
const B: Id = 'b'.repeat(64);
const C: Id = 'c'.repeat(64);

describe('nextPeerFailed', () => {
  it('gives the reason: failed for peer-failed, refused for a security check', () => {
    let s = nextPeerFailed({}, { error: { code: 'peer-failed', peer: B } });
    expect(s).toEqual({ [B]: 'failed' });
    s = nextPeerFailed(s, { error: { code: 'fingerprint-mismatch', peer: C } });
    expect(s).toEqual({ [B]: 'failed', [C]: 'refused' });
    expect(nextPeerFailed({}, { error: { code: 'unverified-signal', peer: C } })).toEqual({ [C]: 'refused' });
  });

  it('a failure turns into a refusal, never the other way; mic errors are ignored', () => {
    const failed = { [B]: 'failed' as const };
    expect(nextPeerFailed(failed, { error: { code: 'unverified-signal', peer: B } })).toEqual({ [B]: 'refused' });
    const refused = { [B]: 'refused' as const };
    expect(nextPeerFailed(refused, { error: { code: 'peer-failed', peer: B } })).toBe(refused);
    expect(nextPeerFailed(failed, { error: { code: 'peer-failed', peer: B } })).toBe(failed);
    expect(nextPeerFailed(failed, { error: { code: 'mic-denied' } })).toBe(failed);
  });

  it('clears a peer that connects again, keeps the others', () => {
    const s = nextPeerFailed(
      { [B]: 'failed', [C]: 'refused' },
      {
        participants: [
          { identity: A, state: 'connected' },
          { identity: B, state: 'connected' },
        ],
      },
    );
    expect(s).toEqual({ [C]: 'refused' });
    const same = nextPeerFailed(s, { participants: [{ identity: A, state: 'connected' }, { identity: C, state: 'connecting' }] });
    expect(same).toBe(s); // unchanged: no re-render
  });

  it('clears everything when the call ends', () => {
    expect(nextPeerFailed({ [B]: 'failed', [C]: 'refused' }, { participants: [] })).toEqual({});
  });
});
