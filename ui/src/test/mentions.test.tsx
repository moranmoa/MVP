// G-24 (#330): mention someone with "@", and see mentions as names.

import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import { mentionQuery, splitMentions, toDisplay, toTokens } from '../app/mentions';
import type { Settings } from '../app/settings';
import { MockCore } from '../core/mock';

const en: Settings = { locale: 'en', theme: 'light', uiScale: 1, fontSize: 15 };
const HEX = 'b'.repeat(64);

describe('mention helpers', () => {
  it('split, show as names and turn back into tokens', () => {
    expect(splitMentions(`hi <@${HEX}>!`)).toEqual([{ text: 'hi ' }, { mention: HEX }, { text: '!' }]);
    const map = new Map<string, string>();
    const shown = toDisplay(`hi <@${HEX}> and <@gone>`, (id) => (id === HEX ? 'Dana' : undefined), 'former account', map);
    expect(shown).toBe('hi @Dana and @former account');
    expect(toTokens(shown, map)).toBe(`hi <@${HEX}> and <@gone>`);
  });

  it('finds the "@query" before the caret, only at a word start', () => {
    expect(mentionQuery('hello @da', 9)).toEqual({ start: 6, query: 'da' });
    expect(mentionQuery('@', 1)).toEqual({ start: 0, query: '' });
    expect(mentionQuery('mail a@b', 8)).toBeNull();
  });
});

describe('mentions in the chat (G-24)', () => {
  async function setup() {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    // A short #general, so a new message is on screen in jsdom.
    (core as unknown as { channelIndex: Map<string, { baseCount: number }> }).channelIndex.get('c-general')!.baseCount = 3;
    render(<App core={core} initialSettings={en} />);
    const box = (await screen.findByRole('textbox', { name: /^Message / })) as HTMLTextAreaElement;
    return { core, box };
  }
  const typeIn = (box: HTMLTextAreaElement, value: string) => {
    fireEvent.change(box, { target: { value, selectionStart: value.length } });
  };

  it('"@" suggests members; Enter picks one; the message carries a token, the box only the name', async () => {
    const { core, box } = await setup();
    typeIn(box, 'hey @Ta');
    const list = screen.getByRole('listbox', { name: 'Mention suggestions' });
    const option = within(list).getAllByRole('option')[0];
    expect(option.textContent).toContain('Tal');
    expect(box.getAttribute('aria-activedescendant')).toBe(option.id);
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    expect(box.value).toBe('hey @Tal ');
    expect(box.value).not.toMatch(/<@/);
    typeIn(box, box.value + 'ready?');
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    const sent = (await core.getMessages('c-general', 3, 4)).messages[0];
    expect(sent.body).toMatch(/^hey <@[^>]+> ready\?$/);
    expect(sent.mentions).toHaveLength(1);
    // Shown as a chip with the name; never the id.
    const chip = await screen.findByText('@Tal');
    expect(chip.classList.contains('mention-chip')).toBe(true);
    expect(document.body.textContent).not.toContain(sent.mentions![0] + '>');
  });

  it('a token for someone who is not a member shows as a former account, never the raw id', async () => {
    const { core, box } = await setup();
    typeIn(box, `about <@${HEX}> yesterday`);
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    const chip = await screen.findByText('@former account');
    expect(chip.classList.contains('former')).toBe(true);
    expect(document.body.textContent).not.toContain(HEX);
    void core;
  });

  it('Esc closes the suggestions without leaving the composer', async () => {
    const { box } = await setup();
    typeIn(box, '@');
    expect(screen.getByRole('listbox')).toBeTruthy();
    fireEvent.keyDown(box, { key: 'Escape' });
    expect(screen.queryByRole('listbox')).toBeNull();
  });
});
