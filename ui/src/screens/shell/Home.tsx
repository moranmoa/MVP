import { useEffect, useState, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { isCoreError, useCore, useCoreEvent, type AgeRestriction, type Id, type JoinStatus, type ServerSummary } from '../../core';
import { Code, Dialog } from '../../components/common';
import { LinkIcon, PlusIcon, ShieldIcon, WifiOffIcon } from '../../components/icons';
import { LINK_MAX } from '../../core/sanitize';

export function Home({ hasServers, onCreate, onJoin }: { hasServers: boolean; onCreate: () => void; onJoin: () => void }) {
  return (
    <div className="home">
      <h1>
        <FormattedMessage id="home.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id={hasServers ? 'home.subtitleWithServers' : 'home.subtitle'} />
      </p>
      <div className="home-cards">
        <button type="button" className="home-card" onClick={onCreate}>
          <span className="home-card-icon">
            <PlusIcon />
          </span>
          <strong>
            <FormattedMessage id="home.create.title" />
          </strong>
          <span className="muted">
            <FormattedMessage id="home.create.body" />
          </span>
        </button>
        <button type="button" className="home-card" onClick={onJoin}>
          <span className="home-card-icon">
            <LinkIcon />
          </span>
          <strong>
            <FormattedMessage id="home.join.title" />
          </strong>
          <span className="muted">
            <FormattedMessage id="home.join.body" />
          </span>
        </button>
      </div>
    </div>
  );
}

const AGE_OPTIONS: AgeRestriction[] = ['none', '16+', '18+'];
const ageKey = (a: AgeRestriction) => (a === 'none' ? 'none' : a.slice(0, 2));

export function CreateServerDialog({
  open,
  onClose,
  onNeedCheck,
  onOpenServer,
}: {
  open: boolean;
  onClose: () => void;
  onNeedCheck: () => void;
  onOpenServer: (id: Id) => void;
}) {
  const core = useCore();
  const intl = useIntl();
  const store = useAppStore();
  const checked = useStore(store, (s) => s.identity?.recoveryChecked ?? false);
  const [name, setName] = useState('');
  const [age, setAge] = useState<AgeRestriction | null>(null);
  const [errors, setErrors] = useState<{ name?: boolean; age?: boolean }>({});
  const [created, setCreated] = useState<ServerSummary | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!open) {
      setName('');
      setAge(null);
      setErrors({});
      setCreated(null);
      setCopied(false);
    }
  }, [open]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const errs = { name: !name.trim(), age: !age };
    setErrors(errs);
    if (errs.name || errs.age) return;
    setBusy(true);
    try {
      setCreated(await core.createServer({ name: name.trim(), ageRestriction: age! }));
    } catch (err) {
      if (isCoreError(err) && err.code === 'recovery-check-required') onNeedCheck();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={intl.formatMessage({ id: created ? 'create.done.title' : 'create.title' })}
    >
      {!checked ? (
        <div className="stack">
          <div className="warning">
            <ShieldIcon />
            <p>
              <FormattedMessage id="create.needCheck" />
            </p>
          </div>
          <div className="actions">
            <button type="button" className="primary" onClick={onNeedCheck}>
              <FormattedMessage id="banner.recovery.action" />
            </button>
          </div>
        </div>
      ) : created ? (
        <div className="stack">
          <p>
            <FormattedMessage id="create.done.body" />
          </p>
          <div className="invite-box">
            <Code value={created.inviteLink} />
            <button
              type="button"
              className="secondary small"
              onClick={async () => {
                await navigator.clipboard?.writeText(created.inviteLink).catch(() => undefined);
                setCopied(true);
              }}
            >
              <FormattedMessage id={copied ? 'common.copied' : 'common.copy'} />
            </button>
          </div>
          <div className="actions">
            <button type="button" className="primary" onClick={() => onOpenServer(created.id)}>
              <FormattedMessage id="create.done.open" />
            </button>
          </div>
        </div>
      ) : (
        <form onSubmit={submit} className="stack" noValidate>
          <label className="field">
            <span>
              <FormattedMessage id="create.name" />
            </span>
            <input
              dir="auto"
              value={name}
              maxLength={48}
              autoFocus
              required
              aria-invalid={errors.name || undefined}
              aria-describedby={errors.name ? 'create-name-error' : undefined}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
          {errors.name && (
            <p className="error" id="create-name-error" role="alert">
              <FormattedMessage id="create.error.name" />
            </p>
          )}
          <fieldset className="radio-group" aria-invalid={errors.age || undefined} aria-describedby="create-age-hint">
            <legend>
              <FormattedMessage id="create.age.legend" />
            </legend>
            {AGE_OPTIONS.map((a) => (
              <label key={a} className="radio">
                <input type="radio" name="age" value={a} checked={age === a} required onChange={() => setAge(a)} />
                <FormattedMessage id={`create.age.${ageKey(a)}`} />
              </label>
            ))}
            <p className="muted small" id="create-age-hint">
              <FormattedMessage id="create.age.hint" />
            </p>
          </fieldset>
          {errors.age && (
            <p className="error" role="alert">
              <FormattedMessage id="create.error.age" />
            </p>
          )}
          <div className="actions">
            <button type="submit" className="primary" disabled={busy}>
              <FormattedMessage id="create.submit" />
            </button>
            <button type="button" className="secondary" onClick={onClose}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
        </form>
      )}
    </Dialog>
  );
}

