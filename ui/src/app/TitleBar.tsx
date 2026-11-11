// Our own window frame (desktop app on Windows and Linux): the shell opens a
// frameless window and this bar is its title bar. The whole bar drags the
// window (double-click maximizes); the buttons call the shell's
// `window.mvs.windowControl`. In RTL the controls sit on the left, as in
// mirrored Windows apps; without the shell nothing is drawn.

import { useEffect, useState, type ReactNode } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useCoreEvent } from '../core';
import { CloseIcon } from '../components/icons';

type WindowAction = 'minimize' | 'maximize' | 'close' | 'state';
type WindowControl = (action: WindowAction) => Promise<{ maximized: boolean } | null>;

function windowControl(): WindowControl | null {
  const bridge = (window as { mvs?: { windowControl?: WindowControl } }).mvs;
  return typeof bridge?.windowControl === 'function' ? bridge.windowControl : null;
}

/** Wraps the app with the title bar when the shell gives us a frameless window. */
export function WindowFrame({ children }: { children: ReactNode }) {
  if (!windowControl()) return <>{children}</>;
  return (
    <div className="window-frame">
      <TitleBar />
      <div className="window-body">{children}</div>
    </div>
  );
}

function TitleBar() {
  const intl = useIntl();
  const control = windowControl()!;
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    void control('state').then((s) => s && setMaximized(s.maximized));
  }, [control]);
  useCoreEvent('window.state', (s) => setMaximized(s.maximized));

  const act = (action: WindowAction) => () =>
    void control(action).then((s) => {
      if (s) setMaximized(s.maximized);
    });

  return (
    <header className="titlebar" onDoubleClick={(e) => e.target === e.currentTarget && act('maximize')()}>
      <span className="titlebar-logo" aria-hidden="true">
        M
      </span>
      <span className="titlebar-title">
        <FormattedMessage id="app.name" />
      </span>
      <div className="titlebar-controls">
        <button type="button" className="titlebar-button" aria-label={intl.formatMessage({ id: 'window.minimize' })} title={intl.formatMessage({ id: 'window.minimize' })} onClick={act('minimize')}>
          <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" focusable="false">
            <path d="M2 6.5h8" stroke="currentColor" strokeWidth="1.2" />
          </svg>
        </button>
        <button
          type="button"
          className="titlebar-button"
          aria-label={intl.formatMessage({ id: maximized ? 'window.restore' : 'window.maximize' })}
          title={intl.formatMessage({ id: maximized ? 'window.restore' : 'window.maximize' })}
          onClick={act('maximize')}
        >
          <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" strokeWidth="1.2">
            {maximized ? <path d="M2.5 4.5h5v5h-5zM4.5 4.5v-2h5v5h-2" /> : <rect x="2.5" y="2.5" width="7" height="7" />}
          </svg>
        </button>
        <button type="button" className="titlebar-button close" aria-label={intl.formatMessage({ id: 'window.close' })} title={intl.formatMessage({ id: 'window.close' })} onClick={act('close')}>
          <CloseIcon />
        </button>
      </div>
    </header>
  );
}
