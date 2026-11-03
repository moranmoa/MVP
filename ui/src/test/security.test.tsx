// Security checks for everything other people can put on screen, and for the
// recovery words. See SECURITY.md for the threat list these tests cover.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import type { CoreApi } from '../core/api';
import { MockCore } from '../core/mock';
import {
  BODY_MAX,
  cleanExcerpt,
  cleanLink,
  cleanMember,
  cleanMessage,
  cleanName,
  cleanServer,
  cleanText,
  guardCore,
  nameSkeleton,
  safeColor,
  safeImageSrc,
} from '../core/sanitize';
import type { Member, Message, ServerSummary } from '../core/types';
import { lookalikeIds } from '../screens/shell/MemberList';
import { CSP, CSP_DIRECTIVES } from '../../csp';

const RLO = '\u202E';
const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

const member = (over: Partial<Member>): Member => ({
  id: 'u-x',
  serverId: 's',
  displayName: 'Dana',
  shortCode: 'K7QM-2XPA',
  avatarColor: '#5865f2',
  presence: 'online',
  isOwner: false,
  ...over,
});

const message = (over: Partial<Message>): Message => ({
  id: 'm',
  channelId: 'c',
  index: 0,
  authorId: 'u-x',
  body: 'hi',
  createdAt: 0,
  ...over,
});

/**
 * A demo core whose oldest messages in the open channel carry `bodies`, as if
 * other people had written them. The list starts at the top in jsdom (it
 * cannot scroll), so these are the rows that render.
 */
function coreWithHistory(bodies: string[]): MockCore {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  const base = core.getMessages.bind(core);
  core.getMessages = async (channelId, start, end) => {
    const r = await base(channelId, start, end);
    return {
      ...r,
      messages: r.messages.map((m) => (m.index < bodies.length ? { ...m, body: bodies[m.index], deleted: false, replyTo: undefined } : m)),
    };
  };
  return core;
}

function srcFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? (f === 'test' ? [] : srcFiles(p)) : /\.(ts|tsx)$/.test(f) ? [p] : [];
  });
}
const SRC = join(__dirname, '..');

describe('names from other people', () => {
  it('drops direction overrides and isolates, so a name cannot reverse itself or its surroundings', () => {
    expect(cleanName(`admin${RLO}txt.exe`)).toBe('admintxt.exe');
    expect(cleanName('\u2067Dana\u2069\u200F')).toBe('Dana');
  });

  it('drops invisible and blank characters, and names that are only invisible', () => {
    expect(cleanName('Da\u200Bna\u00AD')).toBe('Dana');
    expect(cleanName(String.fromCharCode(0x3164, 0x3164))).toBe(''); // Hangul fillers
    expect(cleanName(String.fromCharCode(0x2800))).toBe(''); // braille blank
    expect(cleanName('Dana\u{E0041}')).toBe('Dana');
  });

  it('keeps emoji sequences but not stray joiners', () => {
    expect(cleanName('Dev 👩\u200D💻')).toBe('Dev 👩\u200D💻');
    expect(cleanName('Da\u200Dna')).toBe('Dana');
  });

  it('caps stacked combining marks ("zalgo") but keeps Hebrew vowels', () => {
    const zalgo = 'a' + '\u0301'.repeat(200);
    expect([...cleanName(zalgo)].length).toBe(5);
    expect(cleanName('ש\u05B8\u05C1לו\u05B9ם')).toBe('ש\u05B8\u05C1לו\u05B9ם'.normalize('NFKC'));
  });

  it('is one line, at most 32 characters, and costs nothing for a megabyte name', () => {
    expect(cleanName('a\nb\tc')).toBe('a b c');
    expect([...cleanName('x'.repeat(1_000_000))].length).toBe(32);
    const t = performance.now();
    cleanName('\u0301'.repeat(5_000_000));
    expect(performance.now() - t).toBeLessThan(200);
  });

  it('folds look-alike letters (fullwidth, math bold) and removes fake identity codes', () => {
    expect(cleanName('Ｍｏｒａｎ')).toBe('Moran');
    expect(cleanName('𝐀𝐝𝐦𝐢𝐧')).toBe('Admin');
    expect(cleanName('Moran K7QM-2XPA')).toBe('Moran');
  });

  it('falls back to the identity code when the name is empty after cleaning', () => {
    const m = cleanMember(member({ displayName: `${RLO}\u200B` }));
    expect(m.displayName).toBe('');
  });
});

