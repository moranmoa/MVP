// The recovery-words screen and the 3-word check (B19, spec
// docs/ux/specs/B19-recovery-words.md). The words are never in the page until
// someone chooses to show them, and the check uses the challenge the core
// picked, so nothing here can be used to guess the words one by one.

import { useEffect, useRef, useState, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { isCoreError, useCore } from '../../core';
import { AlertIcon, CheckIcon } from '../../components/icons';

/**
 * Numbered grid of the 12 words, read down the columns (1–4, 5–8, 9–12) and
 * always LTR. Without `words` it shows only dots: the real words are not in
 * the DOM, so a stream, a screenshot or a screen reader can't pick them up.
 */
export function WordGrid({ words }: { words: string[] | null }) {
  const intl = useIntl();
  return (
    <ol
      id="word-grid"
      className={`word-grid words-shown ${words ? '' : 'words-hidden'}`}
      dir="ltr"
      aria-label={intl.formatMessage({ id: words ? 'onb.words.gridLabel' : 'onb.words.gridHidden' })}
      // The words are meant for paper. Copying would leave them in the clipboard,
      // where other apps and clipboard history can read them.
      onCopy={(e) => e.preventDefault()}
      onCut={(e) => e.preventDefault()}
      onDragStart={(e) => e.preventDefault()}
    >
      {Array.from({ length: 12 }, (_, i) => (
        <li key={i} aria-hidden={words ? undefined : true}>
          <span className="word-num">{i + 1}.</span>
          {words ? (
            <span className="word">{words[i]}</span>
          ) : (
            <span className="word word-mask">•••••</span>
          )}
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

type Load = { state: 'hidden' } | { state: 'loading' } | { state: 'shown'; words: string[] } | { state: 'error'; cancelled: boolean };

/**
 * The words screen: warning, hidden grid, one show/hide button, and "I wrote
 * them down", which only works after the words were shown once. `load` gets
 * the words (from memory in onboarding, `getRecoveryWords()` after a reload or
 * from the reminder, where the real core asks for OS authentication).
 */
export function RecoveryWordsPanel({ load, onDone }: { load: () => Promise<string[]>; onDone: () => void }) {
  const intl = useIntl();
  const [view, setView] = useState<Load>({ state: 'hidden' });
  const [seen, setSeen] = useState(false);

  const toggle = async () => {
    if (view.state === 'shown') {
      setView({ state: 'hidden' });
      return;
    }
    setView({ state: 'loading' });
    try {
      const words = await load();
      setView({ state: 'shown', words });
      setSeen(true);
    } catch (e) {
      setView({ state: 'error', cancelled: isCoreError(e) && e.code === 'auth-cancelled' });
    }
  };

  const shown = view.state === 'shown';
  return (
    <>
      <WordsWarning />
      {intl.locale === 'he' && (
        <p className="muted small words-english">
          <FormattedMessage id="onb.words.english" />
        </p>
      )}
      <div className="word-grid-wrap">
        <WordGrid words={shown ? view.words : null} />
        {!shown && (
          <div className="word-reveal">
            {view.state === 'error' ? (
              <p className="error" role="alert">
                <FormattedMessage id={view.cancelled ? 'onb.words.authCancelled' : 'onb.words.loadError'} />
              </p>
            ) : (
              <p className="muted small">
                <FormattedMessage id="onb.words.revealHint" />
              </p>
            )}
          </div>
        )}
      </div>
      <div className="actions">
        <button
          type="button"
          // The only thing to do before the first reveal, so it leads until then.
          className={seen ? 'secondary' : 'primary'}
          aria-expanded={shown}
          aria-controls="word-grid"
          disabled={view.state === 'loading'}
          onClick={() => void toggle()}
        >
          <FormattedMessage
            id={
              view.state === 'loading'
                ? 'common.loading'
                : shown
                  ? 'onb.words.hide'
                  : view.state === 'error'
                    ? 'common.retry'
                    : 'onb.words.reveal'
            }
          />
        </button>
        <button
          type="button"
          className="primary"
          // aria-disabled keeps it in the Tab order so the hint is read out.
          aria-disabled={!seen || undefined}
          aria-describedby={seen ? undefined : 'words-written-hint'}
          onClick={() => seen && onDone()}
        >
          <FormattedMessage id="onb.words.written" />
        </button>
      </div>
      {!seen && (
        <p id="words-written-hint" className="muted small">
          <FormattedMessage id="onb.words.writtenHint" />
        </p>
      )}
    </>
  );
}

/**
 * Asks for the 3 words of the core's current challenge. Used in onboarding
 * and from the reminder banner. Only the wrong fields are marked; after 5
 * failed checks the core burns the challenge and the words must be shown again.
 */
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
  const [positions, setPositions] = useState<number[] | null>(null);
  const [answers, setAnswers] = useState<Record<number, string>>({});
  const [wrong, setWrong] = useState<number[]>([]);
  const [tooMany, setTooMany] = useState(false);
  const [ok, setOk] = useState(false);
  const [busy, setBusy] = useState(false);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);
  const complete = !!positions && positions.every((p) => (answers[p] ?? '').trim());

  useEffect(() => {
    core.getRecoveryChallenge().then(
      (c) => setPositions(c.positions),
      (e) => {
        if (isCoreError(e) && e.code === 'too-many-attempts') setTooMany(true);
      },
    );
  }, [core]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!complete || busy || tooMany) return;
    setBusy(true);
    try {
      const result = await core.checkRecoveryWords(answers);
      if (result.ok) {
        setOk(true);
        setTimeout(onChecked, 600);
        return;
      }
      setWrong(result.wrong);
      inputs.current[positions!.indexOf(result.wrong[0])]?.focus();
    } catch (err) {
      if (isCoreError(err) && err.code === 'too-many-attempts') setTooMany(true);
    } finally {
      setBusy(false);
    }
  };

  const errorId = 'check-error';
  return (
    <form onSubmit={submit} className="stack check-form">
      <p className="muted">
        <FormattedMessage id="onb.check.hint" />
      </p>
      <div className="check-fields" aria-busy={!positions && !tooMany}>
        {(positions ?? [0, 0, 0]).map((p, i) =>
          p ? (
            <label key={p} className="field">
              <span>{intl.formatMessage({ id: 'onb.check.word' }, { n: p })}</span>
              <input
                ref={(el) => {
                  inputs.current[i] = el;
                }}
                dir="ltr"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                autoFocus={i === 0}
                disabled={tooMany || ok}
                value={answers[p] ?? ''}
                aria-invalid={wrong.includes(p) || undefined}
                aria-describedby={wrong.length || tooMany ? errorId : undefined}
                onChange={(e) => {
                  setWrong((w) => w.filter((x) => x !== p));
                  setAnswers({ ...answers, [p]: e.target.value });
                }}
              />
            </label>
          ) : (
            <span key={i} className="field skeleton-field" aria-hidden="true" />
          ),
        )}
      </div>
      {tooMany ? (
        <p id={errorId} className="error" role="alert">
          <FormattedMessage id="onb.check.tooMany" />
        </p>
      ) : (
        wrong.length > 0 && (
          <p id={errorId} className="error" role="alert">
            <FormattedMessage
              id="onb.check.wrong"
              values={{ count: wrong.length, positions: intl.formatList(wrong.map(String), { type: 'conjunction' }) }}
            />
          </p>
        )
      )}
      {ok && (
        <p className="success" role="status">
          <CheckIcon /> <FormattedMessage id="onb.check.ok" />
        </p>
      )}
      <div className="actions">
        {!tooMany && (
          <button type="submit" className="primary" disabled={!complete || busy || ok}>
            <FormattedMessage id="onb.check.verify" />
          </button>
        )}
        {onShowAgain && (
          <button type="button" className={tooMany ? 'primary' : 'link'} onClick={onShowAgain}>
            <FormattedMessage id={tooMany ? 'onb.words.reveal' : 'onb.check.showAgain'} />
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
