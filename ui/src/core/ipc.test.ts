import { describe, expect, it, vi } from 'vitest';
import { createIpcCore, hasDesktopBridge, type DesktopBridge } from './ipc';

function fakeBridge(answer: (cmd: string, args: unknown[]) => { ok?: unknown; error?: { code: string } }) {
  let emit: (event: string, payload: unknown) => void = () => {};
  const bridge: DesktopBridge = {
    invoke: vi.fn(async (cmd: string, args: unknown[]) => answer(cmd, args)),
    onEvent: (cb) => {
      emit = cb;
      return () => {};
    },
  };
  return { bridge, emit: (e: string, p: unknown) => emit(e, p) };
}

describe('ipc core', () => {
  it('sends the command name and positional args', async () => {
    const { bridge } = fakeBridge(() => ({ ok: { total: 3 } }));
    const core = createIpcCore(bridge);
    await expect(core.getMessages('c1', 0, 10)).resolves.toEqual({ total: 3 });
    expect(bridge.invoke).toHaveBeenCalledWith('getMessages', ['c1', 0, 10]);
  });

  it('rejects with the core error object', async () => {
    const { bridge } = fakeBridge(() => ({ error: { code: 'invalid-words', positions: [2] } as never }));
    await expect(createIpcCore(bridge).restoreIdentity(['a'])).rejects.toEqual({ code: 'invalid-words', positions: [2] });
  });

  it('delivers events to the matching handlers until unsubscribed', () => {
    const { bridge, emit } = fakeBridge(() => ({ ok: null }));
    const core = createIpcCore(bridge);
    const seen: unknown[] = [];
    const off = core.on('server.removed', (p) => seen.push(p));
    emit('server.removed', { serverId: 's1' });
    emit('message.added', { id: 'm' });
    off();
    emit('server.removed', { serverId: 's2' });
    expect(seen).toEqual([{ serverId: 's1' }]);
  });

  it('keeps a link.open that arrives before anyone listens, for the first listener only', async () => {
    const { bridge, emit } = fakeBridge(() => ({ ok: null }));
    const core = createIpcCore(bridge);
    emit('link.open', { link: 'mvs://join/a' });
    emit('link.open', { link: 'mvs://join/b' });
    const first: unknown[] = [];
    const second: unknown[] = [];
    core.on('link.open' as never, (p: unknown) => first.push(p));
    core.on('link.open' as never, (p: unknown) => second.push(p));
    await new Promise((r) => setTimeout(r, 0));
    expect(first).toEqual([{ link: 'mvs://join/b' }]);
    expect(second).toEqual([]);
    emit('link.open', { link: 'mvs://join/c' });
    expect(first).toEqual([{ link: 'mvs://join/b' }, { link: 'mvs://join/c' }]);
    expect(second).toEqual([{ link: 'mvs://join/c' }]);
  });

  it('gives the latest core.status to every listener, also ones that come late (T8-01)', async () => {
    const { bridge, emit } = fakeBridge(() => ({ ok: null }));
    const core = createIpcCore(bridge);
    emit('core.status', { state: 'starting' });
    emit('core.status', { state: 'down', detail: 'crashed' });
    const a: unknown[] = [];
    const b: unknown[] = [];
    core.on('core.status' as never, (p: unknown) => a.push(p));
    core.on('core.status' as never, (p: unknown) => b.push(p));
    await new Promise((r) => setTimeout(r, 0));
    expect(a).toEqual([{ state: 'down', detail: 'crashed' }]);
    expect(b).toEqual([{ state: 'down', detail: 'crashed' }]);
    emit('core.status', { state: 'up' });
    expect(a.at(-1)).toEqual({ state: 'up' });
    expect(b.at(-1)).toEqual({ state: 'up' });
    // A listener added later still learns the current state.
    const c: unknown[] = [];
    core.on('core.status' as never, (p: unknown) => c.push(p));
    await new Promise((r) => setTimeout(r, 0));
    expect(c).toEqual([{ state: 'up' }]);
  });

  it('does not replay a stale core.status after a newer one arrived', async () => {
    const { bridge, emit } = fakeBridge(() => ({ ok: null }));
    const core = createIpcCore(bridge);
    emit('core.status', { state: 'down', detail: 'crashed' });
    const seen: unknown[] = [];
    core.on('core.status' as never, (p: unknown) => seen.push(p));
    emit('core.status', { state: 'up' });
    await new Promise((r) => setTimeout(r, 0));
    expect(seen).toEqual([{ state: 'up' }]);
  });

  it('is not a thenable', () => {
    const { bridge } = fakeBridge(() => ({ ok: null }));
    expect((createIpcCore(bridge) as unknown as { then?: unknown }).then).toBeUndefined();
  });

  it('detects the bridge', () => {
    expect(hasDesktopBridge()).toBe(false);
  });
});
