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

  it('is not a thenable', () => {
    const { bridge } = fakeBridge(() => ({ ok: null }));
    expect((createIpcCore(bridge) as unknown as { then?: unknown }).then).toBeUndefined();
  });

  it('detects the bridge', () => {
    expect(hasDesktopBridge()).toBe(false);
  });
});
