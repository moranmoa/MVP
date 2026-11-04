// T8-01: the core is not answering (desktop shell: core.status 'down', or a
// command rejected with core-unavailable / keyring-unavailable, or nothing at
// all for 10 s at start). A full-screen state instead of a stuck splash or
// silent failures; it goes away when the core is up again.

import { FormattedMessage } from 'react-intl';
import { AlertIcon } from '../components/icons';
import type { CoreApi, CoreDownDetail } from '../core';
import { isCoreError } from '../core';

const TEXT: Record<CoreDownDetail | 'unknown', string> = {
  missing: 'core.down.missing',
  'start-failed': 'core.down.startFailed',
  crashed: 'core.down.crashed',
  'crash-loop': 'core.down.crashLoop',
  keyring: 'core.down.keyring',
  unknown: 'core.down.unknown',
};

/** Shell actions for this screen (desktop/src/preload.js), when the shell has them. */
type ShellActions = { retryCore?: () => Promise<boolean>; openLogsFolder?: () => Promise<boolean> };
export const shellActions = (): ShellActions => (window as { mvs?: ShellActions }).mvs ?? {};

export function CoreDownScreen({ detail, onRetry }: { detail: CoreDownDetail | undefined; onRetry: () => void }) {
  const { openLogsFolder } = shellActions();
  return (
    <div className="core-down" role="alert">
      <AlertIcon />
      <h1>
        <FormattedMessage id="core.down.title" />
      </h1>
      <p>
        <FormattedMessage id={TEXT[detail ?? 'unknown']} />
      </p>
      <div className="actions">
        <button type="button" className="primary" onClick={onRetry}>
          <FormattedMessage id="common.retry" />
        </button>
        {openLogsFolder && (
          <button type="button" className="secondary" onClick={() => void openLogsFolder()}>
            <FormattedMessage id="core.down.openLogs" />
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * Wraps the core so a command rejected with `core-unavailable` (or the
 * keychain's `keyring-unavailable`) reports it. The rejection itself still
 * reaches the caller unchanged.
 */
export function watchUnavailable(core: CoreApi, onDown: (detail: CoreDownDetail | undefined) => void): CoreApi {
  return new Proxy(core, {
    get(target, prop, receiver) {
      const value = Reflect.get(target, prop, receiver);
      if (typeof value !== 'function' || prop === 'on') return value;
      return (...args: unknown[]) => {
        const result = (value as (...a: unknown[]) => unknown).apply(target, args);
        if (result instanceof Promise) {
          result.catch((e: unknown) => {
            if (isCoreError(e) && e.code === 'core-unavailable') onDown(e.detail);
            else if (isCoreError(e) && e.code === 'keyring-unavailable') onDown('keyring');
          });
        }
        return result;
      };
    },
  });
}
