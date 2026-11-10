// G-01 (docs/ux/GAPS.md §2.1): my own order of the servers in the rail. Kept
// with the local settings on this computer only; never sent to the core or
// anyone else.

import type { Id } from '../core/types';

/** The servers in my order. Any not in it (new ones) go at the end, in join order. */
export function sortByOrder<T extends { id: Id }>(servers: T[], order: readonly Id[]): T[] {
  const rank = new Map(order.map((id, i) => [id, i]));
  return servers
    .map((s, i) => ({ s, i }))
    .sort((a, b) => (rank.get(a.s.id) ?? order.length + a.i) - (rank.get(b.s.id) ?? order.length + b.i))
    .map(({ s }) => s);
}

/** The ids with `id` moved to position `to` (0-based), clamped to the ends. */
export function moveTo(ids: readonly Id[], id: Id, to: number): Id[] {
  const from = ids.indexOf(id);
  if (from < 0) return [...ids];
  const out = ids.filter((x) => x !== id);
  out.splice(Math.max(0, Math.min(out.length, to)), 0, id);
  return out;
}

/** Stored order, checked: a list of short strings, at most 500. Anything else is no order. */
export function cleanOrder(raw: unknown): Id[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter((x): x is string => typeof x === 'string' && x.length > 0 && x.length <= 128).slice(0, 500);
}
