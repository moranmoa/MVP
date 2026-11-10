// Sign out of this computer (#304). The account stays on the network: this
// computer forgets it, and only the 12 recovery words bring it back. So the
// confirm asks for the words; "I don't have my words" is a separate path with
// a stronger warning.

import { useRef, useState, type ClipboardEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { Dialog } from '../../components/common';
import { AlertIcon } from '../../components/icons';
import { isCoreError, useCore } from '../../core';

type Step = 'explain' | 'words' | 'noWords';

/** Best effort, for a browser: the desktop shell clears its own storage too (#304). */
async function clearPageStorage(): Promise<void> {
  try {
    localStorage.clear();
    sessionStorage.clear();
  } catch {
    // storage may be unavailable
  }
  try {
    if (typeof caches !== 'undefined') for (const k of await caches.keys()) await caches.delete(k);
  } catch {
    // ignore
  }
  try {
    const dbs = typeof indexedDB !== 'undefined' && indexedDB.databases ? await indexedDB.databases() : [];
    for (const d of dbs) if (d.name) indexedDB.deleteDatabase(d.name);
  } catch {
    // ignore
  }
}
type Failure = 'incomplete' | 'invalid' | 'other-account' | 'keyring' | 'other';

export function SignOutButton() {
  const [open, setOpen] = useState(false);
  return (
    <div className="sign-out">
      <button type="button" className="danger" onClick={() => setOpen(true)}>
        <FormattedMessage id="signOut.button" />
      </button>
      <p className="muted small">
        <FormattedMessage id="signOut.buttonHint" />
      </p>
      {open && <SignOutDialog onClose={() => setOpen(false)} />}
    </div>
  );
}

function SignOutDialog({ onClose }: { onClose: () => void }) {
  const intl = useIntl();
  const core = useCore();
  const store = useAppStore();
  const [step, setStep] = useState<Step>('explain');
  const [words, setWords] = useState<string[]>(() => Array(12).fill(''));
  const [bad, setBad] = useState<number[]>([]);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [typed, setTyped] = useState('');
  const phrase = intl.formatMessage({ id: 'signOut.noWordsPhrase' });
  const typedOk = typed.trim().toLowerCase() === phrase.toLowerCase();
  const [busy, setBusy] = useState(false);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);

  const setWord = (i: number, v: string) => {
    setWords((ws) => ws.map((w, j) => (j === i ? v : w)));
    setBad((b) => b.filter((p) => p !== i + 1));
    setFailure(null);
  };
  // Pasting several words fills the following boxes too (as on the restore screen).
  const onPaste = (i: number) => (e: ClipboardEvent<HTMLInputElement>) => {
    const parts = e.clipboardData.getData('text').trim().split(/[\s,]+/).filter(Boolean);
    if (parts.length < 2) return;
    e.preventDefault();
    setWords((ws) => ws.map((w, j) => (j >= i && j - i < parts.length ? parts[j - i].toLowerCase() : w)));
    inputs.current[Math.min(11, i + parts.length - 1)]?.focus();
  };

  const signOut = async (withWords: boolean) => {
    if (withWords && words.some((w) => !w.trim())) {
      setFailure('incomplete');
      inputs.current[words.findIndex((w) => !w.trim())]?.focus();
      return;
    }
    setBusy(true);
    setFailure(null);
    try {
      if (withWords) await core.signOut(words.map((w) => w.trim().toLowerCase()));
      else await core.signOut(undefined, true);
      // The core exits and the desktop shell restarts it (and clears the page's
      // storage); nothing to wait for. In a browser, clear what we can here.
      await clearPageStorage();
      store.signedOut();
    } catch (e) {
      setBusy(false);
      const code = isCoreError(e) ? e.code : '';
      if (code === 'cancelled') return; // the native dialog was declined: nothing happened
      if (code === 'invalid-words') {
        const positions = (e as { positions?: number[] }).positions ?? [];
        setBad(positions);
        setFailure('invalid');
        inputs.current[(positions[0] ?? 1) - 1]?.focus();
      } else if (code === 'not-allowed') setFailure('other-account');
      else if (code === 'keyring-unavailable') setFailure('keyring');
      else setFailure('other');
    }
  };

  const failureText =
    failure === 'invalid'
      ? intl.formatMessage({ id: 'restore.invalid' }, { count: bad.length, positions: intl.formatList(bad.map(String)) })
      : failure && intl.formatMessage({ id: `signOut.error.${failure}` });

  return (
    <Dialog open onClose={() => !busy && onClose()} title={intl.formatMessage({ id: 'signOut.title' })} className="sign-out-dialog">
      {step === 'explain' && (
        <>
          <ul className="sign-out-facts">
            <li>
              <FormattedMessage id="signOut.fact.thisComputer" />
            </li>
            <li>
              <FormattedMessage id="signOut.fact.network" />
            </li>
            <li>
              <strong>
                <FormattedMessage id="signOut.fact.words" />
              </strong>
            </li>
          </ul>
          <div className="actions">
            <button type="button" className="primary" autoFocus onClick={() => setStep('words')}>
              <FormattedMessage id="signOut.next" />
            </button>
            <button type="button" className="secondary" onClick={onClose}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
        </>
      )}

      {step === 'words' && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void signOut(true);
          }}
        >
          <p>
            <FormattedMessage id="signOut.wordsHint" />
          </p>
          <ol className="word-grid word-inputs" dir="ltr">
            {words.map((w, i) => (
              <li key={i}>
                <span className="word-num" aria-hidden="true">
                  {i + 1}
                </span>
                <input
                  ref={(el) => {
                    inputs.current[i] = el;
                  }}
                  value={w}
                  autoFocus={i === 0}
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  readOnly={busy}
                  aria-label={intl.formatMessage({ id: 'restore.word' }, { n: i + 1 })}
                  aria-invalid={bad.includes(i + 1) || undefined}
                  onChange={(e) => setWord(i, e.target.value.trim().toLowerCase())}
                  onPaste={onPaste(i)}
                />
              </li>
            ))}
          </ol>
          {failureText && (
            <p className="error" role="alert">
              {failureText}
            </p>
          )}
          <div className="actions">
            <button type="submit" className="danger" disabled={busy}>
              <FormattedMessage id={busy ? 'signOut.busy' : 'signOut.confirm'} />
            </button>
            <button type="button" className="secondary" disabled={busy} onClick={onClose}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
          <button
            type="button"
            className="link small sign-out-no-words"
            disabled={busy}
            onClick={() => {
              setFailure(null);
              setStep('noWords');
            }}
          >
            <FormattedMessage id="signOut.noWords" />
          </button>
        </form>
      )}

      {step === 'noWords' && (
        <>
          <div className="warning danger-zone">
            <AlertIcon />
            <p>
              <FormattedMessage id="signOut.noWordsWarning" />
            </p>
          </div>
          <label className="field">
            <span>
              <FormattedMessage id="signOut.noWordsType" values={{ phrase: <strong>{phrase}</strong> }} />
            </span>
            <input value={typed} autoComplete="off" spellCheck={false} readOnly={busy} onChange={(e) => setTyped(e.target.value)} />
          </label>
          {failureText && (
            <p className="error" role="alert">
              {failureText}
            </p>
          )}
          <div className="actions">
            <button type="button" className="danger" disabled={!typedOk || busy} onClick={() => void signOut(false)}>
              <FormattedMessage id={busy ? 'signOut.busy' : 'signOut.noWordsConfirm'} />
            </button>
            <button type="button" className="secondary" disabled={busy} onClick={() => setStep('words')}>
              <FormattedMessage id="signOut.back" />
            </button>
          </div>
        </>
      )}
    </Dialog>
  );
}
