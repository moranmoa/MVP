import { useEffect, useMemo, useRef, useState, type ChangeEvent, type FormEvent, type ReactNode } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { ageOn, countryOptions, detectCountry, MIN_AGE, toBirthDate } from '../../app/age';
import { useAppStore } from '../../app/App';
import { useSettings } from '../../app/settings';
import { useCore, useCoreEvent, type Identity, type Locale } from '../../core';
import type { CoreEvents } from '../../core/api';
import { safeImageSrc } from '../../core/sanitize';
import { Avatar, Code } from '../../components/common';
import { CheckIcon } from '../../components/icons';
import { LOCALES } from '../../i18n';
import { RecoveryCheck, WordGrid, WordsWarning } from './RecoveryWords';
import { RestoreScreen } from './Restore';

type Step = 'language' | 'agreement' | 'country' | 'dob' | 'blocked' | 'preparing' | 'words' | 'check' | 'profile' | 'restore';

/** Numbered steps shown as "step x of y"; blocked/preparing/restore are not counted. */
const NUMBERED: Step[] = ['language', 'agreement', 'country', 'dob', 'words', 'check', 'profile'];

export function Onboarding({ onFinished }: { onFinished: () => void }) {
  const [step, setStep] = useState<Step>('language');
  const [country, setCountry] = useState(detectCountry);
  const [birthDate, setBirthDate] = useState<string | null>(null);
  const [words, setWords] = useState<string[]>([]);
  const { settings } = useSettings();
  const core = useCore();
  const store = useAppStore();

  const back: Partial<Record<Step, Step>> = {
    agreement: 'language',
    country: 'agreement',
    dob: 'country',
    check: 'words',
    restore: 'language',
  };

  const createAccount = async (dob: string) => {
    setStep('preparing');
    const { identity, words } = await core.createIdentity({ locale: settings.locale, country, birthDate: dob });
    setWords(words);
    store.setIdentity(identity);
    setStep('words');
  };

  // The words are not needed after the check, so they do not stay in memory.
  const leaveWords = () => {
    setWords([]);
    setStep('profile');
  };

  const stepNo = NUMBERED.indexOf(step);

  return (
    <main className="onboarding">
      <div className="onb-card">
        {(back[step] || stepNo >= 0) && (
          <div className="onb-top">
            {back[step] ? (
              <button type="button" className="link" onClick={() => setStep(back[step]!)}>
                <FormattedMessage id="common.back" />
              </button>
            ) : (
              <span />
            )}
            {stepNo >= 0 && (
              <span className="muted small">
                <FormattedMessage id="onb.progress" values={{ step: stepNo + 1, total: NUMBERED.length }} />
              </span>
            )}
          </div>
        )}
        <StepBody key={step}>
          {step === 'language' && <LanguageStep onNext={() => setStep('agreement')} onRestore={() => setStep('restore')} />}
          {step === 'agreement' && <AgreementStep onNext={() => setStep('country')} />}
          {step === 'country' && <CountryStep value={country} onChange={setCountry} onNext={() => setStep('dob')} />}
          {step === 'dob' && (
            <DobStep
              initial={birthDate}
              onNext={(dob) => {
                setBirthDate(dob);
                if (ageOn(dob) < MIN_AGE) setStep('blocked');
                else void createAccount(dob);
              }}
            />
          )}
          {step === 'blocked' && (
            <BlockedStep
              onBack={() => {
                setBirthDate(null);
                setStep('language');
              }}
            />
          )}
          {step === 'preparing' && <PreparingStep />}
          {step === 'words' && <WordsStep words={words} onNext={() => setStep('check')} />}
          {step === 'check' && (
            <>
              <h1>
                <FormattedMessage id="onb.check.title" />
              </h1>
              <RecoveryCheck
                onChecked={leaveWords}
                onLater={leaveWords}
                onShowAgain={() => setStep('words')}
              />
            </>
          )}
          {step === 'profile' && <ProfileStep onDone={onFinished} />}
          {step === 'restore' && (
            <RestoreScreen
              onRestored={(identity: Identity) => {
                store.setIdentity(identity);
                onFinished();
              }}
            />
          )}
        </StepBody>
      </div>
    </main>
  );
}

/** Moves focus to the new step's heading so screen readers announce it. */
function StepBody({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const target = ref.current?.querySelector<HTMLElement>('[autofocus], h1');
    if (target?.tagName === 'H1') {
      target.tabIndex = -1;
      target.focus();
    }
  }, []);
  return (
    <div ref={ref} className="onb-step">
      {children}
    </div>
  );
}

