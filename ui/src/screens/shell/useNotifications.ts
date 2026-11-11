// G-61: turns new messages into desktop notifications (through the shell's
// `window.mvs.notify`) and opens the channel when one is clicked.

import { useEffect, useRef } from 'react';
import { useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { isMuted } from '../../app/mute';
import { createGrouper, defaultNotifyPrefs, shellNotify, shouldNotify } from '../../app/notify';
import { useSettings } from '../../app/settings';
import { useCoreEvent, type Id, type Message } from '../../core';
import { displayName } from './MemberList';

/**
 * `viewing`: the channel on screen now (null on Home or in Settings).
 * `open`: shows a server's channel, for a click on a notification.
 */
export function useDesktopNotifications(viewing: Id | null, open: (serverId: Id, channelId: Id) => void) {
  const store = useAppStore();
  const intl = useIntl();
  const prefs = useSettings().settings.notify ?? defaultNotifyPrefs();
  const now = useRef({ viewing, prefs, intl, open });
  now.current = { viewing, prefs, intl, open };

  // Where a channel is, and what to call it, from what the store already has.
  const lookup = (channelId: Id) => {
    const s = store.getState();
    for (const [serverId, list] of Object.entries(s.channels)) {
      const channel = list.find((c) => c.id === channelId);
      if (channel) return { s, serverId, channel, server: s.servers.find((x) => x.id === serverId) };
    }
    return null;
  };

  const grouper = useRef<ReturnType<typeof createGrouper<Message>>>(null);
  if (!grouper.current) {
    const send = (m: Message, count: number) => {
      const notify = shellNotify();
      const where = lookup(m.channelId);
      if (!notify || !where) return;
      const { intl, prefs } = now.current;
      const author = where.s.members[where.serverId]?.find((x) => x.id === m.authorId);
      const place = `#${where.channel.name} · ${where.server?.name ?? ''}`;
      const body =
        count > 1
          ? intl.formatMessage({ id: 'notify.grouped' }, { count, channel: where.channel.name })
          : prefs.preview && m.body
            ? m.body
            : intl.formatMessage({ id: 'notify.hidden' });
      void notify({
        title: count > 1 || !author ? place : `${displayName(author)} · ${place}`,
        body,
        channelId: m.channelId,
        messageId: m.id,
      }).catch(() => {});
    };
    grouper.current = createGrouper<Message>((m) => send(m, 1), send);
  }
  useEffect(() => () => grouper.current?.stop(), []);

  // Opening a channel sees whatever was waiting to be grouped.
  useEffect(() => {
    if (viewing) grouper.current?.drop(viewing);
  }, [viewing]);

  useCoreEvent('message.added', (m) => {
    if (!shellNotify()) return;
    const where = lookup(m.channelId);
    if (!where) return;
    const { viewing, prefs } = now.current;
    const author = where.s.members[where.serverId]?.find((x) => x.id === m.authorId);
    const go = shouldNotify(m, {
      prefs,
      serverId: where.serverId,
      myId: where.s.identity?.id,
      authorBlocked: author?.blocked === true,
      muted: isMuted(where.s.mutes, where.serverId, m.channelId),
      watching: viewing === m.channelId && document.hasFocus(),
      now: Date.now(),
    });
    if (go) grouper.current?.add(m.channelId, m);
  });

  useCoreEvent('notify.click', ({ channelId }) => {
    const where = lookup(channelId);
    if (where) now.current.open(where.serverId, channelId);
  });
}
