import { useEffect, useRef, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { PHONE_QUERY, useMediaQuery } from '../../app/media';
import { useCore, type Id } from '../../core';
import { Dialog } from '../../components/common';
import { ShieldIcon } from '../../components/icons';
import { RecoveryCheck, RecoveryWordsPanel } from '../onboarding/RecoveryWords';
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
  const core = useCore();
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
  // The reminder can show the words again (the panel loads them on demand).
  const [showWords, setShowWords] = useState(false);
  const closeCheck = () => {
    setShowWords(false);
    setDialog(null);
  };
  // On a phone the channel list and the chat are two pages; this is the one shown.
  const phone = useMediaQuery(PHONE_QUERY);
  const [phoneView, setPhoneView] = useState<'nav' | 'chat'>('nav');
  // The phone member sheet has its own state: it is never saved, and every
  // page switch closes it, so a channel tap always lands on the chat.
  const [phoneMembers, setPhoneMembers] = useState(false);
  // Where focus goes after a phone page switch or after the sheet closes.
  const focusNext = useRef<'chat' | 'channel' | 'members-button' | null>(null);
  const shellRef = useRef<HTMLDivElement>(null);

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
    if (phone) {
      if (phoneMembers) focusNext.current = 'members-button';
      setPhoneMembers((o) => !o);
      return;
    }
    setMembersOpen((o) => {
      localStorage.setItem(MEMBERS_KEY, o ? '0' : '1');
      return !o;
    });
  };

  const goPhone = (view: 'nav' | 'chat') => {
    setPhoneMembers(false);
    setPhoneView(view);
    if (phone) focusNext.current = view === 'chat' ? 'chat' : 'channel';
  };
  const selectServer = (id: Id | 'home') => {
    setServerId(id);
    setPhoneMembers(false);
    setPhoneView('nav');
  };
  const sheetOpen = phone && phoneMembers && phoneView === 'chat';
  const showMembers = !!server && (phone ? sheetOpen : membersOpen);

  // Keyboard and screen-reader users keep their place when the phone pages switch:
  // the chat heading after opening a channel, the open channel after going back,
  // and the members button after the sheet closes.
  useEffect(() => {
    const target = focusNext.current;
    if (!target) return;
    focusNext.current = null;
    const root = shellRef.current;
    const el =
      target === 'chat'
        ? root?.querySelector<HTMLElement>('.chat-title')
        : target === 'channel'
          ? root?.querySelector<HTMLElement>('.channel[aria-current="page"]')
          : root?.querySelector<HTMLElement>('.members-button');
    el?.focus();
  });

  // While the sheet covers the screen, everything under it is inert.
  useEffect(() => {
    const root = shellRef.current;
    if (!root || !sheetOpen) return;
    const under = [...root.children].filter((el) => !el.classList.contains('members')) as HTMLElement[];
    under.forEach((el) => (el.inert = true));
    return () => under.forEach((el) => (el.inert = false));
  }, [sheetOpen]);
  const layout = [
    'shell',
    showMembers ? 'with-members' : '',
    server ? '' : 'no-sidebar',
    phone && server ? `phone phone-${phoneView}` : '',
  ];

  return (
    <div ref={shellRef} className={layout.filter(Boolean).join(' ')}>
      {!(phone && server && phoneView === 'nav') && (
        <a className="skip-link" href="#chat-main">
          <FormattedMessage id="a11y.skipToChat" />
        </a>
      )}
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
            goPhone('chat');
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
            membersOpen={phone ? phoneMembers : membersOpen}
            onToggleMembers={toggleMembers}
            onBack={phone ? () => goPhone('nav') : undefined}
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
      <Dialog
        open={dialog === 'check'}
        onClose={closeCheck}
        title={intl.formatMessage({ id: showWords ? 'onb.words.title' : 'onb.check.title' })}
      >
        {showWords ? (
          <RecoveryWordsPanel load={() => core.getRecoveryWords()} onDone={() => setShowWords(false)} />
        ) : (
          // The words can be shown again here, so a reload during onboarding never
          // leaves an account that can't pass the check (QA-01).
          <RecoveryCheck onChecked={closeCheck} onShowAgain={() => setShowWords(true)} />
        )}
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
