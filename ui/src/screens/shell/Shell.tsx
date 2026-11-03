import { useEffect, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { PHONE_QUERY, useMediaQuery } from '../../app/media';
import type { Id } from '../../core';
import { Dialog } from '../../components/common';
import { ShieldIcon } from '../../components/icons';
import { RecoveryCheck } from '../onboarding/RecoveryWords';
import { SettingsScreen } from '../settings/SettingsScreen';
import { ChannelSidebar } from './ChannelSidebar';
import { ChatView } from './ChatView';
import { CreateServerDialog, Home, JoinDialog } from './Home';
import { MemberList } from './MemberList';
import { ServerRail } from './ServerRail';

const MEMBERS_KEY = 'mvp.ui.membersOpen';

export type ShellDialog = 'create' | 'join' | 'check' | null;

export function Shell() {
  const store = useAppStore();
  const identity = useStore(store, (s) => s.identity)!;
  const servers = useStore(store, (s) => s.servers);
  const notice = useStore(store, (s) => s.notice);
  const intl = useIntl();

  const [serverId, setServerId] = useState<Id | 'home'>(() => servers[0]?.id ?? 'home');
  const [channelByServer, setChannelByServer] = useState<Record<Id, Id>>({});
  const [membersOpen, setMembersOpen] = useState(() => {
    const stored = localStorage.getItem(MEMBERS_KEY);
    return stored === null ? window.innerWidth >= 1000 : stored === '1';
  });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [dialog, setDialog] = useState<ShellDialog>(null);
  // On a phone the channel list and the chat are two pages; this is the one shown.
  const phone = useMediaQuery(PHONE_QUERY);
  const [phoneView, setPhoneView] = useState<'nav' | 'chat'>('nav');

  const channels = useStore(store, (s) => (serverId === 'home' ? undefined : s.channels[serverId]));
  const channelId = serverId === 'home' ? undefined : (channelByServer[serverId] ?? channels?.[0]?.id);
  const server = servers.find((s) => s.id === serverId);

  useEffect(() => {
    if (serverId !== 'home' && !servers.some((s) => s.id === serverId)) setServerId(servers[0]?.id ?? 'home');
  }, [servers, serverId]);

  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => store.clearNotice(), 6000);
    return () => clearTimeout(t);
  }, [notice, store]);

  // Alt+Up / Alt+Down move between channels, Ctrl+Alt+Up / Down between servers.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
      const dir = e.key === 'ArrowUp' ? -1 : 1;
      e.preventDefault();
      if (e.ctrlKey) {
        const ids: (Id | 'home')[] = ['home', ...servers.map((s) => s.id)];
        const next = ids[(ids.indexOf(serverId) + dir + ids.length) % ids.length];
        setServerId(next);
      } else if (server && channels?.length && channelId) {
        const i = channels.findIndex((c) => c.id === channelId);
        const next = channels[(i + dir + channels.length) % channels.length];
        setChannelByServer((m) => ({ ...m, [server.id]: next.id }));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [servers, serverId, server, channels, channelId]);

  const toggleMembers = () => {
    setMembersOpen((o) => {
      localStorage.setItem(MEMBERS_KEY, o ? '0' : '1');
      return !o;
    });
  };

  const selectServer = (id: Id | 'home') => {
    setServerId(id);
    setPhoneView('nav');
  };
  const showMembers = !!server && membersOpen && (!phone || phoneView === 'chat');
  const layout = [
    'shell',
    showMembers ? 'with-members' : '',
    server ? '' : 'no-sidebar',
    phone && server ? `phone phone-${phoneView}` : '',
  ];

  return (
    <div className={layout.filter(Boolean).join(' ')}>
      <a className="skip-link" href="#chat-main">
        <FormattedMessage id="a11y.skipToChat" />
      </a>
      {!identity.recoveryChecked && (
        <div className="banner" role="status">
          <ShieldIcon />
          <span>
            <FormattedMessage id="banner.recovery" />
          </span>
          <button type="button" className="secondary small" onClick={() => setDialog('check')}>
            <FormattedMessage id="banner.recovery.action" />
          </button>
        </div>
      )}
      <ServerRail
        selected={serverId}
        onSelect={selectServer}
        onAdd={() => selectServer('home')}
        onOpenSettings={() => setSettingsOpen(true)}
      />
      {server && channelId && (
        <ChannelSidebar
          server={server}
          channelId={channelId}
          onSelectChannel={(id) => {
            setChannelByServer((m) => ({ ...m, [server.id]: id }));
            setPhoneView('chat');
          }}
          onOpenSettings={() => setSettingsOpen(true)}
        />
      )}
      <main id="chat-main" className="main" tabIndex={-1}>
        {server && channelId ? (
          <ChatView
            key={channelId}
            server={server}
            channelId={channelId}
            membersOpen={membersOpen}
            onToggleMembers={toggleMembers}
            onBack={phone ? () => setPhoneView('nav') : undefined}
          />
        ) : (
          <Home
            hasServers={servers.length > 0}
            onCreate={() => setDialog('create')}
            onJoin={() => setDialog('join')}
          />
        )}
      </main>
      {showMembers && <MemberList server={server} onClose={phone ? toggleMembers : undefined} />}

      <CreateServerDialog
        open={dialog === 'create'}
        onClose={() => setDialog(null)}
        onNeedCheck={() => setDialog('check')}
        onOpenServer={(id) => {
          setDialog(null);
          setServerId(id);
        }}
      />
      <JoinDialog
        open={dialog === 'join'}
        onClose={() => setDialog(null)}
        onJoined={(id) => {
          setDialog(null);
          setServerId(id);
        }}
      />
      <Dialog open={dialog === 'check'} onClose={() => setDialog(null)} title={intl.formatMessage({ id: 'onb.check.title' })}>
        <RecoveryCheck onChecked={() => setDialog(null)} />
      </Dialog>
      <SettingsScreen open={settingsOpen} onClose={() => setSettingsOpen(false)} onCheckWords={() => setDialog('check')} />

      {notice && (
        <div className="toast" role="status">
          <ShieldIcon />
          <FormattedMessage id="notice.encryptionRenewed" />
        </div>
      )}
    </div>
  );
}
