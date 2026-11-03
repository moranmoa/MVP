import { useMemo, useRef, useState, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useCore } from '../../core';
import { AlertIcon } from '../../components/icons';

/**
 * Numbered grid of the 12 words, read down the columns (1–4, 5–8, 9–12) and
 * always LTR. Until `revealed`, the words are not in the page at all (only
 * dots), so a stream, a screenshot or a screen reader can't pick them up.
 */
export function WordGrid({ words, revealed = true, onReveal }: { words: string[]; revealed?: boolean; onReveal?: () => void }) {
  const intl = useIntl();
  return (
    <div className="word-grid-wrap">
    <ol
      className={`word-grid words-shown ${revealed ? '' : 'words-hidden'}`}
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
          {revealed ? (
            <span className="word">{w}</span>
          ) : (
            <span className="word word-mask" aria-hidden="true">
              •••••
            </span>
          )}
        </li>
      ))}
    </ol>
    {!revealed && (
      <div className="word-reveal">
        <p className="muted small">
          <FormattedMessage id="onb.words.hiddenHint" />
        </p>
        <button type="button" className="primary" onClick={onReveal}>
          <FormattedMessage id="onb.words.reveal" />
        </button>
      </div>
    )}
    </div>
  );
}

/**
 * The words screen: warning, hidden grid, and "I wrote them down", which only
 * works once the words were shown. Used in onboarding and to show them again
 * from the reminder (QA-01).
 */
export function RecoveryWordsPanel({ words, onDone }: { words: string[]; onDone: () => void }) {
  const [revealed, setRevealed] = useState(false);
  return (
    <>
      <WordsWarning />
      <WordGrid words={words} revealed={revealed} onReveal={() => setRevealed(true)} />
      <div className="actions">
        <button type="button" className="primary" onClick={onDone} disabled={!revealed}>
          <FormattedMessage id="onb.words.written" />
        </button>
      </div>
    </>
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
  const [wrong, setWrong] = useState<number[]>([]);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);
  const [busy, setBusy] = useState(false);
  const complete = positions.every((p) => (answers[p] ?? '').trim());

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!complete) return;
    setBusy(true);
    // One position per call, so only the wrong fields get marked (B19).
    const results = await Promise.all(positions.map((p) => core.checkRecoveryWords({ [p]: answers[p] })));
    setBusy(false);
    const bad = positions.filter((_, i) => !results[i]);
    if (!bad.length && (await core.checkRecoveryWords(answers))) {
      onChecked();
      return;
    }
    setWrong(bad);
    inputs.current[positions.indexOf(bad[0] ?? positions[0])]?.focus();
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
              ref={(el) => {
                inputs.current[i] = el;
              }}
              aria-invalid={wrong.includes(p) || undefined}
              onChange={(e) => {
                setWrong((w) => w.filter((x) => x !== p));
                setAnswers({ ...answers, [p]: e.target.value });
              }}
            />
          </label>
        ))}
      </div>
      {wrong.length > 0 && (
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
