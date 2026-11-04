// B11c (spec docs/ux/specs/B11a-d-server-management.md §4): the owner creates,
// edits and deletes channels. One dialog for create and edit; #general (the
// default channel) can be renamed but not deleted.

import { useEffect, useState, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { useUserTextProps } from '../../app/textDir';
import { Dialog } from '../../components/common';
import { isCoreError, useCore, type Channel, type Id } from '../../core';

/** As the core stores it: spaces become '-', runs of '-' collapse (§4.1). */
export function channelNameOf(raw: string): string {
  return raw.replace(/\s+/g, '-').replace(/-{2,}/g, '-').slice(0, 32);
}

function isAdult(birthDate: string | undefined): boolean {
  const d = birthDate ? new Date(birthDate) : null;
  if (!d || Number.isNaN(d.getTime())) return false;
  const now = new Date();
  const age = now.getFullYear() - d.getFullYear() - (now < new Date(now.getFullYear(), d.getMonth(), d.getDate()) ? 1 : 0);
  return age >= 18;
}

type Mode = { kind: 'create'; serverId: Id } | { kind: 'edit'; channel: Channel };

export function ChannelDialog({
  mode,
  onClose,
  onCreated,
}: {
  mode: Mode | null;
  onClose: () => void;
  onCreated: (channel: Channel) => void;
}) {
  const core = useCore();
  const intl = useIntl();
  const identity = useStore(useAppStore(), (s) => s.identity);
  const editing = mode?.kind === 'edit' ? mode.channel : null;
  const [name, setName] = useState('');
  const [adult, setAdult] = useState(false);
  const [error, setError] = useState<'taken' | 'chars' | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const nameProps = useUserTextProps(name);
  const canAdult = isAdult(identity?.birthDate);

  useEffect(() => {
    setName(editing?.name ?? '');
    setAdult(editing?.adult ?? false);
    setError(null);
    setConfirmDelete(false);
  }, [mode, editing]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const clean = channelNameOf(name).replace(/^-|-$/g, '');
    if (!clean || !mode || busy) return;
    setBusy(true);
    try {
      if (mode.kind === 'create') onCreated(await core.createChannel(mode.serverId, { name: clean, adult }));
      else await core.updateChannel(mode.channel.id, { name: clean, adult });
      onClose();
    } catch (err) {
      if (isCoreError(err) && err.code === 'name-taken') setError('taken');
      else if (isCoreError(err) && err.code === 'bad-request') setError('chars');
      else throw err;
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!editing) return;
    await core.deleteChannel(editing.id);
    onClose();
  };

  const preview = channelNameOf(name).replace(/^-|-$/g, '');
  return (
    <>
      <Dialog
        open={!!mode && !confirmDelete}
        onClose={onClose}
        title={intl.formatMessage({ id: editing ? 'channel.edit' : 'channel.create' })}
      >
        <form onSubmit={submit} className="stack" noValidate>
          <label className="field">
            <span>
              <FormattedMessage id="channel.name" />
            </span>
            <input
              {...nameProps}
              value={name}
              maxLength={32}
              autoFocus
              required
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? 'channel-name-error' : 'channel-name-preview'}
              onChange={(e) => {
                setName(channelNameOf(e.target.value));
                setError(null);
              }}
            />
          </label>
          <p id="channel-name-preview" className="muted small" dir="ltr">
            #<bdi>{preview}</bdi>
          </p>
          {error && (
            <p className="error" id="channel-name-error" role="alert">
              <FormattedMessage id={error === 'taken' ? 'channel.error.nameTaken' : 'name.invalidChars'} />
            </p>
          )}
          <label className="check">
            <input type="checkbox" checked={adult} disabled={!canAdult} aria-describedby="channel-adult-hint" onChange={(e) => setAdult(e.target.checked)} />
            <span>
              <FormattedMessage id="channel.adultLabel" />
            </span>
          </label>
          <p id="channel-adult-hint" className="muted small">
            <FormattedMessage id={canAdult ? 'channel.adultHint' : 'channel.adultNeeds18'} />
          </p>
          <div className="actions">
            <button type="submit" className="primary" disabled={!preview || busy}>
              <FormattedMessage id={editing ? 'channel.save' : 'create.submit'} />
            </button>
            <button type="button" className="secondary" onClick={onClose}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
          {editing &&
            (editing.isDefault ? (
              <p className="muted small">
                <FormattedMessage id="channel.cannotDeleteDefault" />
              </p>
            ) : (
              <div className="danger-zone">
                <button type="button" className="danger" onClick={() => setConfirmDelete(true)}>
                  <FormattedMessage id="channel.delete" />
                </button>
              </div>
            ))}
        </form>
      </Dialog>
      <Dialog
        open={!!editing && confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={intl.formatMessage({ id: 'channel.delete.title' }, { channel: editing?.name ?? '' })}
      >
        <p>
          <FormattedMessage id="channel.delete.body" />
        </p>
        <div className="actions">
          <button type="button" className="danger" autoFocus onClick={() => void remove()}>
            <FormattedMessage id="channel.delete" />
          </button>
          <button type="button" className="secondary" onClick={() => setConfirmDelete(false)}>
            <FormattedMessage id="common.cancel" />
          </button>
        </div>
      </Dialog>
    </>
  );
}
