import { useCallback, useEffect, useRef, useState } from 'react';
import { FormattedMessage, FormattedNumber, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { FONT_MAX, FONT_MIN, SCALE_MAX, SCALE_MIN, useSettings, type Theme } from '../../app/settings';
import { useStore } from '../../app/store';
import { useCore, type AppInfo, type StorageUsage } from '../../core';
import { Avatar, Code, Dialog, Name, RICH } from '../../components/common';
import { CheckIcon, ShieldIcon } from '../../components/icons';
import { LOCALES } from '../../i18n';
import { textDir, useUiDir } from '../../app/textDir';
import { ProfileEditor, type ProfileEditorHandle } from './ProfileEditor';
import { SignOutButton } from './SignOut';

type Section = 'account' | 'privacy' | 'language' | 'appearance' | 'storage' | 'diagnostics' | 'about';
const SECTIONS: Section[] = ['account', 'privacy', 'language', 'appearance', 'storage', 'diagnostics', 'about'];

const GB = 1024 ** 3;
const LIMIT_STEPS = [1, 2, 5, 10, 20, 50].map((g) => g * GB);

export function SettingsScreen({
  open,
  request,
  onClose,
  onCheckWords,
}: {
  open: boolean;
  /** Open at this section, e.g. Privacy from the voice room's "Change in Settings". */
  request?: { section: string; at: number } | null;
  onClose: () => void;
  onCheckWords: () => void;
}) {
  const intl = useIntl();
  const [section, setSection] = useState<Section>(() => {
    const s = new URLSearchParams(location.search).get('settings');
    return SECTIONS.includes(s as Section) ? (s as Section) : 'appearance';
  });
  // B11e: unsaved profile changes are asked about before leaving Account.
  const [profileDirty, setProfileDirty] = useState(false);
  const editor = useRef<ProfileEditorHandle>(null);
  const [leaving, setLeaving] = useState<(() => void) | null>(null);
  const guard = (action: () => void) => (profileDirty && section === 'account' ? setLeaving(() => action) : action());
  const onDirtyChange = useCallback((d: boolean) => setProfileDirty(d), []);
  useEffect(() => {
    if (request && SECTIONS.includes(request.section as Section)) guard(() => setSection(request.section as Section));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request]);
  const leave = () => {
    const go = leaving;
    setLeaving(null);
    setProfileDirty(false);
    go?.();
  };

  return (
    <Dialog open={open} onClose={() => guard(onClose)} title={intl.formatMessage({ id: 'settings.title' })} className="settings-dialog">
      <div className="settings">
        <nav className="settings-nav" aria-label={intl.formatMessage({ id: 'settings.title' })}>
          <ul>
            {SECTIONS.map((s) => (
              <li key={s}>
                <button
                  type="button"
                  className={section === s ? 'active' : ''}
                  aria-current={section === s ? 'true' : undefined}
                  onClick={() => s !== section && guard(() => setSection(s))}
                >
                  <FormattedMessage id={`settings.nav.${s}`} />
                  {s === 'account' && profileDirty && (
                    <span className="nav-dirty" role="img" aria-label={intl.formatMessage({ id: 'profile.edit.unsaved' })} />
                  )}
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <section className="settings-panel" aria-labelledby={`settings-h-${section}`}>
          <h3 id={`settings-h-${section}`}>
            <FormattedMessage id={`settings.nav.${section}`} />
          </h3>
          {section === 'account' && <AccountSection onCheckWords={onCheckWords} editor={editor} onDirtyChange={onDirtyChange} />}
          {section === 'language' && <LanguageSection />}
          {section === 'appearance' && <AppearanceSection />}
          {section === 'storage' && <StorageSection />}
          {section === 'diagnostics' && <DiagnosticsSection />}
          {section === 'about' && <AboutSection />}
          {section === 'privacy' && <PrivacySection focus={request?.section === 'privacy' ? request.at : undefined} />}
        </section>
      </div>
      <Dialog open={!!leaving} onClose={() => setLeaving(null)} title={intl.formatMessage({ id: 'profile.edit.unsavedTitle' })}>
        <div className="actions">
          <button
            type="button"
            className="primary"
            autoFocus
            onClick={async () => {
              if (await editor.current?.save()) leave();
              else setLeaving(null); // the error shows in the form
            }}
          >
            <FormattedMessage id="profile.edit.save" />
          </button>
          <button
            type="button"
            className="secondary"
            onClick={() => {
              editor.current?.discard();
              leave();
            }}
          >
            <FormattedMessage id="profile.edit.unsavedDiscard" />
          </button>
          <button type="button" className="link" onClick={() => setLeaving(null)}>
            <FormattedMessage id="profile.edit.unsavedBack" />
          </button>
        </div>
      </Dialog>
    </Dialog>
  );
}

function AccountSection({
  onCheckWords,
  editor,
  onDirtyChange,
}: {
  onCheckWords: () => void;
  editor: React.Ref<ProfileEditorHandle>;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const store = useAppStore();
  const identity = useStore(store, (s) => s.identity)!;
  return (
    <div className="stack account-sections">
      <ProfileEditor ref={editor} onDirtyChange={onDirtyChange} />
      <BirthDateSection />
      <h4>
        <FormattedMessage id="settings.account.words" />
      </h4>
      {identity.recoveryChecked ? (
        <p className="success">
          <CheckIcon /> <FormattedMessage id="settings.account.checked" />
        </p>
      ) : (
        <div className="warning">
          <ShieldIcon />
          <div>
            <p>
              <FormattedMessage id="settings.account.notChecked" />
            </p>
            <button type="button" className="secondary small" onClick={onCheckWords}>
              <FormattedMessage id="settings.account.check" />
            </button>
          </div>
        </div>
      )}
      <h4>
        <FormattedMessage id="signOut.heading" />
      </h4>
      <SignOutButton />
    </div>
  );
}

function LanguageSection() {
  const { settings, update } = useSettings();
  const intl = useIntl();
  return (
    <div className="stack">
      <div className="choice-list" role="radiogroup" aria-label={intl.formatMessage({ id: 'settings.language.label' })}>
        {LOCALES.map((l) => (
          <button
            key={l}
            type="button"
            role="radio"
            aria-checked={settings.locale === l}
            className="choice"
            lang={l}
            onClick={() => update({ locale: l })}
          >
            <FormattedMessage id={`lang.${l}`} />
            {settings.locale === l && <CheckIcon />}
          </button>
        ))}
      </div>
      <p className="muted small">
        <FormattedMessage id="settings.instant" />
      </p>
    </div>
  );
}

function AppearanceSection() {
  const { settings, update } = useSettings();
  const intl = useIntl();
  const uiDir = useUiDir();
  const themes: Theme[] = ['system', 'light', 'dark'];
  return (
    <div className="stack">
      <fieldset className="segmented">
        <legend>
          <FormattedMessage id="settings.theme.label" />
        </legend>
        {themes.map((t) => (
          <label key={t} className={settings.theme === t ? 'checked' : ''}>
            <input type="radio" name="theme" checked={settings.theme === t} onChange={() => update({ theme: t })} />
            <FormattedMessage id={`settings.theme.${t}`} />
          </label>
        ))}
      </fieldset>

      <label className="field range">
        <span>
          <FormattedMessage id="settings.scale.label" />
          <output>
            <FormattedNumber value={settings.uiScale} style="percent" />
          </output>
        </span>
        <input
          type="range"
          min={SCALE_MIN * 100}
          max={SCALE_MAX * 100}
          step={5}
          value={Math.round(settings.uiScale * 100)}
          onChange={(e) => update({ uiScale: Number(e.target.value) / 100 })}
          aria-valuetext={intl.formatNumber(settings.uiScale, { style: 'percent' })}
        />
      </label>

      <label className="field range">
        <span>
          <FormattedMessage id="settings.font.label" />
          <output>
            <FormattedMessage id="settings.font.value" values={{ size: settings.fontSize }} />
          </output>
        </span>
        <input
          type="range"
          min={FONT_MIN}
          max={FONT_MAX}
          step={1}
          value={settings.fontSize}
          onChange={(e) => update({ fontSize: Number(e.target.value) })}
          aria-valuetext={intl.formatMessage({ id: 'settings.font.value' }, { size: settings.fontSize })}
        />
      </label>
      <p className="muted small">
        <FormattedMessage id="settings.zoomHint" values={RICH} />
      </p>
      <div>
        <button type="button" className="secondary small" onClick={() => update({ uiScale: 1, fontSize: 15 })}>
          <FormattedMessage id="settings.reset" />
        </button>
      </div>

      <div className="preview" aria-label={intl.formatMessage({ id: 'settings.preview' })} role="group">
        <span className="preview-label">
          <FormattedMessage id="settings.preview" />
        </span>
        {[1, 2].map((n) => (
          <div key={n} className="msg">
            <div className="msg-main">
              <div className="msg-gutter">
                <Avatar name={n === 1 ? 'N' : 'D'} color={n === 1 ? '#d6336c' : '#1098ad'} size={40} />
              </div>
              <div className="msg-content">
                <div className="msg-head">
                  <Name className="author">{n === 1 ? 'נועה' : 'Daniel'}</Name>
                  <Code value={n === 1 ? 'K7QM-2XPA' : 'R4TD-9HWE'} />
                  <time>{intl.formatTime(new Date(2026, 10, 3, 21, n * 2), { hour: '2-digit', minute: '2-digit' })}</time>
                </div>
                <p className="msg-body user-text" dir={textDir(intl.formatMessage({ id: `settings.preview.sample${n}` }), uiDir)}>
                  <FormattedMessage id={`settings.preview.sample${n}`} />
                </p>
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Bytes({ value }: { value: number }) {
  const gb = value >= GB;
  // Sizes read "451 MB" in every language (U4).
  return (
    <bdi dir="ltr">
      <FormattedNumber
        value={gb ? value / GB : value / 1024 ** 2}
        style="unit"
        unit={gb ? 'gigabyte' : 'megabyte'}
        unitDisplay="short"
        maximumFractionDigits={gb ? 1 : 0}
      />
    </bdi>
  );
}

function StorageSection() {
  const core = useCore();
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [cleared, setCleared] = useState(false);
  const intl = useIntl();

  useEffect(() => {
    void core.getStorageUsage().then(setUsage);
  }, [core]);

  if (!usage) return null;
  const stepIndex = Math.max(0, LIMIT_STEPS.findIndex((b) => b >= usage.limitBytes));

  return (
    <div className="stack">
      <p>
        <FormattedMessage id="settings.storage.used" values={{ used: <Bytes value={usage.usedBytes} />, limit: <Bytes value={usage.limitBytes} /> }} />
      </p>
      <meter min={0} max={usage.limitBytes} value={usage.usedBytes} aria-label={intl.formatMessage({ id: 'settings.nav.storage' })} />
      <dl className="storage-breakdown">
        <dt>
          <FormattedMessage id="settings.storage.messages" />
        </dt>
        <dd>
          <Bytes value={usage.messagesBytes} />
        </dd>
        <dt>
          <FormattedMessage id="settings.storage.files" />
        </dt>
        <dd>
          <Bytes value={usage.filesBytes} />
        </dd>
      </dl>
      <label className="field range">
        <span>
          <FormattedMessage id="settings.storage.limit" />
          <output>
            <Bytes value={usage.limitBytes} />
          </output>
        </span>
        <input
          type="range"
          min={0}
          max={LIMIT_STEPS.length - 1}
          value={stepIndex}
          aria-valuetext={intl.formatNumber(LIMIT_STEPS[stepIndex] / GB, { style: 'unit', unit: 'gigabyte' })}
          onChange={async (e) => {
            const bytes = LIMIT_STEPS[Number(e.target.value)];
            await core.setStorageLimit(bytes);
            setUsage({ ...usage, limitBytes: bytes });
          }}
        />
      </label>
      <p className="muted small">
        <FormattedMessage id="settings.storage.limitHint" />
      </p>
      <div className="actions">
        <button
          type="button"
          className="secondary"
          onClick={async () => {
            await core.clearCache();
            setCleared(true);
          }}
        >
          <FormattedMessage id="settings.storage.clear" />
        </button>
        {cleared && (
          <span className="success" role="status">
            <FormattedMessage id="settings.storage.cleared" />
          </span>
        )}
      </div>
    </div>
  );
}

function DiagnosticsSection() {
  const core = useCore();
  const [done, setDone] = useState(false);
  const exportFile = async () => {
    const text = await core.exportDiagnostics();
    // In Tauri/Electron this becomes a native save dialog; in the browser it downloads.
    const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `diagnostics-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.json`;
    a.click();
    URL.revokeObjectURL(url);
    setDone(true);
  };
  return (
    <div className="stack">
      <p>
        <FormattedMessage id="settings.diag.body" />
      </p>
      <div className="actions">
        <button type="button" className="primary" onClick={exportFile}>
          <FormattedMessage id="settings.diag.export" />
        </button>
        {done && (
          <span className="success" role="status">
            <FormattedMessage id="settings.diag.done" />
          </span>
        )}
      </div>
    </div>
  );
}

// Relay-only (GAP-6r, #272), per device: nobody connects to me directly, so
// members never see my IP address; calls are off while it is on.
function PrivacySection({ focus }: { focus?: number }) {
  const store = useAppStore();
  const intl = useIntl();
  const relayOnly = useStore(store, (s) => s.relayOnly);
  const inCall = useStore(store, (s) => !!s.call);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  // Turning it on ends a call in progress: ask first, in the row.
  const [confirm, setConfirm] = useState(false);
  const switchRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!focus) return;
    // After the dialog opens (showModal moves focus first).
    const t = setTimeout(() => switchRef.current?.focus());
    return () => clearTimeout(t);
  }, [focus]);

  const apply = async (on: boolean) => {
    setConfirm(false);
    setBusy(true);
    setFailed(false);
    try {
      await store.setRelayOnly(on);
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="stack">
      <label className="switch-row">
        <span>
          <FormattedMessage id="privacy.relayOnly.label" />
        </span>
        <input
          ref={switchRef}
          type="checkbox"
          role="switch"
          className="switch"
          checked={relayOnly}
          disabled={busy}
          aria-describedby="relay-only-hint relay-only-calls"
          onChange={(e) => (e.target.checked && inCall ? setConfirm(true) : void apply(e.target.checked))}
        />
      </label>
      {confirm && (
        <div className="confirm" role="alertdialog" aria-label={intl.formatMessage({ id: 'privacy.relayOnly.confirm' })}>
          <p>
            <FormattedMessage id="privacy.relayOnly.confirm" />
          </p>
          <div className="actions">
            <button type="button" className="danger small" autoFocus onClick={() => void apply(true)}>
              <FormattedMessage id="privacy.relayOnly.turnOn" />
            </button>
            <button type="button" className="secondary small" onClick={() => setConfirm(false)}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
        </div>
      )}
      <p id="relay-only-hint" className="muted small">
        <FormattedMessage id="privacy.relayOnly.hint" />
      </p>
      <p id="relay-only-calls" className="muted small">
        <FormattedMessage id="privacy.relayOnly.calls" />
      </p>
      {failed && (
        <p className="error small" role="alert">
          <FormattedMessage id="invite.failed" />
        </p>
      )}
    </div>
  );
}

function AboutSection() {
  const core = useCore();
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    void core.getAppInfo().then(setInfo);
  }, [core]);
  if (!info) return null;
  return (
    <div className="stack">
      <p>
        <strong>
          <FormattedMessage id="app.name" />
        </strong>{' '}
        <FormattedMessage id="settings.about.version" values={{ version: <span dir="ltr">{info.version}</span> }} />
      </p>
      <p className="muted">
        <FormattedMessage id="settings.about.alpha" />
      </p>
      <fieldset className="segmented">
        <legend>
          <FormattedMessage id="settings.about.channel" />
        </legend>
        {(['stable', 'beta'] as const).map((c) => (
          <label key={c} className={info.updateChannel === c ? 'checked' : ''}>
            <input
              type="radio"
              name="update-channel"
              checked={info.updateChannel === c}
              onChange={async () => {
                await core.setUpdateChannel(c);
                setInfo({ ...info, updateChannel: c });
              }}
            />
            <FormattedMessage id={`settings.about.${c}`} />
          </label>
        ))}
      </fieldset>
      <p className="muted small">
        <FormattedMessage id="settings.about.channelHint" />
      </p>
      <p className="muted small">
        <FormattedMessage id="settings.about.licenses" />
      </p>
    </div>
  );
}

/** A calendar date (YYYY-MM-DD) as the person reads it, without a time-zone shift. */
function formatBirthDate(intl: ReturnType<typeof useIntl>, date: string): string {
  const [y, m, d] = date.split('-').map(Number);
  return intl.formatDate(new Date(y, m - 1, d), { dateStyle: 'long' });
}

/**
 * B7 §2: my date of birth, and a correction that raises my age while it waits
 * its 30 days. Correcting it ("תיקון", and cancelling a pending correction)
 * waits for the core's correction commands.
 */
function BirthDateSection() {
  const store = useAppStore();
  const intl = useIntl();
  const identity = useStore(store, (s) => s.identity)!;
  const pending = identity.pendingBirthDate;
  return (
    <section className="dob-section" aria-labelledby="dob-heading">
      <h4 id="dob-heading">
        <FormattedMessage id="age.dob.section" />
      </h4>
      {identity.birthDate && <p className="dob-current">{formatBirthDate(intl, identity.birthDate)}</p>}
      {pending && (
        <p className="dob-pending" role="status">
          <FormattedMessage
            id="age.dob.pending"
            values={{
              newDate: formatBirthDate(intl, pending.date),
              date: intl.formatDate(pending.effectiveAt, { dateStyle: 'long' }),
            }}
          />
        </p>
      )}
      <p className="muted small">
        <FormattedMessage id="age.dob.privacy" />
      </p>
    </section>
  );
}
