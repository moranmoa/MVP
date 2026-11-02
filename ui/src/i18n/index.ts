import type { Locale } from '../core/types';
import en from './en.json';
import he from './he.json';

export type MessageId = keyof typeof he;

const catalogs: Record<Locale, Record<string, string>> = { he, en };

export const LOCALES: Locale[] = ['he', 'en'];

export function messagesFor(locale: Locale): Record<string, string> {
  return catalogs[locale];
}

export function directionOf(locale: Locale): 'rtl' | 'ltr' {
  return locale === 'he' ? 'rtl' : 'ltr';
}
