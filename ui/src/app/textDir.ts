// Direction of a piece of user text (UX-01, B17, spec docs/ux/specs/B17-mixed-direction.md §2).
//
// `dir="auto"` looks only at the first strong letter, so "lol זה היה מטורף"
// or "GG על המשחק" turn LTR in a Hebrew chat. Here a leading RLM/LRM wins,
// then the majority of strong letters, leaving out links, @mentions and
// `code`; a tie (or text with no letters, like "🎉🎉") goes to the
// interface direction. Alignment is not decided here: CSS keeps every message
// on the interface side, whatever its direction.

import { useIntl } from 'react-intl';
import type { Locale } from '../core/types';
import { directionOf } from '../i18n';

export type Dir = 'rtl' | 'ltr';

const RTL_LETTER = /[\p{Script=Hebrew}\p{Script=Arabic}\p{Script=Syriac}\p{Script=Thaana}\p{Script=Nko}]/u;
const LETTER = /\p{L}/gu;
const NOT_COUNTED = /https?:\/\/\S+|@\S+|`[^`]*`/gu;
const RLM = String.fromCharCode(0x200f);
const LRM = String.fromCharCode(0x200e);

/** Direction of one paragraph: an explicit leading mark wins, then the majority of strong letters, then the UI direction. */
export function textDir(text: string, uiDir: Dir): Dir {
  if (text.startsWith(RLM)) return 'rtl';
  if (text.startsWith(LRM)) return 'ltr';
  let rtl = 0;
  let ltr = 0;
  for (const [ch] of text.replace(NOT_COUNTED, ' ').matchAll(LETTER)) {
    if (RTL_LETTER.test(ch)) rtl++;
    else ltr++;
  }
  return rtl === ltr ? uiDir : rtl > ltr ? 'rtl' : 'ltr';
}

/** Splits on newlines and returns each paragraph with its own direction. */
export function paragraphs(text: string, uiDir: Dir): { text: string; dir: Dir }[] {
  return text.split('\n').map((p) => ({ text: p, dir: textDir(p, uiDir) }));
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
