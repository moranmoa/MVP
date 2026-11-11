import type { Locale } from '../core/types';
import en from './en.json';
import he from './he.json';

export type MessageId = keyof typeof he;

/**
 * The available interface languages. Adding one is one entry here (and its
 * catalog): the language picker, the default and the direction follow.
 * `name` is the language's own name, as the picker shows it.
 */
export const LANGUAGES: { code: Locale; name: string; dir: 'rtl' | 'ltr'; messages: Record<string, string> }[] = [
  { code: 'en', name: 'English', dir: 'ltr', messages: en },
  { code: 'he', name: 'עברית', dir: 'rtl', messages: he },
];

export const LOCALES: Locale[] = LANGUAGES.map((l) => l.code);

/** The default when the system language isn't one we have. */
export const FALLBACK_LOCALE: Locale = 'en';

export function messagesFor(locale: Locale): Record<string, string> {
  return (LANGUAGES.find((l) => l.code === locale) ?? LANGUAGES[0]).messages;
}

export function directionOf(locale: Locale): 'rtl' | 'ltr' {
  return LANGUAGES.find((l) => l.code === locale)?.dir ?? 'ltr';
}

/** The first system language we have a catalog for, or English (owner, 0.4.1). */
export function pickLocale(languages: readonly string[]): Locale {
  for (const tag of languages) {
    const base = tag.toLowerCase().split('-')[0];
    const code = base === 'iw' ? 'he' : base; // old Hebrew tag
    const hit = LANGUAGES.find((l) => l.code === code);
    if (hit) return hit.code;
  }
  return FALLBACK_LOCALE;
}
