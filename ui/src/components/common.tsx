import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { copyText } from '../app/clipboard';
import { safeColor, safeImageSrc } from '../core/sanitize';
import { CloseIcon } from './icons';

/** User-controlled names are isolated so they never reorder the sentence around them. */
export function Name({ children, className }: { children: string; className?: string }) {
  return <bdi className={className ?? 'name'}>{children}</bdi>;
}

/**
 * An invite link: shown as selectable text (one click selects all of it, so
 * Ctrl+C always works) with a Copy button that says whether it worked.
 */
export function InviteBox({ link }: { link: string }) {
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle');
  return (
    <div className="stack-tight">
      <div className="invite-box">
        <span className="code invite-link" dir="ltr">
          {link}
        </span>
        <button
          type="button"
          className="secondary small"
          onClick={async () => setState((await copyText(link)) ? 'copied' : 'failed')}
        >
          <FormattedMessage id={state === 'copied' ? 'common.copied' : 'common.copy'} />
        </button>
      </div>
      <p className={state === 'failed' ? 'error small' : 'visually-hidden'} role="status">
        {state === 'failed' && <FormattedMessage id="common.copyFailed" values={RICH} />}
        {state === 'copied' && <FormattedMessage id="common.copied" />}
      </p>
    </div>
  );
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
    <span className="avatar" style={{ width: `${size / 16}rem`, height: `${size / 16}rem`, background: safeColor(color) }} aria-hidden="true">
      {safeImageSrc(src) ? <img src={safeImageSrc(src)} alt="" /> : <span style={{ fontSize: `${size / 40}rem` }}>{letter}</span>}
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
  describedBy,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  className?: string;
  hideClose?: boolean;
  /** The id of the text that explains the dialog, read with its title. */
  describedBy?: string;
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
      aria-describedby={describedBy}
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

/** Rich-text tags for ICU messages: a kbd tag renders as a key, always left to right. */
export const RICH = {
  kbd: (chunks: ReactNode[]) => <kbd dir="ltr">{chunks}</kbd>,
};
