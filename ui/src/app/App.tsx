import { createContext, useContext, useEffect, useState } from 'react';
import { FormattedMessage } from 'react-intl';
import { CoreContext, type CoreApi } from '../core';
import { guardCore } from '../core/sanitize';
import { Onboarding } from '../screens/onboarding/Onboarding';
import { Shell } from '../screens/shell/Shell';
import { SettingsProvider, type Settings } from './settings';
import { Store, useStore } from './store';

const StoreContext = createContext<Store | null>(null);

export function useAppStore(): Store {
  const s = useContext(StoreContext);
  if (!s) throw new Error('StoreContext missing');
  return s;
}

export function App({ core: rawCore, initialSettings }: { core: CoreApi; initialSettings?: Settings }) {
  // Every value from the core passes through the cleaners once, here.
  const [core] = useState(() => guardCore(rawCore));
  const [store] = useState(() => new Store(core));
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const stop = store.start();
    void store.init().then(() => setReady(true));
    return stop;
  }, [store]);

  return (
    <CoreContext.Provider value={core}>
      <StoreContext.Provider value={store}>
        <SettingsProvider initial={initialSettings}>
          {ready ? <Root /> : <Splash />}
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
  const [onboarding, setOnboarding] = useState(!identity);

  if (onboarding || !identity) {
    return (
      <Onboarding
        onFinished={async () => {
          await store.loadServers();
          setOnboarding(false);
        }}
      />
    );
  }
  return <Shell />;
}