export function JoinDialog({ open, onClose, onJoined }: { open: boolean; onClose: () => void; onJoined: (id: Id) => void }) {
  const core = useCore();
  const intl = useIntl();
  const [link, setLink] = useState('');
  const [status, setStatus] = useState<JoinStatus | null>(null);

  useEffect(() => {
    if (!open) {
      setLink('');
      setStatus(null);
    }
  }, [open]);

  useCoreEvent(
    'join.status',
    (s) => {
      if (!open || s.link !== link.trim()) return;
      setStatus(s);
      if (s.phase === 'joined') setTimeout(() => onJoined(s.serverId), 600);
    },
    [open, link, onJoined],
  );

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!link.trim()) return;
    setStatus(await core.joinServer(link));
  };

  const cancel = () => {
    if (status?.phase === 'searching') void core.cancelJoin(link);
    onClose();
  };

  const busy = status?.phase === 'searching' || status?.phase === 'joined';

  return (
    <Dialog open={open} onClose={cancel} title={intl.formatMessage({ id: 'join.title' })}>
      <form onSubmit={submit} className="stack">
        <label className="field">
          <span>
            <FormattedMessage id="join.link" />
          </span>
          <input
            dir="ltr"
            value={link}
            autoFocus
            disabled={busy}
            placeholder="mvp://join/…"
            maxLength={LINK_MAX}
            spellCheck={false}
            aria-invalid={status?.phase === 'invalid' || undefined}
            onChange={(e) => {
              setLink(e.target.value);
              setStatus(null);
            }}
          />
        </label>
        <div aria-live="polite">
          {status?.phase === 'searching' && (
            <div className="join-state searching">
              <span className="spinner" aria-hidden="true" />
              <div>
                <p>
                  <FormattedMessage id="join.searching" />
                </p>
                <p className="muted small">
                  <FormattedMessage id="join.searchingHint" />
                </p>
              </div>
            </div>
          )}
          {status?.phase === 'nobody-online' && (
            <div className="join-state nobody">
              <WifiOffIcon />
              <p>
                <FormattedMessage id="join.nobody" />
              </p>
            </div>
          )}
          {status?.phase === 'invalid' && (
            <p className="error">
              <FormattedMessage id="join.invalid" />
            </p>
          )}
          {status?.phase === 'age-blocked' && (
            <p className="error">
              <FormattedMessage id="join.ageBlocked" values={{ age: status.required.slice(0, 2) }} />
            </p>
          )}
          {status?.phase === 'joined' && (
            <p className="success">
              <FormattedMessage id="join.joined" />
            </p>
          )}
        </div>
        <div className="actions">
          {status?.phase === 'nobody-online' ? (
            <button type="button" className="primary" onClick={onClose}>
              <FormattedMessage id="join.keepTrying" />
            </button>
          ) : (
            <button type="submit" className="primary" disabled={busy || !link.trim()}>
              <FormattedMessage id="join.submit" />
            </button>
          )}
          <button type="button" className="secondary" onClick={cancel}>
            <FormattedMessage id="common.cancel" />
          </button>
        </div>
      </form>
    </Dialog>
  );
}
