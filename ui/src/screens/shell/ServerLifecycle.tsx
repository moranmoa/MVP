// Server lifecycle (#261, design docs/design/server-rename-leave-delete.md):
// rename (in server settings), close for good (settings → Danger zone) and
// leave (the server's menu in the rail). Rename and close answer
// `update-needed` while a member still runs an older MVS; nothing was signed
// then. The UI asks the core who (`getUpdateNeeded`), and says that removing
// them is the other way out. A leave is never gated.

import { useEffect, useState, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useUserTextProps } from '../../app/textDir';
import { Dialog, Name } from '../../components/common';
import { isCoreError, useCore, type Member, type ServerSummary } from '../../core';
import { SERVER_NAME_MAX } from '../../core/sanitize';
import { displayName } from './MemberList';

const NO_MEMBERS: Member[] = [];

type Failure = { kind: 'update-needed' } | { kind: 'bad-name' } | { kind: 'other' };

function failureOf(e: unknown): Failure {
  if (isCoreError(e) && e.code === 'update-needed') return { kind: 'update-needed' };
  if (isCoreError(e) && e.code === 'bad-request') return { kind: 'bad-name' };
  return { kind: 'other' };
}

/** `update-needed`: who still runs an older MVS, and the two ways out. */
export function UpdateNeeded({ server }: { server: ServerSummary }) {
  const core = useCore();
  const store = useAppStore();
  const members = useStore(store, (s) => s.members[server.id] ?? NO_MEMBERS);
  // Only the owner can remove members, so only the owner is offered that way out.
  const owner = useStore(store, (s) => s.identity?.id === server.ownerId);
  const [memberIds, setMemberIds] = useState<string[]>([]);
  useEffect(() => {
    let live = true;
    // Without the list the alert still says what happened and what to do.
    core.getUpdateNeeded(server.id).then((ids) => live && setMemberIds(ids), () => {});
    return () => {
      live = false;
    };
  }, [core, server.id]);
  const names = memberIds.map((id) => members.find((m) => m.id === id)).filter((m): m is Member => !!m);
  return (
    <div className="warning update-needed" role="alert">
      <div>
        <p>
          <FormattedMessage id="server.updateNeeded" />
        </p>
        {names.length > 0 && (
          <>
            <p className="small">
              <FormattedMessage id="server.updateNeeded.who" />
            </p>
            <ul className="update-needed-list">
              {names.map((m) => (
                <li key={m.id}>
                  <Name>{displayName(m)}</Name>
                </li>
              ))}
            </ul>
          </>
        )}
        <p className="small">
          <FormattedMessage id={owner ? 'server.updateNeeded.remedy' : 'server.updateNeeded.remedyMember'} />
        </p>
      </div>
    </div>
  );
}

function FailureNote({ server, failure }: { server: ServerSummary; failure: Failure | null }) {
  if (!failure) return null;
  if (failure.kind === 'update-needed') return <UpdateNeeded server={server} />;
  return (
    <p className="error small" role="alert">
      <FormattedMessage id={failure.kind === 'bad-name' ? 'name.invalidChars' : 'invite.failed'} />
    </p>
  );
}

/** Settings → General: the server's name, for the owner (admins too, once roles reach the UI). */
export function RenameForm({ server }: { server: ServerSummary }) {
  const core = useCore();
  const [name, setName] = useState(server.name);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const nameProps = useUserTextProps(name);
  useEffect(() => setName(server.name), [server.name]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const clean = name.trim();
    if (!clean || clean === server.name || busy) return;
    setBusy(true);
    setFailure(null);
    setSaved(false);
    try {
      await core.renameServer(server.id, clean);
      setSaved(true);
    } catch (err) {
      setFailure(failureOf(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="stack" onSubmit={submit} noValidate>
      <label className="field">
        <span>
          <FormattedMessage id="server.settings.name" />
        </span>
        <input
          {...nameProps}
          value={name}
          maxLength={SERVER_NAME_MAX}
          onChange={(e) => {
            setName(e.target.value);
            setSaved(false);
            setFailure(null);
          }}
        />
      </label>
      <div className="actions">
        <button type="submit" className="primary" disabled={busy || !name.trim() || name.trim() === server.name}>
          <FormattedMessage id="server.rename.save" />
        </button>
        {saved && (
          <span className="success small" role="status">
            <FormattedMessage id="server.rename.saved" />
          </span>
        )}
      </div>
      <FailureNote server={server} failure={failure} />
    </form>
  );
}

/** Settings → Danger zone: close the server for good, behind a typed-name confirmation. */
export function ClosePanel({ server }: { server: ServerSummary }) {
  const core = useCore();
  const intl = useIntl();
  const [confirm, setConfirm] = useState(false);
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const typedProps = useUserTextProps(typed);
  const matches = typed.trim() === server.name;

  const close = async () => {
    if (!matches || busy) return;
    setBusy(true);
    setFailure(null);
    try {
      await core.closeServer(server.id);
      setConfirm(false);
    } catch (err) {
      setFailure(failureOf(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      <p>
        <FormattedMessage id="server.close.hint" />
      </p>
      <div>
        <button
          type="button"
          className="danger"
          onClick={() => {
            setTyped('');
            setFailure(null);
            setConfirm(true);
          }}
        >
          <FormattedMessage id="server.close.button" />
        </button>
      </div>
      <Dialog
        open={confirm}
        onClose={() => setConfirm(false)}
        title={intl.formatMessage({ id: 'server.close.title' }, { name: server.name })}
      >
        <div className="stack">
          <p>
            <FormattedMessage id="server.close.body" />
          </p>
          <label className="field">
            <span>
              <FormattedMessage id="server.close.typeName" values={{ name: <Name>{server.name}</Name> }} />
            </span>
            <input {...typedProps} value={typed} autoFocus onChange={(e) => setTyped(e.target.value)} />
          </label>
          <FailureNote server={server} failure={failure} />
          {failure?.kind === 'update-needed' ? (
            // Nothing can be done here until the gate opens: only Close remains.
            <div className="actions">
              <button type="button" className="secondary" autoFocus onClick={() => setConfirm(false)}>
                <FormattedMessage id="common.close" />
              </button>
            </div>
          ) : (
            <div className="actions">
              <button type="button" className="danger" disabled={!matches || busy} onClick={() => void close()}>
                <FormattedMessage id="server.close.confirm" />
              </button>
              <button type="button" className="secondary" onClick={() => setConfirm(false)}>
                <FormattedMessage id="common.cancel" />
              </button>
            </div>
          )}
        </div>
      </Dialog>
    </div>
  );
}

/** From the server's menu in the rail: any member but the owner. */
export function LeaveServerDialog({ server, onClose }: { server: ServerSummary | null; onClose: () => void }) {
  const core = useCore();
  const intl = useIntl();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  useEffect(() => setFailure(null), [server?.id]);

  const leave = async () => {
    if (!server || busy) return;
    setBusy(true);
    setFailure(null);
    try {
      await core.leaveServer(server.id);
      onClose();
    } catch (err) {
      setFailure(failureOf(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={!!server} onClose={onClose} title={intl.formatMessage({ id: 'server.leave.title' }, { name: server?.name ?? '' })}>
      {server && (
        <div className="stack">
          <p>
            <FormattedMessage id="server.leave.body" />
          </p>
          <FailureNote server={server} failure={failure} />
          <div className="actions">
            <button type="button" className="danger" autoFocus disabled={busy} onClick={() => void leave()}>
              <FormattedMessage id="server.leave.confirm" />
            </button>
            <button type="button" className="secondary" onClick={onClose}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
        </div>
      )}
    </Dialog>
  );
}
