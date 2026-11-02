import { useRef, useState, type ClipboardEvent, type FormEvent } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { isCoreError, useCore, type Identity } from '../../core';

export function RestoreScreen({ onRestored }: { onRestored: (identity: Identity) => void }) {
  const core = useCore();
  const intl = useIntl();
  const [words, setWords] = useState<string[]>(() => Array(12).fill(''));
  const [badPositions, setBadPositions] = useState<number[]>([]);
  const [error, setError] = useState<'incomplete' | 'invalid' | 'generic' | null>(null);
  const [busy, setBusy] = useState(false);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);

  const setWord = (i: number, value: string) => {
    setError(null);
    setBadPositions((b) => b.filter((p) => p !== i + 1));
    setWords((w) => w.map((x, j) => (j === i ? value : x)));
  };

  // Pasting several words fills the following boxes too.
  const onPaste = (i: number) => (e: ClipboardEvent<HTMLInputElement>) => {
    const parts = e.clipboardData.getData('text').trim().split(/[\s,]+/).filter(Boolean);
    if (parts.length < 2) return;
    e.preventDefault();
    setError(null);
    setBadPositions([]);
    setWords((w) => w.map((x, j) => (j >= i && j - i < parts.length ? parts[j - i].toLowerCase() : x)));
    inputs.current[Math.min(11, i + parts.length - 1)]?.focus();
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (words.some((w) => !w.trim())) {
      setError('incomplete');
      return;
    }
    setBusy(true);
    try {
      onRestored(await core.restoreIdentity(words));
    } catch (err) {
      if (isCoreError(err) && err.code === 'invalid-words') {
        setBadPositions(err.positions ?? []);
        setError('invalid');
        inputs.current[(err.positions?.[0] ?? 1) - 1]?.focus();
      } else {
        setError('generic');
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit}>
      <h1>
        <FormattedMessage id="restore.title" />
      </h1>
      <p className="muted">
        <FormattedMessage id="restore.hint" />
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
              aria-label={intl.formatMessage({ id: 'restore.word' }, { n: i + 1 })}
              aria-invalid={badPositions.includes(i + 1) || undefined}
              onChange={(e) => setWord(i, e.target.value.trim().toLowerCase())}
              onPaste={onPaste(i)}
            />
          </li>
        ))}
      </ol>
      {error && (
        <p className="error" role="alert">
          {error === 'invalid' ? (
            <FormattedMessage
              id="restore.invalid"
              values={{ count: badPositions.length, positions: intl.formatList(badPositions.map(String)) }}
            />
          ) : (
            <FormattedMessage id={error === 'incomplete' ? 'restore.incomplete' : 'error.generic'} />
          )}
        </p>
      )}
      <div className="actions">
        <button type="submit" className="primary" disabled={busy}>
          <FormattedMessage id={busy ? 'restore.working' : 'restore.submit'} />
        </button>
      </div>
    </form>
  );
}
