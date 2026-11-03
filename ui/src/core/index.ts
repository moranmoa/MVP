import { createContext, useContext, useEffect } from 'react';
import type { CoreApi, CoreEventName, CoreEvents } from './api';
import { createIpcCore, hasDesktopBridge } from './ipc';
import { MockCore } from './mock';

export type { CoreApi } from './api';
export * from './types';

/**
 * Picks the core implementation. In the desktop app (B1, Electron) the preload
 * script exposes `window.mvs` and the real core is reached over IPC; the URL
 * flags are ignored there. Anywhere else (browser, tests, screenshots) it is
 * the mock. Either way App wraps the result in guardCore() once, so what the
 * core returns is cleaned before the UI sees it.
 *
 * URL flags for the mock (handy for demos and screenshots):
 *   ?demo=1          start signed in with sample servers
 *   ?demo=unchecked  same, but recovery words not checked yet
 *   ?update=<ms>     announce a ready update after <ms>
 *   ?fresh=1         forget the stored mock account
 *   ?hostile=1       add attacker-style names and messages (security demo)
 *   ?wordsError=1    showing the recovery words fails (load error state)
 *   ?bidi=1          add the mixed-direction messages of B17 §2 to "כללי"
 */
export function createCore(search = typeof location !== 'undefined' ? location.search : ''): CoreApi {
  if (hasDesktopBridge()) return createIpcCore();
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
    wordsError: params.has('wordsError'),
    bidi: params.has('bidi'),
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
