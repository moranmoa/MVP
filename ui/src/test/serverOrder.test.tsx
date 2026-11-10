// G-01 (docs/ux/GAPS.md §2.1): reorder my servers in the rail, on this computer only.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { App } from '../app/App';
import { cleanOrder, moveTo, sortByOrder } from '../app/serverOrder';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const realMatchMedia = window.matchMedia;
afterEach(() => {
  window.matchMedia = realMatchMedia;
});

const rail = () => screen.getByRole('navigation', { name: /servers/i });
const serverButtons = () => [...rail().querySelectorAll<HTMLButtonElement>('button[data-server-id]')];
const order = () => serverButtons().map((b) => b.title);

async function setup(initialSettings: Settings = en) {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  render(<App core={core} initialSettings={initialSettings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  return core;
}

describe('server order helpers', () => {
  it('sorts by my order; servers not in it go at the end, in join order', () => {
    const s = ['a', 'b', 'c', 'd'].map((id) => ({ id }));
    expect(sortByOrder(s, ['c', 'a']).map((x) => x.id)).toEqual(['c', 'a', 'b', 'd']);
    expect(sortByOrder(s, ['gone', 'd']).map((x) => x.id)).toEqual(['d', 'a', 'b', 'c']);
  });

  it('moves and clamps; a stored order that is not a list of ids is no order', () => {
    expect(moveTo(['a', 'b', 'c'], 'c', 0)).toEqual(['c', 'a', 'b']);
    expect(moveTo(['a', 'b', 'c'], 'a', 9)).toEqual(['b', 'c', 'a']);
    expect(cleanOrder('a,b')).toEqual([]);
    expect(cleanOrder(['a', 3, '', 'x'.repeat(200), 'b'])).toEqual(['a', 'b']);
  });
});

describe('server order in the rail (G-01)', () => {
  it('Alt+Shift+Down moves the focused server, says where it went, and the order survives a reload', async () => {
    await setup();
    const before = order();
    const first = serverButtons()[0];
    first.focus();
    await act(async () => {
      fireEvent.keyDown(first, { key: 'ArrowDown', altKey: true, shiftKey: true });
    });
    expect(order()).toEqual([before[1], before[0], ...before.slice(2)]);
    expect(document.activeElement?.getAttribute('title')).toBe(before[0]);
    expect(rail().querySelector('[aria-live]')?.textContent).toBe(`⁨${before[0]}⁩ moved to position ⁨2⁩ of ⁨${before.length}⁩`);
    // In the local settings only; a fresh start reads it back from there.
    const ids = serverButtons().map((b) => b.dataset.serverId);
    expect(JSON.parse(localStorage.getItem('mvp.settings.v1')!).serverOrder).toEqual(ids);
    cleanup();
    render(<App core={new MockCore({ demo: true, speed: 0, simulateActivity: false })} />);
    await screen.findByRole('textbox', { name: /^Message / });
    expect(order()).toEqual([before[1], before[0], ...before.slice(2)]);
  });

  it('the menu has Move up / Move down, but not past the ends', async () => {
    await setup();
    const buttons = serverButtons();
    fireEvent.contextMenu(buttons[0]);
    expect(screen.queryByRole('menuitem', { name: 'Move up' })).toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Move down' })).toBeTruthy();
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    fireEvent.contextMenu(buttons[buttons.length - 1]);
    expect(screen.getByRole('menuitem', { name: 'Move up' })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: 'Move down' })).toBeNull();
    const before = order();
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Move up' }));
    });
    expect(order().at(-2)).toBe(before.at(-1));
  });

  it('a new server comes at the end of my order', async () => {
    const core = await setup();
    const first = serverButtons()[0];
    first.focus();
    await act(async () => {
      fireEvent.keyDown(first, { key: 'ArrowDown', altKey: true, shiftKey: true });
    });
    await act(async () => {
      await core.createServer({ name: 'Brand new', ageRestriction: 'none' } as never);
    });
    expect(order().at(-1)).toBe('Brand new');
  });

  it('phone: long-press menu → Arrange servers → arrows next to each server → Done', async () => {
    window.matchMedia = ((query: string) => ({
      matches: true,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })) as typeof window.matchMedia;
    await setup();
    const before = order();
    fireEvent.contextMenu(serverButtons()[0]);
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Arrange servers' }));
    });
    const nav = rail();
    expect(within(nav).getByRole('button', { name: `Move up: ${before[0]}` }).getAttribute('aria-disabled')).toBe('true'); // stays focusable (UX)
    await act(async () => {
      fireEvent.click(within(nav).getByRole('button', { name: `Move down: ${before[0]}` }));
    });
    expect(order()).toEqual([before[1], before[0], ...before.slice(2)]);
    await act(async () => {
      fireEvent.click(within(nav).getByRole('button', { name: 'Done' }));
    });
    expect(within(nav).queryByRole('button', { name: /^Move down/ })).toBeNull();
  });
});
