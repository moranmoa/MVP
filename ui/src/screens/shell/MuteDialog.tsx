// "Mute…" from a server's or a channel's menu (G-03, G-17; GAPS §2.4): how
// long. On this device only.

import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { MUTE_DURATIONS, MUTE_FOREVER, type MuteDuration, type MuteKey } from '../../app/mute';
import { Dialog } from '../../components/common';

/** `name` is the plain name; the title adds the channel's # inside the isolate. */
export type MuteTarget = { key: MuteKey; name: string; kind: 'server' | 'channel' };

/** "Muted" or "Muted until 14:00" (another day: with the day), for menus and the 🔕 title. */
export function useMutedLabel(): (until: number) => string {
  const intl = useIntl();
  return (until) => {
    if (until === MUTE_FOREVER) return intl.formatMessage({ id: 'notify.muted' });
    const sameDay = new Date(until).toDateString() === new Date().toDateString();
    const time = sameDay
      ? intl.formatTime(until, { hour: '2-digit', minute: '2-digit' })
      : intl.formatDate(until, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
    return intl.formatMessage({ id: 'notify.mutedUntil' }, { time });
  };
}

export function MuteDialog({ target, onClose }: { target: MuteTarget | null; onClose: () => void }) {
  const store = useAppStore();
  const intl = useIntl();
  return (
    <Dialog open={!!target} onClose={onClose} title={intl.formatMessage({ id: target?.kind === 'channel' ? 'notify.mute.titleChannel' : 'notify.mute.titleServer' }, { name: target?.name ?? '' })}>
      {target && (
        <div className="stack">
          <div className="choice-list mute-choices">
            {(Object.keys(MUTE_DURATIONS) as MuteDuration[]).map((d, i) => (
              <button
                key={d}
                type="button"
                className="choice"
                autoFocus={i === 0}
                onClick={() => {
                  store.mute(target.key, d);
                  onClose();
                }}
              >
                <FormattedMessage id={`notify.mute.${d}`} />
              </button>
            ))}
          </div>
          <div className="actions">
            <button type="button" className="secondary" onClick={onClose}>
              <FormattedMessage id="common.cancel" />
            </button>
          </div>
        </div>
      )}
    </Dialog>
  );
}
