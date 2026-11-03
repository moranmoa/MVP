// B17 / UX-01: the direction of user text follows the majority of its letters,
// and the text stays on the interface side.

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../app/App';
import type { Settings } from '../app/settings';
import { paragraphs, textDir } from '../app/textDir';
import { MockCore } from '../core/mock';

describe('textDir', () => {
  it.each([
    ['lol זה היה מטורף', 'rtl'],
    ['GG על המשחק אתמול, 3:1 לנו!', 'rtl'],
    ['@Itamar תבדוק את ה-patch החדש', 'rtl'],
    ['version 2.4.1 fixed the crash', 'ltr'],
    ['https://youtu.be/abcdefg תראו את זה', 'rtl'],
    ['`npm run build` ואז לבדוק', 'rtl'],
    ['the server was laggy for me yesterday, anyone else?', 'ltr'],
    ['ok אחי', 'rtl'],
  ] as const)('%s → %s in either interface', (text, dir) => {
    expect(textDir(text, 'rtl')).toBe(dir);
    expect(textDir(text, 'ltr')).toBe(dir);
  });

  it('falls back to the interface direction when nothing decides', () => {
    for (const text of ['🎉🎉', '3:1', '', 'ab אב']) {
      expect(textDir(text, 'rtl')).toBe('rtl');
      expect(textDir(text, 'ltr')).toBe('ltr');
    }
  });

  it('lets a leading RLM or LRM decide', () => {
    expect(textDir(String.fromCharCode(0x200f) + 'ok', 'ltr')).toBe('rtl');
    expect(textDir(String.fromCharCode(0x200e) + 'שלום', 'rtl')).toBe('ltr');
  });

  it('gives each paragraph its own direction', () => {
    expect(paragraphs('שלום לכולם' + String.fromCharCode(10) + 'npm run build', 'rtl')).toEqual([
      { text: 'שלום לכולם', dir: 'rtl' },
      { text: 'npm run build', dir: 'ltr' },
    ]);
  });

  it('counts Arabic as right to left', () => {
    expect(textDir('مرحبا hi', 'ltr')).toBe('rtl');
  });
});

describe('messages in the chat', () => {
  const settings = (locale: 'he' | 'en'): Settings => ({ locale, theme: 'light', uiScale: 1, fontSize: 15 });

  it('render a Hebrew message that starts with English as RTL, aligned with the interface', async () => {
    const core = new MockCore({ demo: true, speed: 0, simulateActivity: false });
    const base = core.getMessages.bind(core);
    core.getMessages = async (c, s, e) => {
      const r = await base(c, s, e);
      return { ...r, messages: r.messages.map((m) => (m.index === 0 ? { ...m, body: 'lol זה היה מטורף', deleted: false, replyTo: undefined } : m)) };
    };
    render(<App core={core} initialSettings={settings('en')} />);
    const body = await screen.findByText('lol זה היה מטורף');
    expect(body.classList.contains('msg-line')).toBe(true);
    expect(body.getAttribute('dir')).toBe('rtl');
    expect(body.classList.contains('user-text')).toBe(true);
  });

  it('start the empty composer in the interface direction', async () => {
    render(<App core={new MockCore({ demo: true, speed: 0 })} initialSettings={settings('he')} />);
    const box = await screen.findByRole('textbox', { name: /^כתיבה ב-/ });
    expect(box.getAttribute('dir')).toBe('rtl');
  });
});
