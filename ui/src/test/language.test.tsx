// 0.4.1: the system language when we have it, otherwise English; and a picker
// listing every catalog in the registry, each in its own name.

import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import { defaultSettings } from '../app/settings';
import { LANGUAGES, pickLocale } from '../i18n';
import { MockCore } from '../core/mock';

describe('interface language', () => {
  it('uses the system language when we have it, otherwise English', () => {
    expect(defaultSettings([]).locale).toBe('en');
    expect(defaultSettings(['fr-FR', 'de']).locale).toBe('en');
    expect(defaultSettings(['he-IL']).locale).toBe('he');
    expect(defaultSettings(['iw']).locale).toBe('he');
    expect(defaultSettings(['fr', 'he']).locale).toBe('he'); // the first one we have
    expect(pickLocale(['en-GB'])).toBe('en');
  });

  it('the picker lists every language in the registry, each in its own name', async () => {
    render(<App core={new MockCore({ speed: 0 })} initialSettings={{ locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 }} />);
    const select = await screen.findByRole('combobox', { name: 'Choose a language' });
    const options = within(select).getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => [o.value, o.textContent, o.lang])).toEqual(LANGUAGES.map((l) => [l.code, l.name, l.code]));
    expect(options.map((o) => o.textContent)).toEqual(['English', 'עברית']);
  });
});