describe('look-alike names', () => {
  it('sees through Cyrillic, Greek, digits and rn/m', () => {
    expect(nameSkeleton('Моrаn')).toBe(nameSkeleton('Moran')); // Cyrillic М, а
    expect(nameSkeleton('Μοran')).toBe(nameSkeleton('Moran')); // Greek Μ, ο
    expect(nameSkeleton('M0ran')).toBe(nameSkeleton('Moran'));
    expect(nameSkeleton('rnoran')).toBe(nameSkeleton('Moran'));
    expect(nameSkeleton('Dana')).not.toBe(nameSkeleton('Moran'));
  });

  it('flags members who copy the owner or a role, never the owner', () => {
    const members = [
      member({ id: 'owner', displayName: 'Moran', isOwner: true }),
      member({ id: 'fake', displayName: 'Моrаn' }),
      member({ id: 'admin', displayName: 'Αdmin' }),
      member({ id: 'heb', displayName: 'מנהל' }),
      member({ id: 'ok', displayName: 'Dana' }),
    ];
    expect([...lookalikeIds(members)].sort()).toEqual(['admin', 'fake', 'heb']);
  });

  it('shows the warning next to the name in the chat', async () => {
    // The hostile demo adds "מודן" next to the owner "מורן", and "Αdmin" (Greek Alpha).
    render(<App core={new MockCore({ demo: true, hostile: true, speed: 0 })} initialSettings={en} />);
    expect((await screen.findAllByText('Look-alike name')).length).toBeGreaterThan(1);
    // The colour that tried to load a remote image fell back to plain grey.
    expect(document.documentElement.innerHTML).not.toContain('tracker.invalid');
  });
});

