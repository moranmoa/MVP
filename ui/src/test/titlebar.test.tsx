// Our own window frame: the title bar is drawn only when the desktop shell
// gives us a frameless window (`window.mvs.windowControl`).

import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import type { CoreEvents } from '../core/api';
import { MockCore } from '../core/mock';

async function setup(locale: 'en' | 'he' = 'en') {
  const settings: Settings = { locale, theme: 'light', uiScale: 1, fontSize: 15 };
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  render(<App core={core} initialSettings={settings} />);
  await screen.findAllByRole('textbox');
  const emit = (p: CoreEvents['window.state']) =>
    act(() => (core as unknown as { emit: (e: 'window.state', p: CoreEvents['window.state']) => void }).emit('window.state', p));
  return { emit };
}

describe('title bar', () => {
  afterEach(() => {
    delete (window as { mvs?: unknown }).mvs;
  });

  it('is not drawn without the desktop shell', async () => {
    await setup();
    expect(document.querySelector('.titlebar')).toBeNull();
  });

  it('minimizes, maximizes and closes through the shell, and follows the window state', async () => {
    const windowControl = vi.fn(async (a: string) => (a === 'close' || a === 'minimize' ? { maximized: false } : { maximized: a === 'maximize' }));
    (window as { mvs?: unknown }).mvs = { windowControl };
    const { emit } = await setup();
    const bar = document.querySelector('.titlebar')!;
    expect(bar.textContent).toContain('MVS');
    expect(windowControl).toHaveBeenCalledWith('state');

    fireEvent.click(screen.getByRole('button', { name: 'Minimize' }));
    expect(windowControl).toHaveBeenLastCalledWith('minimize');
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Maximize' }));
    });
    expect(windowControl).toHaveBeenLastCalledWith('maximize');
    expect(screen.getByRole('button', { name: 'Restore' })).toBeTruthy();
    // Restored from the OS (e.g. a drag off the top of the screen).
    emit({ maximized: false });
    expect(screen.getByRole('button', { name: 'Maximize' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(windowControl).toHaveBeenLastCalledWith('close');
  });

  it('speaks Hebrew in Hebrew', async () => {
    (window as { mvs?: unknown }).mvs = { windowControl: vi.fn(async () => ({ maximized: false })) };
    await setup('he');
    expect(screen.getByRole('button', { name: 'מזעור' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'סגירה' })).toBeTruthy();
  });
});
