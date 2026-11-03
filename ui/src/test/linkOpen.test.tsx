// B1-3 (#78): an mvs:// link from the OS opens the join dialog pre-filled and
// never joins on its own; only the Join button does.

import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import type { CoreEvents } from '../core/api';
import { MockCore } from '../core/mock';
import { cleanInviteLink } from '../core/sanitize';

const settings: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const LINK = `mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}/${'e'.repeat(64)}`;

async function setup() {
  const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
  const join = vi.spyOn(core, 'joinServer');
  render(<App core={core} initialSettings={settings} />);
  await screen.findByRole('textbox', { name: /^Message / });
  const openLink = (link: string) =>
    act(() => (core as unknown as { emit: (e: 'link.open', p: CoreEvents['link.open']) => void }).emit('link.open', { link }));
  return { join, openLink };
}

describe('mvs:// links from the OS', () => {
  it('open the join dialog pre-filled, and join only after the Join button', async () => {
    const { join, openLink } = await setup();
    openLink(LINK.toUpperCase().replace('MVS://JOIN', 'mvs://join')); // canonicalised to lower case
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(field.value).toBe(LINK);
    expect(join).not.toHaveBeenCalled();

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Join' }));
    });
    expect(join).toHaveBeenCalledTimes(1);
    expect(join).toHaveBeenCalledWith(LINK);
  });

  it('do not swap the link while the join dialog is open', async () => {
    const { join, openLink } = await setup();
    openLink(LINK);
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    const other = `mvs://join/${'c'.repeat(64)}/${'d'.repeat(64)}/${'f'.repeat(64)}`;
    openLink(other);
    expect(field.value).toBe(LINK);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Join' }));
    });
    expect(join).toHaveBeenCalledWith(LINK);
    expect(join).not.toHaveBeenCalledWith(other);
  });

  it('ignore anything that is not exactly an invite link', async () => {
    const { join, openLink } = await setup();
    for (const bad of [`${LINK}?--inspect`, 'mvs://join/xyz', 'https://evil.example/join', `mvs://evil/${'a'.repeat(64)}/${'b'.repeat(64)}/${'e'.repeat(64)}`, `mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}`]) {
      openLink(bad);
    }
    expect(screen.queryByLabelText('Invite link')).toBeNull();
    expect(join).not.toHaveBeenCalled();
  });

  it('cleanInviteLink accepts only the canonical format', () => {
    expect(cleanInviteLink(` ${LINK}/ `)).toBe(LINK);
    expect(cleanInviteLink(`${LINK}x`)).toBe('');
    expect(cleanInviteLink(42)).toBe('');
    expect(cleanInviteLink(LINK + ' '.repeat(300))).toBe('');
  });
});
