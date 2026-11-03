// B23: the core refuses names with bidi, control or zero-width characters
// (bad-request). The UI says so on the field and keeps the name as typed,
// never failing silently or stripping it.

import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const MESSAGE = "The name contains characters that can't be used in names.";
const NAME = `Evil${String.fromCharCode(0x202e)}name`;

function memoryStorage() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), removeItem: (k: string) => void m.delete(k) };
}

describe('a name the core refuses', () => {
  it('creating a server: the error is on the name field, the dialog stays open with the name', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    core.createServer = async () => {
      throw { code: 'bad-request' };
    };
    render(<App core={core} initialSettings={settings} />);
    fireEvent.click(await screen.findByRole('button', { name: /^Home$/ }));
    fireEvent.click(await screen.findByRole('button', { name: /Create a server/ }));
    const field = screen.getByLabelText('Server name') as HTMLInputElement;
    fireEvent.change(field, { target: { value: NAME } });
    fireEvent.click(screen.getByLabelText('18 and over'));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    });
    expect(screen.getByRole('alert').textContent).toBe(MESSAGE);
    expect(field.getAttribute('aria-invalid')).toBe('true');
    expect(field.value).toBe(NAME);

    fireEvent.change(field, { target: { value: 'Evilname' } }); // editing clears it
    expect(screen.queryByText(MESSAGE)).toBeNull();
  });

  it('the profile step: the error is on the display name, and onboarding does not move on', async () => {
    const storage = memoryStorage();
    await new MockCore({ speed: 0, storage }).createIdentity({ locale: 'en', country: 'IL', birthDate: '2000-01-01' });
    localStorage.setItem('mvs.onboarding.v1', 'profile');
    const core = new MockCore({ speed: 0, storage });
    core.updateProfile = async () => {
      throw { code: 'bad-request' };
    };
    render(<App core={core} initialSettings={settings} />);
    const field = (await screen.findByLabelText('Display name')) as HTMLInputElement;
    fireEvent.change(field, { target: { value: NAME } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Done' }));
    });
    expect(screen.getByRole('alert').textContent).toBe(MESSAGE);
    expect(field.getAttribute('aria-invalid')).toBe('true');
    expect(field.value).toBe(NAME);
    expect(screen.getByLabelText('Display name')).toBe(field); // still on the profile step
  });
});
