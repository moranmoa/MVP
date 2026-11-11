// G-03 / G-17: how a mute reads ("Muted", "Muted until 14:00"). Muting itself
// is in the "Notifications" dialog (NotifyDialog, G-61).

import { useIntl } from 'react-intl';
import { MUTE_FOREVER } from '../../app/mute';

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

