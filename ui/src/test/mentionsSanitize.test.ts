// G-24 (#330): Message.mentions reaches the UI only as a list of member ids.

import { describe, expect, it } from 'vitest';
import { cleanMessage } from '../core/sanitize';
import type { Message } from '../core/types';

const id = (c: string) => c.repeat(64);
const msg = (over: Partial<Message>): Message => ({ id: 'm', channelId: 'c', index: 0, authorId: 'a', body: 'hi', createdAt: 0, ...over });

describe('mentions in the sanitizer (G-24)', () => {
  it('keeps 64-hex ids, once each, at most 100', () => {
    expect(cleanMessage(msg({ mentions: [id('a'), id('b'), id('a')] })).mentions).toEqual([id('a'), id('b')]);
    const many = Array.from({ length: 150 }, (_, i) => i.toString(16).padStart(64, '0'));
    expect(cleanMessage(msg({ mentions: many })).mentions).toHaveLength(100);
  });

  it('drops anything that is not a member id, and a list that is not a list', () => {
    const bad = ['Dana', id('A'), 'a'.repeat(63), '<script>', 7 as unknown as string];
    expect('mentions' in cleanMessage(msg({ mentions: bad }))).toBe(false);
    expect(cleanMessage(msg({ mentions: [...bad, id('c')] })).mentions).toEqual([id('c')]);
    expect('mentions' in cleanMessage(msg({ mentions: 'abc' as unknown as string[] }))).toBe(false);
  });

  it('mentionsMe stays a strict boolean', () => {
    expect(cleanMessage(msg({ mentionsMe: 'yes' as unknown as boolean })).mentionsMe).toBe(false);
    expect(cleanMessage(msg({ mentionsMe: true })).mentionsMe).toBe(true);
  });
});
