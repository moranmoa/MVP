import { useMemo, useState, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useCore } from '../../core';
import { AlertIcon } from '../../components/icons';

/** Numbered grid of the 12 words. Always LTR, in every language. */
export function WordGrid({ words }: { words: string[] }) {
  const intl = useIntl();
  return (
    <ol
      className="word-grid words-shown"
      dir="ltr"
      aria-label={intl.formatMessage({ id: 'onb.words.gridLabel' })}
      // The words are meant for paper. Copying would leave them in the clipboard,
      // where other apps and clipboard history can read them.
      onCopy={(e) => e.preventDefault()}
      onCut={(e) => e.preventDefault()}
      onDragStart={(e) => e.preventDefault()}
    >
      {words.map((w, i) => (
        <li key={i}>
          <span className="word-num" aria-hidden="true">
            {i + 1}
          </span>
          <span className="word">{w}</span>
        </li>
      ))}
    </ol>
  );
}

export function WordsWarning() {
  return (
    <div className="warning" role="note">
      <AlertIcon />
      <div>
        <p>
          <strong>
            <FormattedMessage id="onb.words.warning" />
          </strong>
        </p>
        <p>
          <FormattedMessage id="onb.words.tip" />
        </p>
      </div>
    </div>
  );
}

function pickPositions(): number[] {
  const all = Array.from({ length: 12 }, (_, i) => i + 1);
  for (let i = all.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [all[i], all[j]] = [all[j], all[i]];
  }
  return all.slice(0, 3).sort((a, b) => a - b);
}

/** Asks for 3 of the 12 words. Used in onboarding and from the reminder banner. */
export function RecoveryCheck({
  onChecked,
  onLater,
  onShowAgain,
}: {
  onChecked: () => void;
  onLater?: () => void;
  onShowAgain?: () => void;
}) {
  const core = useCore();
  const intl = useIntl();
  const positions = useMemo(pickPositions, []);
  const [answers, setAnswers] = useState<Record<number, string>>({});
  const [wrong, setWrong] = useState(false);
  const [busy, setBusy] = useState(false);
  const complete = positions.every((p) => (answers[p] ?? '').trim());

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!complete) return;
    setBusy(true);
    const ok = await core.checkRecoveryWords(answers);
    setBusy(false);
    if (ok) onChecked();
    else setWrong(true);
  };

  return (
    <form onSubmit={submit} className="stack">
      <p className="muted">
        <FormattedMessage id="onb.check.hint" />
      </p>
      <div className="check-fields">
        {positions.map((p, i) => (
          <label key={p} className="field">
            <span>{intl.formatMessage({ id: 'onb.check.word' }, { n: p })}</span>
            <input
              dir="ltr"
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              autoFocus={i === 0}
              value={answers[p] ?? ''}
              aria-invalid={wrong || undefined}
              onChange={(e) => {
                setWrong(false);
                setAnswers({ ...answers, [p]: e.target.value });
              }}
            />
          </label>
        ))}
      </div>
      {wrong && (
        <p className="error" role="alert">
          <FormattedMessage id="onb.check.wrong" />
        </p>
      )}
      <div className="actions">
        <button type="submit" className="primary" disabled={!complete || busy}>
          <FormattedMessage id="onb.check.verify" />
        </button>
        {onShowAgain && (
          <button type="button" className="link" onClick={onShowAgain}>
            <FormattedMessage id="onb.check.showAgain" />
          </button>
        )}
      </div>
      {onLater && (
        <div className="later">
          <button type="button" className="secondary" onClick={onLater}>
            <FormattedMessage id="onb.check.later" />
          </button>
          <p className="muted small">
            <FormattedMessage id="onb.check.laterNote" />
          </p>
        </div>
      )}
    </form>
  );
}
