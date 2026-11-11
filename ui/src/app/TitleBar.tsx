// Our own title bar (docs/ux/specs/desktop-chrome.md §2), desktop app on
// Windows and Linux. 32px: the mark, "MVS" and the open server's name; the
// whole bar drags the window (double-click maximizes). Windows: the OS draws
// the buttons on an overlay at the top right (`windowFrame: 'overlay'`).
// Linux: we draw them (`'custom'`). The buttons stay on the physical right in
// both languages; only the content follows the UI direction. Without the
// shell (browser, tests, macOS) nothing is drawn.

import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import logo from '../assets/mvs-logo.svg';
import { useCoreEvent } from '../core';
import { Dialog } from '../components/common';
import { CloseIcon } from '../components/icons';
import { useAppStore } from './App';
import { useMediaQuery } from './media';
import { useSettings } from './settings';

type WindowAction = 'minimize' | 'toggle-maximize' | 'close' | 'theme:light' | 'theme:dark';
interface WindowBridge {
  windowFrame: 'overlay' | 'custom';
  windowControl: (action: WindowAction) => Promise<boolean>;
}

function windowBridge(): WindowBridge | null {
  const b = (window as { mvs?: Partial<WindowBridge> }).mvs;
  return typeof b?.windowControl === 'function' && (b.windowFrame === 'overlay' || b.windowFrame === 'custom') ? (b as WindowBridge) : null;
}

const TitleContext = createContext<(title: string | null) => void>(() => {});

/** Shows `title` (the open server's name) in the title bar while mounted. */
export function useWindowTitle(title: string | null | undefined) {
  const set = useContext(TitleContext);
  useEffect(() => {
    set(title ?? null);
    return () => set(null);
  }, [set, title]);
}

/** Wraps the app with the title bar when the shell gives us our own frame. */
export function WindowFrame({ children }: { children: ReactNode }) {
  const [title, setTitle] = useState<string | null>(null);
  const bridge = windowBridge();
  if (!bridge) return <>{children}</>;
  return (
    <TitleContext.Provider value={setTitle}>
      <div className="window-frame">
        <TitleBar bridge={bridge} title={title} />
        <div className="window-body">{children}</div>
      </div>
    </TitleContext.Provider>
  );
}

function TitleBar({ bridge, title }: { bridge: WindowBridge; title: string | null }) {
  const intl = useIntl();
  const store = useAppStore();
  const [state, setState] = useState({ maximized: false, fullScreen: false });
  const [confirmClose, setConfirmClose] = useState(false);
  useCoreEvent('window.state', setState);
  // Close during a call: the shell holds it and asks us to confirm.
  useCoreEvent('window.closeRequest', () => setConfirmClose(true));

  // Windows: the overlay buttons follow the app's theme at once.
  const { theme } = useSettings().settings;
  const systemDark = useMediaQuery('(prefers-color-scheme: dark)');
  const dark = theme === 'dark' || (theme === 'system' && systemDark);
  useEffect(() => {
    if (bridge.windowFrame === 'overlay') void bridge.windowControl(dark ? 'theme:dark' : 'theme:light');
  }, [bridge, dark]);

  const control = (action: WindowAction) => () => void bridge.windowControl(action);
  const close = () => (store.getState().call ? setConfirmClose(true) : control('close')());
  const label = (id: string) => intl.formatMessage({ id });

  if (state.fullScreen) return null;
  return (
    <>
      <header role="banner" className={`titlebar ${bridge.windowFrame}`} onDoubleClick={(e) => !(e.target as HTMLElement).closest('button') && control('toggle-maximize')()}>
        <img className="titlebar-mark" src={logo} alt="" width={16} height={16} draggable={false} />
        <span className="titlebar-title">
          <FormattedMessage id="app.name" />
          {title && (
            <>
              {' · '}
              <bdi>{title}</bdi>
            </>
          )}
        </span>
        {bridge.windowFrame === 'custom' && (
          <div className="titlebar-controls" dir="ltr">
            <button type="button" className="titlebar-button" aria-label={label('window.minimize')} title={label('window.minimize')} onClick={control('minimize')}>
              <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" focusable="false">
                <path d="M2 6.5h8" stroke="currentColor" strokeWidth="1.2" />
              </svg>
            </button>
            <button
              type="button"
              className="titlebar-button"
              aria-label={label(state.maximized ? 'window.restore' : 'window.maximize')}
              title={label(state.maximized ? 'window.restore' : 'window.maximize')}
              onClick={control('toggle-maximize')}
            >
              <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" strokeWidth="1.2">
                {state.maximized ? <path d="M2.5 4.5h5v5h-5zM4.5 4.5v-2h5v5h-2" /> : <rect x="2.5" y="2.5" width="7" height="7" />}
              </svg>
            </button>
            <button type="button" className="titlebar-button close" aria-label={label('window.close')} title={label('window.close')} onClick={close}>
              <CloseIcon />
            </button>
          </div>
        )}
      </header>
      {confirmClose && (
        <Dialog open onClose={() => setConfirmClose(false)} title={<FormattedMessage id="window.closeInCall" />}>
          <div className="actions">
            <button type="button" className="danger" autoFocus onClick={control('close')}>
              <FormattedMessage id="window.close" />
            </button>
            <button type="button" className="secondary" onClick={() => setConfirmClose(false)}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
        </Dialog>
      )}
    </>
  );
}
