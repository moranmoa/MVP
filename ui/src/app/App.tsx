import { createContext, useContext, useEffect, useRef, useState } from 'react';
import { FormattedMessage } from 'react-intl';
import { CoreContext, type CoreApi } from '../core';
import { guardCore } from '../core/sanitize';
import type { CoreDownDetail } from '../core/types';
import { CoreDownScreen, shellActions, watchUnavailable } from './CoreDown';

/** No answer from the core this long after start: show the screen (cause unknown). */
const START_TIMEOUT_MS = 10_000;
import { Onboarding, resumeStep } from '../screens/onboarding/Onboarding';
import { Shell } from '../screens/shell/Shell';
import { SettingsProvider, type Settings } from './settings';
import { WindowFrame } from './TitleBar';
import { Store, useStore } from './store';

const StoreContext = createContext<Store | null>(null);

export function useAppStore(): Store {
  const s = useContext(StoreContext);
  if (!s) throw new Error('StoreContext missing');
  return s;
}

export function App({ core: rawCore, initialSettings }: { core: CoreApi; initialSettings?: Settings }) {
  // T8-01: the core is down (shell status, or a command refused with
  // core-unavailable). `false` = up as far as we know.
  const [down, setDown] = useState<{ detail?: CoreDownDetail } | false>(false);
  const reportDown = useRef((detail: CoreDownDetail | undefined) => setDown({ detail }));
  // Every value from the core passes through the cleaners once, here.
  const [core] = useState(() => watchUnavailable(guardCore(rawCore), (d) => reportDown.current(d)));
  const [store] = useState(() => new Store(core));
  const [ready, setReady] = useState(false);

  const load = useRef(() => store.init().then(() => setReady(true), () => {}));

  useEffect(() => {
    const stop = store.start();
    let answered = false;
    void load.current().then(() => (answered = true));
    const hang = setTimeout(() => {
      if (!answered) setDown((d) => d || {});
    }, START_TIMEOUT_MS);
    const off = core.on('core.status', (s) => {
      if (s.state === 'down') setDown({ detail: s.detail });
      else if (s.state === 'up') {
        setDown(false);
        void load.current(); // the first load may have failed while the core was down
      }
    });
    return () => {
      clearTimeout(hang);
      off();
      stop();
    };
  }, [store, core]);

  // "Try again": the shell restarts the core now (its core.status then hides
  // the screen); without a shell, just try loading again.
  const retry = () => {
    const { retryCore } = shellActions();
    if (retryCore) void retryCore();
    else {
      setDown(false);
      void load.current();
    }
  };

  return (
    <CoreContext.Provider value={core}>
      <StoreContext.Provider value={store}>
        <SettingsProvider initial={initialSettings}>
          <WindowFrame>{down ? <CoreDownScreen detail={down.detail} onRetry={retry} /> : ready ? <Root /> : <Splash />}</WindowFrame>
        </SettingsProvider>
      </StoreContext.Provider>
    </CoreContext.Provider>
  );
}

function Splash() {
  return (
    <div className="splash" role="status">
      <FormattedMessage id="common.loading" />
    </div>
  );
}

function Root() {
  const store = useAppStore();
  const identity = useStore(store, (s) => s.identity);
  // Stays on onboarding until the last step finishes, even though the
  // identity already exists from the "preparing" step on.
  const [resume] = useState(() => resumeStep(identity));
  const [onboarding, setOnboarding] = useState(!identity || !!resume);

  if (onboarding || !identity) {
    return (
      <Onboarding
        initialStep={resume}
        onFinished={async () => {
          await store.loadServers();
          setOnboarding(false);
        }}
      />
    );
  }
  return <Shell />;
}
