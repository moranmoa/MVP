import { useEffect, useId, useRef, type ReactNode } from 'react';
import { useIntl } from 'react-intl';
import { CloseIcon } from './icons';

/** User-controlled names are isolated so they never reorder the sentence around them. */
export function Name({ children, className }: { children: string; className?: string }) {
  return <bdi className={className ?? 'name'}>{children}</bdi>;
}

/** Identity codes and recovery words read left to right in every language. */
export function Code({ value, label }: { value: string; label?: string }) {
  return (
    <span className="code" dir="ltr" aria-label={label} title={label}>
      {value}
    </span>
  );
}

export function Avatar({
  name,
  color,
  src,
  size = 40,
  presence,
}: {
  name: string;
  color: string;
  src?: string;
  size?: number;
  presence?: 'online' | 'offline';
}) {
  const letter = [...(name.trim() || '?')][0].toUpperCase();
  return (
    <span className="avatar" style={{ width: `${size / 16}rem`, height: `${size / 16}rem`, background: color }} aria-hidden="true">
      {src ? <img src={src} alt="" /> : <span style={{ fontSize: `${size / 40}rem` }}>{letter}</span>}
      {presence && <span className={`presence presence-${presence}`} />}
    </span>
  );
}

/**
 * Modal built on <dialog>: the browser handles focus trapping, Esc and the
 * inert background. `onClose` fires for Esc, the close button and backdrop clicks.
 */
export function Dialog({
  open,
  onClose,
  title,
  children,
  className,
  hideClose,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  className?: string;
  hideClose?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const intl = useIntl();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      className={['dialog', className].filter(Boolean).join(' ')}
      aria-labelledby={titleId}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onMouseDown={(e) => {
        if (e.target === ref.current) onClose();
      }}
    >
      {open && (
        <div className="dialog-body">
          <header className="dialog-header">
            <h2 id={titleId}>{title}</h2>
            {!hideClose && (
              <button type="button" className="icon-button" onClick={onClose} aria-label={intl.formatMessage({ id: 'common.close' })}>
                <CloseIcon />
              </button>
            )}
          </header>
          {children}
        </div>
      )}
    </dialog>
  );
}

export function Badge({ count, label }: { count: number; label: string }) {
  if (count <= 0) return null;
  return (
    <span className="badge" aria-label={label} title={label}>
      {count > 99 ? '99+' : count}
    </span>
  );
}
