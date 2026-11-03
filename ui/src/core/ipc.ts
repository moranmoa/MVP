// CoreApi over the desktop shell's bridge (desktop/src/preload.js): every
// command becomes `window.mvs.invoke(name, args)`, and core events arrive
// through `window.mvs.onEvent`. The shell's main process checks the command
// against its allow-list and forwards it to the core process.
//
// createCore() (index.ts) should use this when `hasDesktopBridge()` is true and
// wrap it in guardCore() like any other core: what comes back is other
// people's data and is cleaned the same way.

import type { CoreApi, CoreEventName, CoreEvents, Unsubscribe } from './api';
import type { CoreError } from './types';

/** What desktop/src/preload.js exposes as `window.mvs`. */
export interface DesktopBridge {
  invoke(cmd: string, args: unknown[]): Promise<{ ok?: unknown; error?: CoreError | { code: string } }>;
  onEvent(callback: (event: string, payload: unknown) => void): () => void;
}

declare global {
  interface Window {
    mvs?: DesktopBridge;
  }
}

export function hasDesktopBridge(): boolean {
  return typeof window !== 'undefined' && typeof window.mvs?.invoke === 'function';
}

/**
 * Errors the shell adds to the core's own codes: `timeout` (the core did not
 * answer in time; show "busy"), `core-unavailable`, `core-restarted`,
 * `too-large`, `bad-request`.
 */
export function createIpcCore(bridge: DesktopBridge = window.mvs as DesktopBridge): CoreApi {
  const handlers = new Map<string, Set<(payload: never) => void>>();
  // An mvs:// link (`link.open`) can arrive before anything listens for it:
  // on the first launch, or during onboarding before the shell is mounted.
  // The latest one waits for the first listener.
  let pendingLink: unknown = null;
  bridge.onEvent((event, payload) => {
    const set = handlers.get(event);
    if (event === 'link.open' && !set?.size) {
      pendingLink = payload;
      return;
    }
    set?.forEach((h) => h(payload as never));
  });

  const on = <E extends CoreEventName>(event: E, handler: (payload: CoreEvents[E]) => void): Unsubscribe => {
    let set = handlers.get(event);
    if (!set) handlers.set(event, (set = new Set()));
    set.add(handler as (payload: never) => void);
    if (event === 'link.open' && pendingLink !== null) {
      const p = pendingLink;
      pendingLink = null;
      setTimeout(() => (handler as (payload: unknown) => void)(p), 0);
    }
    return () => {
      set.delete(handler as (payload: never) => void);
    };
  };

  const call =
    (cmd: string) =>
    async (...args: unknown[]): Promise<unknown> => {
      const res = await bridge.invoke(cmd, args);
      if (res && res.error) throw res.error;
      return res?.ok ?? null;
    };

  return new Proxy({ on } as CoreApi, {
    get(target, prop) {
      if (prop === 'on') return target.on;
      // Not a thenable, so `await core` does not call the bridge.
      if (typeof prop !== 'string' || prop === 'then') return undefined;
      return call(prop);
    },
  });
}
