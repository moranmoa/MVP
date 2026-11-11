// Our own title bar (docs/ux/specs/desktop-chrome.md §2): drawn only when the
// desktop shell gives us our own frame (`window.mvs.windowFrame`).

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import type { CoreEvents } from '../core/api';
import { MockCore } from '../core/mock';

async function setup(locale: 'en' | 'he' = 'en', theme: Settings['theme'] = 'light') {
  const settings: Settings = { locale, theme, uiScale: 1, fontSize: 15 };
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  render(<App core={core} initialSettings={settings} />);
  await screen.findAllByRole('textbox');
  const emit = <E extends 'window.state' | 'window.closeRequest'>(e: E, p: CoreEvents[E]) =>
    act(() => (core as unknown as { emit: (e: E, p: CoreEvents[E]) => void }).emit(e, p));
  return { core, emit };
}

function shell(frame: 'overlay' | 'custom') {
  const windowControl = vi.fn(async () => true);
  (window as { mvs?: unknown }).mvs = { windowFrame: frame, windowControl };
  return windowControl;
}

describe('title bar', () => {
  afterEach(() => {
    delete (window as { mvs?: unknown }).mvs;
  });

  it('is not drawn without the desktop shell', async () => {
    await setup();
    expect(document.querySelector<HTMLElement>('.titlebar')).toBeNull();
  });

  it('Linux: shows the mark, MVS and the open server, and its own three buttons', async () => {
    const windowControl = shell('custom');
    const { emit } = await setup();
    const bar = document.querySelector<HTMLElement>('.titlebar')!;
    expect(bar.textContent).toBe('MVS · גיימרים ישראל');
    expect(bar.querySelector('img.titlebar-mark')).not.toBeNull();
    // The buttons keep the physical order in every language.
    expect(bar.querySelector('.titlebar-controls')!.getAttribute('dir')).toBe('ltr');

    fireEvent.click(within(bar).getByRole('button', { name: 'Minimize' }));
    expect(windowControl).toHaveBeenLastCalledWith('minimize');
    fireEvent.click(within(bar).getByRole('button', { name: 'Maximize' }));
    expect(windowControl).toHaveBeenLastCalledWith('toggle-maximize');
    emit('window.state', { maximized: true, fullScreen: false });
    expect(within(bar).getByRole('button', { name: 'Restore' })).toBeTruthy();
    fireEvent.click(within(bar).getByRole('button', { name: 'Close' }));
    expect(windowControl).toHaveBeenLastCalledWith('close');

    // Fullscreen hides the bar.
    emit('window.state', { maximized: false, fullScreen: true });
    expect(document.querySelector<HTMLElement>('.titlebar')).toBeNull();
  });

  it('Windows: no buttons of ours (the OS overlay draws them); the overlay follows the theme', async () => {
    const windowControl = shell('overlay');
    await setup('en', 'dark');
    expect(within(document.querySelector<HTMLElement>('.titlebar')!).queryAllByRole('button')).toHaveLength(0);
    expect(windowControl).toHaveBeenCalledWith('theme:dark');
  });

  it('a close during a call asks first; Cancel keeps the app open', async () => {
    const windowControl = shell('overlay');
    const { emit } = await setup('he');
    emit('window.closeRequest', {});
    const dialog = screen.getByRole('dialog', { name: 'השיחה תסתיים. לסגור את MVS?' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'ביטול' }));
    expect(windowControl).not.toHaveBeenCalledWith('close');
    emit('window.closeRequest', {});
    fireEvent.click(screen.getByRole('dialog').querySelector('button.danger')!);
    expect(windowControl).toHaveBeenLastCalledWith('close');
  });
});
