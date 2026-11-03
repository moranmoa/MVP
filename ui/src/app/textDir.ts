// Direction of a piece of user text (UX-01, B17).
//
// `dir="auto"` looks only at the first strong letter, so "lol זה היה מטורף"
// or "GG על המשחק" turn LTR in a Hebrew chat. Here the majority of strong
// letters decides; a tie (or text with no letters, like "🎉🎉") goes to the
// interface direction. Alignment is not decided here: CSS keeps every message
// on the interface side, whatever its direction.

import { useIntl } from 'react-intl';
import type { Locale } from '../core/types';
import { directionOf } from '../i18n';

export type Dir = 'rtl' | 'ltr';

const RTL_LETTER = /[\p{Script=Hebrew}\p{Script=Arabic}\p{Script=Syriac}\p{Script=Thaana}\p{Script=Nko}]/u;
const LETTER = /\p{L}/gu;

export function textDir(text: string, uiDir: Dir): Dir {
  let rtl = 0;
  let ltr = 0;
  for (const [ch] of text.matchAll(LETTER)) {
    if (RTL_LETTER.test(ch)) rtl++;
    else ltr++;
  }
  return rtl === ltr ? uiDir : rtl > ltr ? 'rtl' : 'ltr';
}

/** The interface direction, from the active language. */
export function useUiDir(): Dir {
  return directionOf(useIntl().locale as Locale);
}

/** Props for an element that shows or edits user text: its own direction, aligned to the interface side. */
export function useUserTextProps(text: string): { dir: Dir; className: string } {
  const uiDir = useUiDir();
  return { dir: text ? textDir(text, uiDir) : uiDir, className: 'user-text' };
}