function LanguageStep({ onNext, onRestore }: { onNext: () => void; onRestore: () => void }) {
  const { settings, update } = useSettings();
  const intl = useIntl();
  return (
    <>
      <h1>
        <FormattedMessage id="onb.language.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id="onb.language.hint" />
      </p>
      <div className="choice-list" role="radiogroup" aria-label={intl.formatMessage({ id: 'onb.language.title' })}>
        {LOCALES.map((l: Locale) => (
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
      <div className="actions">
        <button type="button" className="primary" onClick={onNext}>
          <FormattedMessage id="common.next" />
        </button>
        <button type="button" className="link" onClick={onRestore}>
          <FormattedMessage id="onb.restoreLink" />
        </button>
      </div>
    </>
  );
}

function AgreementStep({ onNext }: { onNext: () => void }) {
  const [accepted, setAccepted] = useState(false);
  return (
    <>
      <h1>
        <FormattedMessage id="onb.agreement.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id="onb.agreement.intro" />
      </p>
      <ol className="terms">
        {[1, 2, 3, 4, 5, 6, 7].map((n) => (
          <li key={n}>
            <FormattedMessage id={`onb.agreement.item${n}`} />
          </li>
        ))}
      </ol>
      <label className="checkbox">
        <input type="checkbox" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
        <FormattedMessage id="onb.agreement.accept" />
      </label>
      <div className="actions">
        <button type="button" className="primary" disabled={!accepted} onClick={onNext}>
          <FormattedMessage id="common.next" />
        </button>
      </div>
    </>
  );
}

function CountryStep({ value, onChange, onNext }: { value: string; onChange: (c: string) => void; onNext: () => void }) {
  const intl = useIntl();
  const options = useMemo(() => countryOptions(intl.locale), [intl.locale]);
  return (
    <>
      <h1>
        <FormattedMessage id="onb.country.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id="onb.country.hint" />
      </p>
      <label className="field">
        <span>
          <FormattedMessage id="onb.country.label" />
        </span>
        <select value={value} onChange={(e) => onChange(e.target.value)}>
          {options.map((o) => (
            <option key={o.code} value={o.code}>
              {o.name}
            </option>
          ))}
        </select>
      </label>
      <div className="actions">
        <button type="button" className="primary" onClick={onNext}>
          <FormattedMessage id="common.next" />
        </button>
      </div>
    </>
  );
}

function DobStep({ initial, onNext }: { initial: string | null; onNext: (dob: string) => void }) {
  const [y0, m0, d0] = initial?.split('-') ?? ['', '', ''];
  const [day, setDay] = useState(d0 ? String(Number(d0)) : '');
  const [month, setMonth] = useState(m0 ? String(Number(m0)) : '');
  const [year, setYear] = useState(y0);
  const [invalid, setInvalid] = useState(false);
  const intl = useIntl();

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const dob = toBirthDate(day, month, year);
    if (!dob) setInvalid(true);
    else onNext(dob);
  };

  const digits = (set: (v: string) => void, max: number) => (e: ChangeEvent<HTMLInputElement>) => {
    setInvalid(false);
    set(e.target.value.replace(/\D/g, '').slice(0, max));
  };

  return (
    <form onSubmit={submit}>
      <h1>
        <FormattedMessage id="onb.dob.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id="onb.dob.hint" />
      </p>
      <fieldset className="dob" aria-describedby={invalid ? 'dob-error' : undefined}>
        <legend className="visually-hidden">
          <FormattedMessage id="onb.dob.legend" />
        </legend>
        <label className="field">
          <span>
            <FormattedMessage id="onb.dob.day" />
          </span>
          <input
            inputMode="numeric"
            autoComplete="bday-day"
            value={day}
            onChange={digits(setDay, 2)}
            aria-invalid={invalid || undefined}
            placeholder={intl.formatNumber(1)}
            size={3}
          />
        </label>
        <label className="field">
          <span>
            <FormattedMessage id="onb.dob.month" />
          </span>
          <input
            inputMode="numeric"
            autoComplete="bday-month"
            value={month}
            onChange={digits(setMonth, 2)}
            aria-invalid={invalid || undefined}
            placeholder={intl.formatNumber(12)}
            size={3}
          />
        </label>
        <label className="field">
          <span>
            <FormattedMessage id="onb.dob.year" />
          </span>
          <input
            inputMode="numeric"
            autoComplete="bday-year"
            value={year}
            onChange={digits(setYear, 4)}
            aria-invalid={invalid || undefined}
            placeholder={String(new Date().getFullYear() - 20)}
            size={5}
          />
        </label>
      </fieldset>
      {invalid && (
        <p className="error" role="alert" id="dob-error">
          <FormattedMessage id="onb.dob.invalid" />
        </p>
      )}
      <div className="actions">
        <button type="submit" className="primary" disabled={!day || !month || year.length < 4}>
          <FormattedMessage id="common.next" />
        </button>
      </div>
    </form>
  );
}

function BlockedStep({ onBack }: { onBack: () => void }) {
  return (
    <div className="calm">
      <div className="calm-mark" aria-hidden="true">
        🌱
      </div>
      <h1>
        <FormattedMessage id="onb.blocked.title" />
      </h1>
      <p>
        <FormattedMessage id="onb.blocked.body" />
      </p>
      <div className="actions">
        <button type="button" className="secondary" onClick={onBack}>
          <FormattedMessage id="onb.blocked.back" />
        </button>
      </div>
    </div>
  );
}

const PREP_STEPS: CoreEvents['identity.progress']['step'][] = ['keys', 'storage', 'encryption', 'done'];

function PreparingStep() {
  const [progress, setProgress] = useState<CoreEvents['identity.progress']>({ step: 'keys', percent: 5 });
  useCoreEvent('identity.progress', setProgress);
  const current = PREP_STEPS.indexOf(progress.step);
  const intl = useIntl();
  return (
    <div className="preparing">
      <h1>
        <FormattedMessage id="onb.preparing.title" />
      </h1>
      <progress
        max={100}
        value={progress.percent}
        aria-label={intl.formatMessage({ id: 'onb.preparing.title' })}
      />
      <ul className="prep-steps" aria-live="polite">
        {PREP_STEPS.map((s, i) => (
          <li key={s} className={i < current || progress.step === 'done' ? 'done' : i === current ? 'active' : ''}>
            {(i < current || progress.step === 'done') && <CheckIcon />}
            <FormattedMessage id={`onb.preparing.${s}`} />
          </li>
        ))}
      </ul>
    </div>
  );
}

function WordsStep({ words, onNext }: { words: string[]; onNext: () => void }) {
  return (
    <>
      <h1>
        <FormattedMessage id="onb.words.title" />
      </h1>
      <WordsWarning />
      <WordGrid words={words} />
      <div className="actions">
        <button type="button" className="primary" onClick={onNext}>
          <FormattedMessage id="onb.words.written" />
        </button>
      </div>
    </>
  );
}

const AVATAR_TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];
const AVATAR_MAX_BYTES = 512 * 1024;

function ProfileStep({ onDone }: { onDone: () => void }) {
  const core = useCore();
  const store = useAppStore();
  const identity = store.getState().identity!;
  const [name, setName] = useState(identity.displayName);
  const [bio, setBio] = useState(identity.bio ?? '');
  const [avatar, setAvatar] = useState<string | undefined>(identity.avatarDataUrl);
  const [avatarError, setAvatarError] = useState(false);
  const intl = useIntl();

  const save = async (e: FormEvent) => {
    e.preventDefault();
    if (name.trim()) await core.updateProfile({ displayName: name.trim(), bio: bio.trim() || undefined, avatarDataUrl: avatar });
    onDone();
  };

  return (
    <form onSubmit={save}>
      <h1>
        <FormattedMessage id="onb.profile.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id="onb.profile.hint" />
      </p>
      <div className="profile-edit">
        <Avatar name={name || '?'} color={identity.avatarColor} src={avatar} size={72} />
        <label className="file-button secondary">
          <FormattedMessage id="onb.profile.avatarPick" />
          <input
            type="file"
            accept={AVATAR_TYPES.join(',')}
            className="visually-hidden"
            aria-label={intl.formatMessage({ id: 'onb.profile.avatar' })}
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (!f) return;
              setAvatarError(false);
              if (!AVATAR_TYPES.includes(f.type) || f.size > AVATAR_MAX_BYTES) {
                setAvatarError(true);
                return;
              }
              const r = new FileReader();
              r.onload = () => {
                const src = safeImageSrc(r.result);
                setAvatar(src);
                setAvatarError(!src);
              };
              r.readAsDataURL(f);
            }}
          />
        </label>
      </div>
      {avatarError && (
        <p className="error" role="alert">
          <FormattedMessage id="onb.profile.avatarTooBig" />
        </p>
      )}
      <label className="field">
        <span>
          <FormattedMessage id="onb.profile.name" />
        </span>
        <input dir="auto" value={name} maxLength={32} autoFocus onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field">
        <span>
          <FormattedMessage id="onb.profile.bio" />
        </span>
        <textarea dir="auto" value={bio} maxLength={190} rows={2} onChange={(e) => setBio(e.target.value)} />
      </label>
      <div className="code-box">
        <span>
          <FormattedMessage id="onb.profile.code" />
        </span>
        <Code value={identity.shortCode} />
        <p className="muted small">
          <FormattedMessage id="onb.profile.codeHint" />
        </p>
      </div>
      <div className="actions">
        <button type="submit" className="primary" disabled={!name.trim()}>
          <FormattedMessage id="common.done" />
        </button>
        <button type="button" className="link" onClick={onDone}>
          <FormattedMessage id="common.skip" />
        </button>
      </div>
    </form>
  );
}
