import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { FormattedNumber, IntlProvider } from 'react-intl';
import type { Locale } from '../core/types';
import { directionOf, messagesFor } from '../i18n';

export type Theme = 'system' | 'light' | 'dark';

export interface Settings {
  locale: Locale;
  theme: Theme;
  /** Interface scale, 0.75–2. */
  uiScale: number;
  /** Message text size in px, 12–24. */
  fontSize: number;
}

export const SCALE_MIN = 0.75;
export const SCALE_MAX = 2;
export const FONT_MIN = 12;
export const FONT_MAX = 24;
const SCALE_STEP = 0.1;
const STORAGE_KEY = 'mvp.settings.v1';

export function defaultSettings(languages: readonly string[] = typeof navigator !== 'undefined' ? navigator.languages : []): Settings {
  const he = languages.some((l) => l.toLowerCase().startsWith('he') || l.toLowerCase().startsWith('iw'));
  return { locale: he || languages.length === 0 ? 'he' : 'en', theme: 'system', uiScale: 1, fontSize: 15 };
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function load(): Settings {
  const base = defaultSettings();
  let stored: Partial<Settings> = {};
  try {
    stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as Partial<Settings>;
  } catch {
    // ignore broken storage
  }
  // URL overrides are for demos and screenshots and are not saved.
  const params = new URLSearchParams(location.search);
  const lang = params.get('lang');
  const theme = params.get('theme');
  return {
    ...base,
    ...stored,
    ...(lang === 'he' || lang === 'en' ? { locale: lang } : {}),
    ...(theme === 'light' || theme === 'dark' ? { theme } : {}),
  };
}

interface SettingsContextValue {
  settings: Settings;
  update: (patch: Partial<Settings>) => void;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error('SettingsProvider missing');
  return ctx;
}

/** Owns the visual settings and applies them to <html>; also wraps IntlProvider. */
export function SettingsProvider({ children, initial }: { children: ReactNode; initial?: Settings }) {
  const [settings, setSettings] = useState<Settings>(() => initial ?? load());
  const [zoomToast, setZoomToast] = useState(false);
  const toastTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const update = useCallback((patch: Partial<Settings>) => {
    setSettings((s) => {
      const next = {
        ...s,
        ...patch,
        uiScale: Math.round(clamp(patch.uiScale ?? s.uiScale, SCALE_MIN, SCALE_MAX) * 100) / 100,
        fontSize: clamp(patch.fontSize ?? s.fontSize, FONT_MIN, FONT_MAX),
      };
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      } catch {
        // private mode etc. – settings still apply for this session
      }
      return next;
    });
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    root.lang = settings.locale;
    root.dir = directionOf(settings.locale);
    root.dataset.theme = settings.theme;
    root.style.fontSize = `${16 * settings.uiScale}px`;
    root.style.setProperty('--msg-font', `${settings.fontSize / 16}rem`);
  }, [settings]);

  // Ctrl+= / Ctrl+- / Ctrl+0 and Ctrl+wheel change the interface scale.
  useEffect(() => {
    const flash = () => {
      setZoomToast(true);
      clearTimeout(toastTimer.current);
      toastTimer.current = setTimeout(() => setZoomToast(false), 1200);
    };
    const step = (dir: number) => {
      update({ uiScale: (settingsRef.current?.uiScale ?? 1) + dir * SCALE_STEP });
      flash();
    };
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === '=' || e.key === '+') step(1);
      else if (e.key === '-') step(-1);
      else if (e.key === '0') {
        update({ uiScale: 1 });
        flash();
      } else return;
      e.preventDefault();
    };
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      step(e.deltaY < 0 ? 1 : -1);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('wheel', onWheel);
    };
  }, [update]);

  const value = useMemo(() => ({ settings, update }), [settings, update]);

  return (
    <SettingsContext.Provider value={value}>
      <IntlProvider locale={settings.locale} messages={messagesFor(settings.locale)} defaultLocale="he">
        {children}
        {zoomToast && (
          <div className="zoom-toast" role="status">
            <FormattedNumber value={settings.uiScale} style="percent" />
          </div>
        )}
      </IntlProvider>
    </SettingsContext.Provider>
  );
}
