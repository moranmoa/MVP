// G-24 (#330): mentions. A message body carries <@userId> tokens, never names;
// the UI shows them as "@Name" from the member list, and a token for someone
// who isn't a member (left, or a stranger) as a neutral former-member chip.
// Raw ids are never shown.

/** A mention token. Real ids are 64 hex; the mock's short ids pass too. */
export const MENTION = /<@([0-9A-Za-z_-]{1,64})>/g;

export type MentionPart = { text: string } | { mention: string };

/** The text split into plain runs and mentioned ids. */
export function splitMentions(text: string): MentionPart[] {
  const out: MentionPart[] = [];
  let last = 0;
  for (const m of text.matchAll(MENTION)) {
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ mention: m[1] });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/**
 * A body for the editor: each token becomes "@Name" (or the former-member
 * label), and `map` learns how to turn it back into the token on save.
 */
export function toDisplay(body: string, nameOf: (id: string) => string | undefined, unknown: string, map: Map<string, string>): string {
  return body.replace(MENTION, (token, id: string) => {
    const shown = `@${nameOf(id) ?? unknown}`;
    if (!map.has(shown)) map.set(shown, token);
    return shown;
  });
}

/** The editor's text with every known "@Name" turned back into its token (longest first). */
export function toTokens(text: string, map: Map<string, string>): string {
  let out = text;
  for (const [shown, token] of [...map].sort((a, b) => b[0].length - a[0].length)) out = out.split(shown).join(token);
  return out;
}

/** The "@query" being typed right before the caret, if any. */
export function mentionQuery(text: string, caret: number): { start: number; query: string } | null {
  const m = /(^|\s)@([^\s@<>]{0,32})$/.exec(text.slice(0, caret));
  return m ? { start: caret - m[2].length - 1, query: m[2] } : null;
}
