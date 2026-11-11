// The interface language, picked from a list (owner, 0.4.1). The options come
// from the catalog registry (i18n/index.ts), each in its own name, so adding a
// language is one registry entry.

import { useId } from 'react';
import { useSettings } from '../app/settings';
import { LANGUAGES } from '../i18n';
import type { Locale } from '../core/types';

export function LanguagePicker({ label }: { label: string }) {
  const { settings, update } = useSettings();
  const id = useId();
  return (
    <label className="field language-picker" htmlFor={id}>
      <span>{label}</span>
      <select id={id} value={settings.locale} onChange={(e) => update({ locale: e.target.value as Locale })}>
        {LANGUAGES.map((l) => (
          <option key={l.code} value={l.code} lang={l.code} dir={l.dir}>
            {l.name}
          </option>
        ))}
      </select>
    </label>
  );
}
