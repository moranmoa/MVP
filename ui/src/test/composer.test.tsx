// QA-10: the composer shows the length limit instead of cutting a paste
// silently, never sends past it, and keeps the text when the core refuses it.

import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };

async function setup(core = new MockCore({ demo: true, speed: 0, simulateActivity: false })) {
  render(<App core={core} initialSettings={en} />);
  const box = (await screen.findByRole('textbox', { name: /^Message / })) as HTMLTextAreaElement;
  const send = screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement;
  return { core, box, send };
}

describe('composer length limit (QA-10)', () => {
  it('shows no counter for normal messages', async () => {
    const { box } = await setup();
    fireEvent.change(box, { target: { value: 'hello' } });
    expect(document.querySelector('.char-count')).toBeNull();
  });

  it('shows a counter near the limit, and keeps sending allowed', async () => {
    const { box, send } = await setup();
    fireEvent.change(box, { target: { value: 'x'.repeat(3600) } });
    expect(screen.getByText('3,600 / 4,000')).toBeTruthy();
    expect(send.disabled).toBe(false);
  });

  it('keeps a long paste whole, blocks sending and says why', async () => {
    const { core, box, send } = await setup();
    const before = await core.getMessageCount('c-general');
    const hebrew = 'א'.repeat(8200); // the QA case: 16,400 bytes of UTF-8
    fireEvent.change(box, { target: { value: hebrew } });
    expect(box.value.length).toBe(8200); // nothing cut silently
    expect(send.disabled).toBe(true);
    expect(box.getAttribute('aria-invalid')).toBe('true');
    expect(screen.getByRole('alert').textContent).toContain('This message is too long');
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    expect(await core.getMessageCount('c-general')).toBe(before);
    expect(box.value.length).toBe(8200);
  });

  it('gives the text back when the core refuses the message', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    core.sendMessage = async () => {
      throw { code: 'not-allowed' };
    };
    const { box } = await setup(core);
    fireEvent.change(box, { target: { value: 'keep me' } });
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    expect(box.value).toBe('keep me');
    expect(screen.getByRole('alert').textContent).toContain("The message wasn't sent");
  });
});
