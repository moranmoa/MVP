import { describe, expect, it } from 'vitest';
import type { Message } from '../core';
import { MockCore } from '../core/mock';
import { ageOn, toBirthDate } from '../app/age';

const tick = () => new Promise((r) => setTimeout(r, 0));

describe('mock core', () => {
  it('serves a 100k-message channel in ranges', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    expect(await core.getMessageCount('c-general')).toBe(100_000);
    const r = await core.getMessages('c-general', 99_950, 100_000);
    expect(r.messages).toHaveLength(50);
    expect(r.messages[0].index).toBe(99_950);
    // Generated content is deterministic.
    const again = await core.getMessages('c-general', 99_950, 99_951);
    expect(again.messages[0]).toEqual(r.messages[0]);
  });

  it('walks a sent message through pending → sent → received', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    const states: string[] = [];
    core.on('message.added', (m) => states.push(m.sendState!));
    core.on('message.updated', (m) => states.push(m.sendState!));
    await core.sendMessage('c-general', 'hello');
    for (let i = 0; i < 5; i++) await tick();
    expect(states).toEqual(['pending', 'sent', 'received']);
  });

  it('keeps messages pending when nobody is online, and fails/retries on demand', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    const last = new Map<string, Message>();
    core.on('message.added', (m) => last.set(m.id, m));
    core.on('message.updated', (m) => last.set(m.id, m));

    const pending = await core.sendMessage('c-fr-general', 'anyone?');
    for (let i = 0; i < 5; i++) await tick();
    expect(last.get(pending.id)!.sendState).toBe('pending');

    const failing = await core.sendMessage('c-general', 'oops /fail');
    for (let i = 0; i < 5; i++) await tick();
    expect(last.get(failing.id)!.sendState).toBe('failed');
    await core.retryMessage(failing.id);
    for (let i = 0; i < 5; i++) await tick();
    expect(last.get(failing.id)!.sendState).toBe('received');
  });

  it('edits and deletes as tombstones without shifting indices', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    const m = await core.sendMessage('c-lfg', 'first');
    await core.editMessage(m.id, 'first!');
    let r = await core.getMessages('c-lfg', m.index, m.index + 1);
    expect(r.messages[0].body).toBe('first!');
    expect(r.messages[0].editedAt).toBeDefined();
    await core.deleteMessage(m.id);
    r = await core.getMessages('c-lfg', m.index, m.index + 1);
    expect(r.messages[0].deleted).toBe(true);
    expect(await core.getMessageCount('c-lfg')).toBe(m.index + 1);
  });

  it('requires the recovery check before creating a server', async () => {
    const core = new MockCore({ speed: 0 });
    const { words } = await core.createIdentity({ locale: 'he', country: 'IL', birthDate: '2000-01-01' });
    await expect(core.createServer({ name: 'x', ageRestriction: 'none' })).rejects.toMatchObject({
      code: 'recovery-check-required',
    });
    const { positions } = await core.getRecoveryChallenge();
    const answer = (wrongAt?: number) => Object.fromEntries(positions.map((p) => [p, p === wrongAt ? 'wrong' : words[p - 1]]));
    expect(await core.checkRecoveryWords(answer(positions[0]))).toEqual({ ok: false, wrong: [positions[0]] });
    expect(await core.checkRecoveryWords(answer())).toEqual({ ok: true });
    const s = await core.createServer({ name: 'שרת', ageRestriction: '18+' });
    expect(s.ageRestriction).toBe('18+');
    expect(await core.listChannels(s.id)).toHaveLength(1);
  });

  it('rejects unknown recovery words with their positions', async () => {
    const core = new MockCore({ speed: 0 });
    const words = Array(12).fill('apple');
    words[3] = 'notaword';
    await expect(core.restoreIdentity(words)).rejects.toMatchObject({ code: 'invalid-words', positions: [4] });
  });

  it('kicking a member renews encryption', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    const notices: unknown[] = [];
    core.on('notice', (n) => notices.push(n));
    const members = await core.listMembers('s-gamers');
    const target = members.find((m) => !m.isOwner)!;
    await core.kickMember('s-gamers', target.id);
    expect((await core.listMembers('s-gamers')).some((m) => m.id === target.id)).toBe(false);
    expect(notices).toEqual([{ kind: 'encryption-renewed', serverId: 's-gamers' }]);
  });
});

describe('birth date', () => {
  const now = new Date(2026, 10, 3);
  it('validates real dates only', () => {
    expect(toBirthDate('31', '2', '2000', now)).toBeNull();
    expect(toBirthDate('29', '2', '2004', now)).toBe('2004-02-29');
    expect(toBirthDate('1', '1', '2030', now)).toBeNull();
  });
  it('computes age on a given day', () => {
    expect(ageOn('2010-11-03', now)).toBe(16);
    expect(ageOn('2010-11-04', now)).toBe(15);
  });
});
