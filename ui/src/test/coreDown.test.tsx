// T8-01: "core unavailable" screen. The desktop shell reports core.status and
// rejects commands with core-unavailable while the core is down.

import { act, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import type { CoreEvents } from '../core/api';
import { MockCore } from '../core/mock';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const emit = (core: MockCore, payload: CoreEvents['core.status']) =>
  act(() => (core as unknown as { emit: (e: 'core.status', p: CoreEvents['core.status']) => void }).emit('core.status', payload));

describe('core unavailable', () => {
  it('shows a full screen while the core is down, per detail, and leaves when it is up', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    render(<App core={core} initialSettings={settings} />);
    await screen.findByRole('textbox', { name: /^Message / });

    emit(core, { state: 'down', detail: 'crash-loop' });
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain("MVS can't run right now");
    expect(alert.textContent).toContain('Trying again every 30 seconds');
    expect(screen.queryByRole('textbox', { name: /^Message / })).toBeNull();

    emit(core, { state: 'up' });
    expect(screen.queryByText("MVS can't run right now")).toBeNull();
    expect(await screen.findByRole('textbox', { name: /^Message / })).toBeTruthy();
  });

  it('shows it when a command is refused with core-unavailable, even at start', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    core.getIdentity = async () => {
      throw { code: 'core-unavailable', detail: 'missing' };
    };
    render(<App core={core} initialSettings={settings} />);
    expect((await screen.findByRole('alert')).textContent).toContain('Reinstalling MVS should fix it.');
  });
});
