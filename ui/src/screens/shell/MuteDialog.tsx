// "Mute…" from a server's or a channel's menu (G-03, G-17; GAPS §2.4): how
// long. On this device only.

import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { MUTE_DURATIONS, type MuteDuration, type MuteKey } from '../../app/mute';
import { Dialog } from '../../components/common';

export type MuteTarget = { key: MuteKey; name: string };

export function MuteDialog({ target, onClose }: { target: MuteTarget | null; onClose: () => void }) {
  const store = useAppStore();
  const intl = useIntl();
  return (
    <Dialog open={!!target} onClose={onClose} title={intl.formatMessage({ id: 'notify.mute.title' }, { name: target?.name ?? '' })}>
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
