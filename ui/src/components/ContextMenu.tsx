// B25 (spec docs/ux/specs/B25-context-menus.md): one menu component for every
// right-click menu. Opens on right-click, a 500 ms long press, or Shift+F10 /
// the Menu key on the focused row. Closes on Esc, a click outside, scroll,
// resize, or choosing an item; focus then goes back to the row. At phone
// width (the shell's phone layout) it is a bottom sheet instead (B25 §5),
// with the same keys and focus.

import { createPortal } from 'react-dom';
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type PointerEvent, type ReactNode } from 'react';
import { phoneQuery, useMediaQuery } from '../app/media';
import { useSettings } from '../app/settings';

export type MenuItem =
  | { kind?: 'item'; key: string; label: string; icon?: ReactNode; danger?: boolean; onSelect: () => void }
  | { kind: 'separator' };

/** Drops leading, trailing and doubled separators, so callers can filter items freely. */
function tidy(items: MenuItem[]): MenuItem[] {
  const out: MenuItem[] = [];
  for (const it of items) {
    if (it.kind === 'separator' && (out.length === 0 || out[out.length - 1].kind === 'separator')) continue;
    out.push(it);
  }
  while (out.length && out[out.length - 1].kind === 'separator') out.pop();
  return out;
}

export function ContextMenu({
  x,
  y,
  label,
  items,
  onClose,
}: {
  x: number;
  y: number;
  label: string;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  const sheet = useMediaQuery(phoneQuery(useSettings().settings.uiScale));

  // Opens at the point, towards the end side; flips to stay inside the window.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    if (sheet) return;
    const { width, height } = el.getBoundingClientRect();
    const rtl = getComputedStyle(el).direction === 'rtl';
    let left = rtl ? x - width : x;
    if (left + width > window.innerWidth) left = x - width;
    if (left < 0) left = Math.min(x, window.innerWidth - width);
    let top = y;
    if (top + height > window.innerHeight) top = y - height;
    setPos({ left: Math.max(0, left), top: Math.max(0, top) });
  }, [x, y, sheet]);

  useEffect(() => {
    const outside = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const close = () => onClose();
    document.addEventListener('mousedown', outside, true);
    document.addEventListener('touchstart', outside, true);
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', close);
    window.addEventListener('blur', close);
    return () => {
      document.removeEventListener('mousedown', outside, true);
      document.removeEventListener('touchstart', outside, true);
      window.removeEventListener('scroll', close, true);
      window.removeEventListener('resize', close);
      window.removeEventListener('blur', close);
    };
  }, [onClose]);

  const onKeyDown = (e: KeyboardEvent) => {
    const els = [...(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const i = els.indexOf(document.activeElement as HTMLElement);
    let next: HTMLElement | undefined;
    if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    } else if (e.key === 'ArrowDown') next = els[(i + 1) % els.length];
    else if (e.key === 'ArrowUp') next = els[(i - 1 + els.length) % els.length];
    else if (e.key === 'Home') next = els[0];
    else if (e.key === 'End') next = els[els.length - 1];
    else if (e.key.length === 1 && /\S/.test(e.key)) {
      // A letter jumps to the next item that starts with it.
      const k = e.key.toLocaleLowerCase();
      const order = [...els.slice(i + 1), ...els.slice(0, i + 1)];
      next = order.find((el) => el.textContent?.trim().toLocaleLowerCase().startsWith(k));
    } else return;
    e.preventDefault();
    next?.focus();
  };

  const shown = tidy(items);
  const menu = (
    <div
      ref={ref}
      className={sheet ? 'context-menu sheet' : 'context-menu'}
      role="menu"
      aria-label={label}
      style={sheet ? undefined : { left: pos.left, top: pos.top }}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
    >
      {sheet && (
        <div className="context-menu-title" aria-hidden="true">
          {label}
        </div>
      )}
      {shown.map((it, i) =>
        it.kind === 'separator' ? (
          <div key={`sep-${i}`} role="separator" className="context-menu-sep" />
        ) : (
          <button
            key={it.key}
            type="button"
            role="menuitem"
            tabIndex={-1}
            className={it.danger ? 'danger-item' : undefined}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            {it.icon && <span className="context-menu-icon" aria-hidden="true">{it.icon}</span>}
            <span>{it.label}</span>
          </button>
        ),
      )}
    </div>
  );
  // The backdrop is outside the menu, so a tap on it is an outside click.
  return sheet ? (
    <>
      <div className="context-menu-backdrop" aria-hidden="true" />
      {menu}
    </>
  ) : (
    menu
  );
}

type Open = { x: number; y: number; label: string; items: MenuItem[]; from: HTMLElement | null };

const LONG_PRESS_MS = 500;
const MOVE_CANCEL_PX = 10;

/**
 * Wires rows to one menu. `bind(label, items)` returns the props for a row;
 * `menu` is the element to render once. A row with no items gets no menu and
 * keeps the system one.
 */
export function useContextMenu() {
  const [open, setOpen] = useState<Open | null>(null);
  const press = useRef<{ timer: number; x: number; y: number } | null>(null);

  const show = useCallback((from: HTMLElement | null, x: number, y: number, label: string, items: MenuItem[]) => {
    if (!tidy(items).some((it) => it.kind !== 'separator')) return false;
    setOpen({ x, y, label, items, from });
    return true;
  }, []);

  const close = useCallback(() => {
    setOpen((o) => {
      // Back to the row, unless an item moved focus somewhere else (a dialog).
      const from = o?.from;
      if (from) setTimeout(() => {
        if (!document.activeElement || document.activeElement === document.body) from.focus();
      });
      return null;
    });
  }, []);

  const cancelPress = () => {
    if (press.current) clearTimeout(press.current.timer);
    press.current = null;
  };

  const bind = (label: string, items: () => MenuItem[]) => ({
    'aria-haspopup': 'menu' as const,
    onContextMenu: (e: MouseEvent<HTMLElement>) => {
      // Selected text keeps the system menu, so copy still works.
      if (window.getSelection()?.toString()) return;
      if (show(e.currentTarget, e.clientX, e.clientY, label, items())) e.preventDefault();
    },
    onKeyDown: (e: KeyboardEvent<HTMLElement>) => {
      if (!((e.shiftKey && e.key === 'F10') || e.key === 'ContextMenu')) return;
      const r = e.currentTarget.getBoundingClientRect();
      if (show(e.currentTarget, r.left + r.width / 2, r.bottom, label, items())) e.preventDefault();
    },
    onPointerDown: (e: PointerEvent<HTMLElement>) => {
      if (e.pointerType !== 'touch') return;
      cancelPress();
      const target = e.currentTarget;
      const { clientX: x, clientY: y } = e;
      press.current = {
        x,
        y,
        timer: window.setTimeout(() => {
          press.current = null;
          if (show(target, x, y, label, items())) navigator.vibrate?.(10);
        }, LONG_PRESS_MS),
      };
    },
    onPointerMove: (e: PointerEvent<HTMLElement>) => {
      const p = press.current;
      if (p && Math.hypot(e.clientX - p.x, e.clientY - p.y) > MOVE_CANCEL_PX) cancelPress();
    },
    onPointerUp: cancelPress,
    onPointerCancel: cancelPress,
  });

  // In <body>, so no ancestor's overflow or transform clips or moves it.
  const menu = open && createPortal(<ContextMenu x={open.x} y={open.y} label={open.label} items={open.items} onClose={close} />, document.body);
  return { bind, menu };
}
