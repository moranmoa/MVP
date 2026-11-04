// In-app update in the UI (B8): the update.installed / update.rolledBack
// notes, and the two ways restartToUpdate can be refused.
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import { MockCore } from '../core/mock';
import { guardCore } from '../core/sanitize';
import type { CoreEventName, CoreEvents } from '../core/api';
import type { Settings } from '../app/settings';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

/** The mock's private emit, for driving events from a test. */
const emit = <E extends CoreEventName>(core: MockCore, event: E, payload: CoreEvents[E]) =>
  act(() => (core as unknown as { emit: (e: E, p: CoreEvents[E]) => void }).emit(event, payload));

async function shell(core: MockCore) {
  render(<App core={core} initialSettings={en} />);
  await screen.findByRole('navigation', { name: 'Servers' });
}

describe('update notes', () => {
  it('shows update.installed once, with the notes as plain text', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    await shell(core);
    emit(core, 'update.installed', { version: '1.3.0', notes: '<img src=x onerror=alert(1)>\nSecond line' });
    const status = await screen.findByText(/Updated to version/);
    expect(status.textContent).toContain('1.3.0');
    fireEvent.click(screen.getByText('What’s new'));
    const notes = document.querySelector('.update-notes')!;
    expect(notes.textContent).toBe('<img src=x onerror=alert(1)>\nSecond line');
    expect(notes.querySelector('img')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(screen.queryByText(/Updated to version/)).toBeNull();
  });

  it('shows update.rolledBack with both versions', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    await shell(core);
    emit(core, 'update.rolledBack', { failed: '1.3.0', running: '1.2.0' });
    const text = (await screen.findByText(/didn’t start properly/)).textContent!;
    expect(text).toContain('1.3.0');
    expect(text).toContain('1.2.0');
  });

  it('cleans the event payloads like other core data', () => {
    // A bare core whose events the test fires itself, wrapped like App does.
    const handlers = new Map<string, (p: unknown) => void>();
    const bare = { on: (e: string, h: (p: unknown) => void) => (handlers.set(e, h), () => {}) } as unknown as Parameters<typeof guardCore>[0];
    const core = guardCore(bare);
    const seen: unknown[] = [];
    core.on('update.installed', (p) => seen.push(p));
    core.on('update.rolledBack', (p) => seen.push(p));
    handlers.get('update.installed')!({ version: '1.3.0‮evil' + 'x'.repeat(100), notes: 'n'.repeat(5000) });
    handlers.get('update.installed')!({ version: '1.3.0', notes: 42 });
    handlers.get('update.rolledBack')!({ failed: 'y'.repeat(100), running: '1.2.0' });
    const [a, b, c] = seen as [{ version: string; notes: string }, { notes: unknown }, { failed: string; running: string }];
    expect(a.version.length).toBeLessThanOrEqual(32);
    expect(a.version).not.toContain('‮');
    expect(a.notes.length).toBeLessThanOrEqual(2001);
    expect(b.notes).toBeNull();
    expect(c.failed.length).toBeLessThanOrEqual(32);
    expect(c.running).toBe('1.2.0');
  });
});

describe('restart to update', () => {
  it('in a call (not-allowed) the pill stays and says it waits for the call', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    core.restartToUpdate = async () => {
      throw { code: 'not-allowed' };
    };
    await shell(core);
    emit(core, 'update.ready', { version: '1.3.0', waitingForCall: false });
    fireEvent.click(await screen.findByRole('button', { name: /Update ready – restart/ }));
    expect(await screen.findByText('Update ready – installs when the call ends')).toBeTruthy();
  });

  it('with nothing staged the pill goes away', async () => {
    const core = new MockCore({ demo: true, speed: 0 });
    core.restartToUpdate = async () => {
      throw { code: 'nothing-staged' };
    };
    await shell(core);
    emit(core, 'update.ready', { version: '1.3.0', waitingForCall: false });
    fireEvent.click(await screen.findByRole('button', { name: /Update ready – restart/ }));
    await waitFor(() => expect(screen.queryByText(/Update ready/)).toBeNull());
  });
});
