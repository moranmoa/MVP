// B1: createCore() talks to the real core over the desktop bridge when there
// is one, and keeps the mock everywhere else.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { createCore } from '../core';
import type { DesktopBridge } from '../core/ipc';
import { MockCore } from '../core/mock';
import { guardCore } from '../core/sanitize';

const RLO = String.fromCharCode(0x202e);

afterEach(() => {
  delete window.mvs;
});

describe('createCore', () => {
  it('uses the mock without a desktop bridge, ?demo included', async () => {
    expect(createCore('')).toBeInstanceOf(MockCore);
    const demo = createCore('?demo=1');
    expect(demo).toBeInstanceOf(MockCore);
    expect((await demo.listServers()).length).toBeGreaterThan(0);
  });

  it('uses the bridge in the desktop app, and guardCore cleans what comes back', async () => {
    const invoke = vi.fn<DesktopBridge['invoke']>(async (cmd) =>
      cmd === 'listMembers'
        ? { ok: [{ id: 'u-1', serverId: 's-1', displayName: `Eve${RLO}nimda`, shortCode: 'AAAA-BBBB', avatarColor: '#123456', presence: 'online', isOwner: false }] }
        : { error: { code: 'not-found' } },
    );
    window.mvs = { invoke, onEvent: () => () => {} };

    const core = createCore('?demo=1'); // URL flags don't matter in the desktop app
    expect(core).not.toBeInstanceOf(MockCore);

    const [member] = await guardCore(core).listMembers('s-1');
    expect(invoke).toHaveBeenCalledWith('listMembers', ['s-1']);
    expect(member.displayName).toBe('Evenimda'); // the direction override is gone
    await expect(core.getIdentity()).rejects.toMatchObject({ code: 'not-found' });
  });
});
