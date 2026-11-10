// B11e §3: edit my profile from Settings → Account. Name, avatar and bio, with
// an explicit Save: a profile change goes out as a signed record to every
// server, so saving on each keystroke would make dozens of them.

import { forwardRef, useEffect, useImperativeHandle, useId, useRef, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { avatarFromFile, IMAGE_TYPES } from '../../app/image';
import { useStore } from '../../app/store';
import { useUserTextProps } from '../../app/textDir';
import { Avatar, Code, Name } from '../../components/common';
import { isCoreError, useCore } from '../../core';
import { cleanName } from '../../core/sanitize';

/** Scrolls a field into view, clear of the sticky save bar (partly visible counts as hidden). */
function keepClear(el: HTMLElement): void {
  el.scrollIntoView?.({ block: 'nearest' });
}
const onFieldFocus = (e: { currentTarget: HTMLElement }) => keepClear(e.currentTarget);

const NAME_MAX = 32;
const NAME_COUNT_FROM = 24;
const BIO_MAX = 190;
const BIO_COUNT_FROM = 150;

function graphemeCount(s: string): number {
  if (typeof Intl !== 'undefined' && 'Segmenter' in Intl) {
    return [...new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(s)].length;
  }
  return [...s].length;
}

export interface ProfileEditorHandle {
  /** Saves; false when it couldn't (the error is shown in the form). */
  save(): Promise<boolean>;
  discard(): void;
}

type Draft = { name: string; bio: string; avatar?: string };

/** The counter "24/32", shown from `from`; read out only when 5 or fewer are left. */
function Counter({ value, max, from }: { value: string; max: number; from: number }) {
  const intl = useIntl();
  const n = graphemeCount(value);
  if (n < from) return null;
  const left = Math.max(0, max - n);
  return (
    <>
      <span className="field-counter" aria-hidden="true">
        {n}/{max}
      </span>
      <span className="visually-hidden" aria-live="polite">
        {left <= 5 ? intl.formatMessage({ id: 'profile.edit.charsLeft' }, { count: left }) : ''}
      </span>
    </>
  );
}

export const ProfileEditor = forwardRef<ProfileEditorHandle, { onDirtyChange: (dirty: boolean) => void }>(function ProfileEditor(
  { onDirtyChange },
  ref,
) {
  const store = useAppStore();
  const core = useCore();
  const intl = useIntl();
  const identity = useStore(store, (s) => s.identity)!;
  const saved: Draft = { name: identity.displayName, bio: identity.bio ?? '', avatar: identity.avatarDataUrl };
  const [draft, setDraft] = useState<Draft>(saved);
  const [busy, setBusy] = useState<'image' | 'saving' | null>(null);
  const [imageError, setImageError] = useState<'too-large' | 'unsupported' | null>(null);
  const [nameError, setNameError] = useState<'empty' | 'chars' | null>(null);
  const [saveError, setSaveError] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const nameProps = useUserTextProps(draft.name);
  const bioProps = useUserTextProps(draft.bio);
  const ids = { name: useId(), nameError: useId(), image: useId(), hint: useId() };

  const dirty = draft.name !== saved.name || draft.bio !== saved.bio || draft.avatar !== saved.avatar;
  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);
  // The save bar turns sticky with the first change: keep the field being edited
  // clear of it (the panel's scroll-padding sets how far).
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = document.activeElement;
    if (dirty && el instanceof HTMLElement && root.current?.contains(el)) keepClear(el);
  }, [dirty]);

  const edit = (patch: Partial<Draft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setJustSaved(false);
    setSaveError(false);
  };

  const save = async (): Promise<boolean> => {
    if (busy) return false;
    // B23 / QA-06: a name of only invisible or control characters is no name.
    if (!cleanName(draft.name)) {
      setNameError('empty');
      nameRef.current?.focus();
      return false;
    }
    setBusy('saving');
    setSaveError(false);
    try {
      // The core replaces every field: send the whole profile (no avatar = removed).
      await core.updateProfile({ displayName: draft.name.trim(), bio: draft.bio.trim() || undefined, avatarDataUrl: draft.avatar });
      setDraft((d) => ({ ...d, name: d.name.trim(), bio: d.bio.trim() }));
      setJustSaved(true);
      return true;
    } catch (e) {
      // B23: bidi, control or zero-width characters are refused, not stripped.
      if (isCoreError(e) && e.code === 'bad-request') {
        setNameError('chars');
        nameRef.current?.focus();
      } else setSaveError(true);
      return false;
    } finally {
      setBusy(null);
    }
  };
  const discard = () => {
    setDraft(saved);
    setNameError(null);
    setImageError(null);
    setSaveError(false);
  };
  useImperativeHandle(ref, () => ({ save, discard }));

  const previewName = draft.name.trim() || identity.shortCode;
  const readOnly = busy === 'saving';

  return (
    <div className="profile-editor" ref={root}>
      <div className="profile-preview" aria-hidden="true">
        <div className="profile-preview-banner" style={{ background: identity.avatarColor }} />
        <div className="profile-preview-body">
          <Avatar name={previewName} color={identity.avatarColor} src={draft.avatar} size={80} />
          <div className="profile-preview-text">
            <span className="profile-preview-name" title={previewName}>
              <Name>{previewName}</Name>
            </span>
            <Code value={identity.shortCode} />
          </div>
        </div>
      </div>

      <div className="profile-avatar-row">
        <label className={`file-button secondary ${busy ? 'disabled' : ''}`}>
          <FormattedMessage id="onb.profile.avatarPick" />
          <input
            type="file"
            accept={IMAGE_TYPES.join(',')}
            className="visually-hidden"
            disabled={!!busy}
            aria-label={intl.formatMessage({ id: 'onb.profile.avatar' })}
            aria-describedby={imageError ? ids.image : undefined}
            onChange={async (e) => {
              const f = e.target.files?.[0];
              e.target.value = '';
              if (!f) return;
              setImageError(null);
              setBusy('image');
              const r = await avatarFromFile(f);
              setBusy(null);
              if (r.ok) edit({ avatar: r.dataUrl });
              else setImageError(r.error); // the previous picture stays
            }}
          />
        </label>
        {draft.avatar && (
          <button type="button" className="secondary" disabled={!!busy} onClick={() => edit({ avatar: undefined })}>
            <FormattedMessage id="profile.edit.remove" />
          </button>
        )}
        {busy === 'image' && <span className="spinner" role="status" aria-label={intl.formatMessage({ id: 'profile.edit.processing' })} />}
      </div>
      {imageError && (
        <p className="error" id={ids.image} role="alert">
          <FormattedMessage id={imageError === 'too-large' ? 'profile.image.tooLarge' : 'profile.image.unsupported'} />
        </p>
      )}

      <label className="field">
        <span>
          <FormattedMessage id="onb.profile.name" />
        </span>
        <input
          {...nameProps}
          ref={nameRef}
          onFocus={onFieldFocus}
          value={draft.name}
          maxLength={NAME_MAX}
          readOnly={readOnly}
          aria-invalid={nameError ? true : undefined}
          aria-describedby={nameError ? ids.nameError : undefined}
          onChange={(e) => {
            edit({ name: e.target.value });
            setNameError(null);
          }}
        />
        <Counter value={draft.name} max={NAME_MAX} from={NAME_COUNT_FROM} />
      </label>
      {nameError && (
        <p className="error" id={ids.nameError} role="alert">
          <FormattedMessage id={nameError === 'empty' ? 'profile.edit.nameEmpty' : 'name.invalidChars'} />
        </p>
      )}

      <label className="field">
        <span>
          <FormattedMessage id="onb.profile.bio" />
        </span>
        <textarea {...bioProps} onFocus={onFieldFocus} value={draft.bio} maxLength={BIO_MAX} rows={4} readOnly={readOnly} onChange={(e) => edit({ bio: e.target.value })} />
        <Counter value={draft.bio} max={BIO_MAX} from={BIO_COUNT_FROM} />
      </label>

      <div className={`profile-actions ${dirty ? 'dirty' : ''}`}>
        <p className="muted small" id={ids.hint}>
          <FormattedMessage id="profile.edit.publishHint" />
        </p>
        {saveError && (
          <p className="error" role="alert">
            <FormattedMessage id="profile.edit.saveError" />
          </p>
        )}
        {justSaved && !dirty && (
          <p className="success" role="status">
            <FormattedMessage id="profile.edit.saved" />
          </p>
        )}
        <div className="actions">
          <button
            type="button"
            className="primary"
            disabled={!dirty || !!busy}
            aria-describedby={ids.hint}
            onClick={() => void save()}
          >
            <FormattedMessage id={busy === 'saving' ? 'profile.edit.saving' : saveError ? 'common.retry' : 'profile.edit.save'} />
          </button>
          <button type="button" className="secondary" disabled={!dirty || !!busy} onClick={discard}>
            <FormattedMessage id="profile.edit.discard" />
          </button>
        </div>
      </div>
    </div>
  );
});
