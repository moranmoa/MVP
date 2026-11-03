import { createContext, useContext, useEffect } from 'react';
import type { CoreApi, CoreEventName, CoreEvents } from './api';
import { MockCore } from './mock';

export type { CoreApi } from './api';
export * from './types';

/**
 * Picks the core implementation. When the Rust core lands, this is the one
 * place that changes: e.g. `if ('__TAURI_INTERNALS__' in window) return new TauriCore()`.
 *
 * URL flags for the mock (handy for demos and screenshots):
 *   ?demo=1          start signed in with sample servers
 *   ?demo=unchecked  same, but recovery words not checked yet
 *   ?update=<ms>     announce a ready update after <ms>
 *   ?fresh=1         forget the stored mock account
 *   ?hostile=1       add attacker-style names and messages (security demo)
 */
export function createCore(search = typeof location !== 'undefined' ? location.search : ''): CoreApi {
  const params = new URLSearchParams(search);
  const demo = params.get('demo');
  const storage = typeof localStorage !== 'undefined' ? localStorage : undefined;
  if (params.has('fresh')) storage?.removeItem('mvp.mock.v1');
  const update = params.get('update');
  return new MockCore({
    demo: !!demo,
    demoUnchecked: demo === 'unchecked',
    simulateActivity: !!demo && !params.has('still'),
    hostile: params.has('hostile'),
    updateAfterMs: update !== null ? Number(update) || 0 : 45_000,
    storage,
  });
}

export const CoreContext = createContext<CoreApi | null>(null);

export function useCore(): CoreApi {
  const core = useContext(CoreContext);
  if (!core) throw new Error('CoreContext missing');
  return core;
}

/** Subscribes to a core event for the lifetime of the component. */
export function useCoreEvent<E extends CoreEventName>(
  event: E,
  handler: (payload: CoreEvents[E]) => void,
  deps: unknown[] = [],
): void {
  const core = useCore();
  useEffect(() => core.on(event, handler), [core, event, ...deps]); // eslint-disable-line react-hooks/exhaustive-deps
}
