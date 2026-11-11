// G-61: a server's or a channel's notification level, opened from its context
// menu ("Notifications"). Saved on this computer only. Mute is its own item in
// the same menu (G-03 / G-17, MuteDialog).

import { FormattedMessage, useIntl } from 'react-intl';
import { defaultNotifyPrefs, withLevel, type NotifyLevel } from '../../app/notify';
import { useSettings } from '../../app/settings';
import { Dialog, Name } from '../../components/common';
import type { Id } from '../../core';

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
  // A channel can follow its server ('inherit') or choose its own level.
  const levels: (NotifyLevel | 'inherit')[] = kind === 'channel' ? ['inherit', 'all', 'mentions', 'none'] : ['all', 'mentions', 'none'];
  const level = prefs[key][id]?.level ?? (kind === 'channel' ? 'inherit' : 'mentions');
  const setLevel = (l: NotifyLevel | 'inherit') => update({ notify: withLevel(prefs, key, id, l === 'inherit' ? null : l) });

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