describe('message text', () => {
  it('is rendered as text: markup and javascript: links stay inert', async () => {
    const evil = '<img src=x onerror="window.__pwned=1"><a href="javascript:alert(1)">click</a>';
    render(<App core={coreWithHistory([evil])} initialSettings={en} />);
    expect(await screen.findByText(evil)).toBeTruthy();
    expect(document.querySelector('img[src="x"]')).toBeNull();
    expect(document.querySelector('a[href^="javascript:"]')).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it('keeps newlines and direction marks, drops overrides', () => {
    expect(cleanText(`שלום\u200F\nhello${RLO}x`).text).toBe('שלום\u200F\nhellox');
  });

  it('clips huge messages and very tall ones, and says so', () => {
    const long = cleanMessage(message({ body: 'x'.repeat(5_000_000) }));
    expect(long.body.length).toBe(BODY_MAX);
    expect(long.clipped).toBe(true);
    const tall = cleanMessage(message({ body: 'a\n'.repeat(10_000) }));
    expect(tall.body.split('\n').length).toBe(100);
    expect(tall.clipped).toBe(true);
    expect(cleanMessage(message({ body: 'short' })).clipped).toBeUndefined();
  });

  it('never cuts an emoji in half', () => {
    const s = 'x'.repeat(BODY_MAX - 1) + '😀';
    expect(cleanText(s).text.endsWith('\uD83D')).toBe(false);
  });

  it('makes reply excerpts one short line', () => {
    expect(cleanExcerpt('a\nb')).toBe('a b');
    expect([...cleanExcerpt('y'.repeat(10_000))].length).toBe(141);
  });

  it('shows the "too long" note in the chat', async () => {
    render(<App core={coreWithHistory(['z'.repeat(50_000)])} initialSettings={en} />);
    expect(await screen.findByText('This message is too long, so only part of it is shown')).toBeTruthy();
  });
});

describe('values that reach CSS, <img> or the clipboard', () => {
  it('only accepts #rrggbb colours (no url(), no extra declarations)', () => {
    expect(safeColor('#12abEF')).toBe('#12abEF');
    expect(safeColor('url(https://tracker.example/p.png)')).toBe('#868e96');
    expect(safeColor('red; background-image: url(x)')).toBe('#868e96');
  });

  it('only accepts inline raster avatars', () => {
    const png = 'data:image/png;base64,iVBORw0KGgo=';
    expect(safeImageSrc(png)).toBe(png);
    expect(safeImageSrc('https://tracker.example/me.png')).toBeUndefined();
    expect(safeImageSrc('javascript:alert(1)')).toBeUndefined();
    expect(safeImageSrc('data:image/svg+xml;base64,PHN2Zz4=')).toBeUndefined();
    expect(safeImageSrc('data:text/html;base64,PGh0bWw+')).toBeUndefined();
    expect(safeImageSrc(`data:image/png;base64,${'A'.repeat(800_000)}`)).toBeUndefined();
  });

  it('keeps invite links one honest line', () => {
    expect(cleanLink(`mvp://join/abc${RLO}gpj.exe\n`)).toBe('mvp://join/abcgpj.exe');
    expect(cleanLink('x'.repeat(10_000)).length).toBe(512);
  });

  it('treats unknown enum values as the safe choice', () => {
    const s = cleanServer({
      id: 's',
      name: 'S',
      initials: 'S',
      color: 'nope',
      ageRestriction: 'kids' as never,
      ownerId: 'o',
      inviteLink: 'l',
      unread: 'yes' as never,
      mentions: -5,
    } satisfies ServerSummary);
    expect(s.ageRestriction).toBe('18+');
    expect(s.unread).toBe(false);
    expect(s.mentions).toBe(0);
  });
});

describe('guardCore', () => {
  it('cleans command results and events, and passes other commands through', async () => {
    const raw = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    const core: CoreApi = guardCore(raw);
    const servers = await core.listServers();
    const channel = (await core.listChannels(servers[0].id))[0];
    const seen: Message[] = [];
    const stop = core.on('message.added', (m) => seen.push(m));
    await core.sendMessage(channel.id, `x${RLO}y`);
    stop();
    expect(seen.at(-1)?.body).toBe('xy');
    // A command the guard does not wrap still reaches the mock with its own state.
    await core.markRead(channel.id);
    expect((await core.getIdentity())?.id).toBe('me-demo');
  });
});

describe('recovery words', () => {
  it('are never written to storage', async () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
    };
    const core = new MockCore({ speed: 0, storage });
    const { words } = await core.createIdentity({ locale: 'en', country: 'IL', birthDate: '2000-01-01' });
    const first = await core.getRecoveryChallenge();
    await core.checkRecoveryWords(Object.fromEntries(first.positions.map((p) => [p, 'nope'])));
    const saved = [...store.values()].join('\n');
    expect(saved.length).toBeGreaterThan(0);
    for (const w of words) expect(saved).not.toMatch(new RegExp(`\\b${w}\\b`));

    // After a restart the check still works, without the words on disk.
    const again = new MockCore({ speed: 0, storage });
    const { positions } = await again.getRecoveryChallenge();
    expect(await again.checkRecoveryWords(Object.fromEntries(positions.map((p) => [p, words[p - 1]])))).toEqual({ ok: true });
  });

  it('drops words that an older build stored', () => {
    const store = new Map([['mvp.mock.v1', JSON.stringify({ identity: { id: 'me-x' }, words: ['a', 'b'], createdServers: [], joinedDemo: false, demo: false })]]);
    new MockCore({
      speed: 0,
      storage: { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => void store.set(k, v), removeItem: (k) => void store.delete(k) },
    });
    expect(store.get('mvp.mock.v1')).not.toContain('words');
  });

  it('cannot be copied from the screen', async () => {
    const { WordGrid } = await import('../screens/onboarding/RecoveryWords');
    const { IntlProvider } = await import('react-intl');
    const { default: msgs } = await import('../i18n/en.json');
    render(
      <IntlProvider locale="en" messages={msgs}>
        <WordGrid words={['apple', 'river']} />
      </IntlProvider>,
    );
    const grid = screen.getByRole('list');
    const copy = new Event('copy', { bubbles: true, cancelable: true });
    grid.querySelector('.word')!.dispatchEvent(copy);
    expect(copy.defaultPrevented).toBe(true);
  });
});

describe('source rules', () => {
  const files = srcFiles(SRC);

  it('never injects HTML or evaluates strings', () => {
    for (const f of files) {
      const text = readFileSync(f, 'utf8');
      expect(text, f).not.toMatch(/dangerouslySetInnerHTML|\.innerHTML\s*=|outerHTML\s*=|insertAdjacentHTML|document\.write|\beval\(|new Function\(/);
    }
  });

  it('never logs (the words, birth date or messages could end up in a log)', () => {
    for (const f of files) expect(readFileSync(f, 'utf8'), f).not.toMatch(/\bconsole\.(log|info|warn|error|debug)\(/);
  });

  it('only renders links it built itself', () => {
    for (const f of files) expect(readFileSync(f, 'utf8'), f).not.toMatch(/href=\{(?!['"`]#)/);
  });
});

describe('Content-Security-Policy', () => {
  it('allows nothing from the network and no inline or eval scripts', () => {
    expect(CSP_DIRECTIVES['default-src']).toEqual(["'none'"]);
    expect(CSP_DIRECTIVES['script-src']).toEqual(["'self'"]);
    expect(CSP).not.toMatch(/unsafe-inline|unsafe-eval|https?:|\*/);
    expect(CSP).toContain("object-src 'none'");
    expect(CSP).toContain("base-uri 'none'");
  });
});
