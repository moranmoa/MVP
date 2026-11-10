import { createContext, useContext, useEffect } from 'react';
import type { CoreApi, CoreEventName, CoreEvents } from './api';
import { createIpcCore, hasDesktopBridge } from './ipc';
import { MockCore, type MockOptions } from './mock';

export type { CoreApi } from './api';
export * from './types';

/**
 * Picks the core implementation. In the desktop app (B1, Electron) the preload
 * script exposes `window.mvs` and the real core is reached over IPC; the URL
 * flags are ignored there. Anywhere else (browser, tests, screenshots) it is
 * the mock. Either way App (src/app/App.tsx) wraps the result in guardCore()
 * once, so what the core returns is cleaned before the UI sees it. Don't wrap
 * it again here, and don't hand App a core any other way.
 *
 * URL flags for the mock (handy for demos and screenshots):
 *   ?demo=1          start signed in with sample servers
 *   ?demo=unchecked  same, but recovery words not checked yet
 *   ?update=<ms>     announce a ready update after <ms>
 *   ?installed=1     a new version was just installed, with release notes
 *   ?rolledBack=1    the last update failed and the previous version is back
 *   ?coreDown=<detail> the desktop shell reports the core down (missing, start-failed, crashed, crash-loop)
 *   ?callError=<code> joinCall fails with call-full, relay-only or not-implemented
 *   ?keyPending=arrive|fail|wait  sends wait for the channel key, then go out, fail or keep waiting (#288)
 *   ?signOutKeyring=1 signOut fails with keyring-unavailable (#304)
 *   ?profileError=1  updateProfile fails (B11e)
 *   ?roomFull=1      every demo voice room already has 5 people
 *   ?stun=<url>      the desktop shell's STUN URL for voice (stun:host:port)
 *   ?inCall=1        restartToUpdate is refused as during a call (not-allowed)
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
    updateInstalled: params.has('installed'),
    updateRolledBack: params.has('rolledBack'),
    inCall: params.has('inCall'),
    coreDown: (params.get('coreDown') as MockOptions['coreDown']) ?? undefined,
    callError: (params.get('callError') as MockOptions['callError']) ?? undefined,
    roomFull: params.has('roomFull'),
    profileError: params.has('profileError'),
    signOutKeyring: params.has('signOutKeyring'),
    keyPending: (params.get('keyPending') as MockOptions['keyPending']) ?? undefined,
    relayOnly: params.has('relayOnly'),
    outdatedMembers: params.has('outdated'),
    closedServer: params.has('closed'),
    stunUrl: params.get('stun') ?? undefined,
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
