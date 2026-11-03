import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { IntlMessageFormat } from 'intl-messageformat';
import { describe, expect, it } from 'vitest';
import en from '../i18n/en.json';
import he from '../i18n/he.json';

const catalogs = { he, en } as Record<string, Record<string, string>>;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === 'test' ? [] : sourceFiles(p);
    return /\.(ts|tsx)$/.test(f) ? [p] : [];
  });
}

const SRC = join(__dirname, '..');

describe('message catalogs', () => {
  it('Hebrew and English have the same keys', () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(he).sort());
  });

  it('every message is valid ICU and formats', () => {
    for (const [locale, messages] of Object.entries(catalogs)) {
      for (const [id, text] of Object.entries(messages)) {
        const fmt = new IntlMessageFormat(text, locale);
        expect(() => fmt.format({ count: 2, n: 3, name: 'x', channel: 'c', step: 1, total: 7, size: 15, positions: '1', age: '18', code: 'AB', version: '1', used: '1', limit: '2', max: 9, kbd: (c: unknown) => c }), `${locale}:${id}`).not.toThrow();
      }
    }
  });

  // B17 / UX-03: user data inside a sentence is isolated (FSI … PDI), so an
  // English channel name cannot reorder a Hebrew sentence, or the reverse.
  it('isolates names, codes and sizes inside sentences', () => {
    const FSI = String.fromCharCode(0x2068);
    const PDI = String.fromCharCode(0x2069);
    // Exactly FSI {x} PDI, with nothing else (not even a channel's #) inside the isolate.
    const isolated = new RegExp(`${FSI}{(channel|name|code|version|used|limit)}${PDI}`, 'g');
    for (const [locale, messages] of Object.entries(catalogs)) {
      for (const [id, text] of Object.entries(messages)) {
        const bare = text.replace(isolated, '').match(/{(channel|name|code|version|used|limit)}/);
        expect(bare, `${locale}:${id}`).toBeNull();
      }
    }
  });

  it('shows keyboard shortcuts as <kbd>, never as bare text', () => {
    for (const [locale, messages] of Object.entries(catalogs)) {
      for (const [id, text] of Object.entries(messages)) {
        const bare = text.replace(/<kbd>.*?<\/kbd>/g, '').match(/\b(Ctrl|Alt|Shift)\+|\bEsc\b/);
        expect(bare, `${locale}:${id}`).toBeNull();
      }
    }
  });

  it('Hebrew interface never says seed, key or peer', () => {
    const banned = /\b(seed|key|peer)s?\b|מפתח|סיד/i;
    const offenders = Object.entries(he).filter(([, t]) => banned.test(t));
    expect(offenders).toEqual([]);
  });

  it('Hebrew avoids common gendered second-person forms', () => {
    // Singular "you" and gendered imperatives; the copy uses plural or infinitive.
    // (JS \b does not work on Hebrew letters, hence the explicit delimiters.)
    const words = ['אתה', 'תלחץ', 'תלחצי', 'בחר', 'בחרי', 'הזן', 'הזיני', 'לחץ', 'לחצי', 'מסכים', 'מסכימה'];
    const gendered = new RegExp(`(^|[\\s"'(])(${words.join('|')})(?=$|[\\s.,!?:;"')–])`);
    const offenders = Object.entries(he).filter(([, t]) => gendered.test(t));
    expect(offenders).toEqual([]);
  });

  it('every message id used in code exists', () => {
    const used = new Set<string>();
    for (const f of sourceFiles(SRC)) {
      const text = readFileSync(f, 'utf8');
      for (const m of text.matchAll(/id[=:]\s*\{?\s*['"]([a-z][\w.]+)['"]/g)) used.add(m[1]);
    }
    const missing = [...used].filter((id) => !(id in he));
    expect(missing).toEqual([]);
  });

  it('has no hard-coded words in JSX text', () => {
    const offenders: string[] = [];
    for (const f of sourceFiles(SRC).filter((p) => p.endsWith('.tsx'))) {
      const text = readFileSync(f, 'utf8');
      // Text directly between tags, e.g. <p>Hello</p>. Numbers, symbols and emoji are fine.
      for (const m of text.matchAll(/(?<![=-])>\s*([^<>{}\n]*[A-Za-z֐-׿]{2,}[^<>{}\n]*)\s*</g)) {
        if (/^[,)]|[:?]|=>|&&|\|\|/.test(m[1].trim())) continue; // TypeScript, not JSX text
        offenders.push(`${f}: ${m[1].trim()}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
