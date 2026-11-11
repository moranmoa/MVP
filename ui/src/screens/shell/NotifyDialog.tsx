// G-61: a server's or a channel's notification level and mute, opened from
// its context menu ("Notifications"). Saved on this computer only.

import { FormattedMessage, FormattedTime, useIntl } from 'react-intl';
import { defaultNotifyPrefs, isMuted, MUTE_HOURS, withScope, type NotifyLevel } from '../../app/notify';
import { useSettings } from '../../app/settings';
import { Dialog, Name } from '../../components/common';
import type { Id } from '../../core';

type MuteChoice = 'off' | `${(typeof MUTE_HOURS)[number]}h` | 'forever';
const MUTES: MuteChoice[] = ['off', ...MUTE_HOURS.map((h) => `${h}h` as const), 'forever'];

export function NotifyDialog({
  kind,
  id,
  name,
  onClose,
}: {
  kind: 'server' | 'channel';
  id: Id;
  name: string;
  onClose: () => void;
}) {
  const { settings, update } = useSettings();
  const intl = useIntl();
  const prefs = settings.notify ?? defaultNotifyPrefs();
  const key = kind === 'server' ? 'servers' : 'channels';
  const scope = prefs[key][id];
  // A channel can follow its server ('inherit') or choose its own level.
  const levels: (NotifyLevel | 'inherit')[] = kind === 'channel' ? ['inherit', 'all', 'mentions', 'none'] : ['all', 'mentions', 'none'];
  const level = scope?.level ?? (kind === 'channel' ? 'inherit' : 'mentions');
  const muted = isMuted(scope, Date.now());
  const until = scope?.mutedUntil;

  const setLevel = (l: NotifyLevel | 'inherit') => update({ notify: withScope(prefs, key, id, { level: l === 'inherit' ? null : l }) });
  const setMute = (m: MuteChoice) =>
    update({
      notify: withScope(prefs, key, id, {
        mutedUntil: m === 'off' ? null : m === 'forever' ? 'forever' : Date.now() + parseInt(m, 10) * 3_600_000,
      }),
    });

  return (
    <Dialog open onClose={onClose} title={<FormattedMessage id="notify.dialog.title" values={{ name: <Name>{name}</Name> }} />} className="notify-dialog">
      <div className="stack">
        <fieldset className="radio-group">
          <legend>
            <FormattedMessage id="notify.menu" />
          </legend>
          {levels.map((l) => (
            <label key={l} className="radio">
              <input type="radio" name={`notify-level-${id}`} checked={level === l} onChange={() => setLevel(l)} />
              <FormattedMessage id={`notify.${l}`} />
            </label>
          ))}
        </fieldset>
        <fieldset className="radio-group" aria-describedby={muted ? `notify-until-${id}` : undefined}>
          <legend>
            <FormattedMessage id="notify.mute" />
          </legend>
          {MUTES.map((m) => (
            <label key={m} className="radio">
              <input
                type="radio"
                name={`notify-mute-${id}`}
                checked={m === 'off' ? !muted : m === 'forever' && until === 'forever'}
                onChange={() => setMute(m)}
              />
              <FormattedMessage id={`notify.mute.${m}`} />
            </label>
          ))}
          {muted && (
            <p className="muted small" id={`notify-until-${id}`} role="status">
              {until === 'forever' ? (
                <FormattedMessage id="notify.mute.forever" />
              ) : (
                <FormattedMessage
                  id="notify.mutedUntil"
                  values={{ time: <FormattedTime value={until} hour="2-digit" minute="2-digit" weekday="short" /> }}
                />
              )}
            </p>
          )}
        </fieldset>
        <p className="muted small">
          <FormattedMessage id="notify.localHint" />
        </p>
        <div className="actions">
          <button type="button" className="secondary" onClick={onClose}>
            {intl.formatMessage({ id: 'common.close' })}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
