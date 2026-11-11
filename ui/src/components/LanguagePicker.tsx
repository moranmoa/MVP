// The interface language, picked from a list (owner, 0.4.1). The options come
// from the catalog registry (i18n/index.ts), each in its own name, so adding a
// language is one registry entry.

import { useId } from 'react';
import { useSettings } from '../app/settings';
import { LANGUAGES } from '../i18n';
import type { Locale } from '../core/types';

/**
 * `hideLabel`: where a heading already says it (onboarding step 1), the label is
 * for screen readers only. A 🌐 next to the list helps someone who can't read
 * the current language find it (UX).
 */
export function LanguagePicker({ label, hideLabel = false }: { label: string; hideLabel?: boolean }) {
  const { settings, update } = useSettings();
  const id = useId();
  return (
    <label className="field language-picker" htmlFor={id}>
      <span className={hideLabel ? 'visually-hidden' : undefined}>{label}</span>
      <span className="language-picker-row">
        <span className="language-globe" aria-hidden="true">
          🌐
        </span>
      <select id={id} value={settings.locale} onChange={(e) => update({ locale: e.target.value as Locale })}>
        {LANGUAGES.map((l) => (
          <option key={l.code} value={l.code} lang={l.code} dir={l.dir}>
            {l.name}
          </option>
        ))}
      </select>
      </span>
    </label>
  );
}
