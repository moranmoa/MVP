import { useEffect, useRef, useState } from 'react';
import { FormattedMessage, useIntl } from 'react-intl';
import { useAppStore } from '../../app/App';
import { useStore } from '../../app/store';
import { phoneQuery, useMediaQuery } from '../../app/media';
import { useSettings } from '../../app/settings';
import { paragraphs, useUiDir } from '../../app/textDir';
import { useCore, useCoreEvent, type Id } from '../../core';
import { Dialog } from '../../components/common';
import { AlertIcon, CloseIcon, RefreshIcon, ShieldIcon } from '../../components/icons';
import { RecoveryCheck, RecoveryWordsPanel } from '../onboarding/RecoveryWords';
import { SettingsScreen } from '../settings/SettingsScreen';
import { ChannelDialog, type ChannelDialogMode } from './ChannelDialog';
import { ChannelSidebar } from './ChannelSidebar';
import { ChatView } from './ChatView';
import { CreateServerDialog, Home, JoinDialog } from './Home';
import { MemberList } from './MemberList';
import { ServerRail } from './ServerRail';
import { ServerSettings } from './ServerSettings';
import { LeaveServerDialog } from './ServerLifecycle';

const MEMBERS_KEY = 'mvp.ui.membersOpen';

export type ShellDialog = 'create' | 'join' | 'check' | null;

export function Shell() {
  const store = useAppStore();
  const core = useCore();
  const identity = useStore(store, (s) => s.identity)!;
  const servers = useStore(store, (s) => s.servers);
  const notice = useStore(store, (s) => s.notice);
  const updateNote = useStore(store, (s) => s.updateNote);
  const flash = useStore(store, (s) => s.flash);
  const settingsRequest = useStore(store, (s) => s.settingsRequest);
  const serverGone = useStore(store, (s) => s.serverGone);
  const uiDir = useUiDir();
  const intl = useIntl();
  const { uiScale } = useSettings().settings;

  const [serverId, setServerId] = useState<Id | 'home'>(() => servers[0]?.id ?? 'home');
  const [channelByServer, setChannelByServer] = useState<Record<Id, Id>>({});
  const [membersOpen, setMembersOpen] = useState(() => {
    const stored = localStorage.getItem(MEMBERS_KEY);
    // Open by default only when there is room for it at the current UI scale.
    return stored === null ? window.innerWidth / uiScale >= 1000 : stored === '1';
  });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [dialog, setDialog] = useState<ShellDialog>(null);
  // An mvs:// link from the OS (desktop app) opens the join dialog pre-filled.
  // It never joins by itself: that waits for the Join button.
  const [invite, setInvite] = useState<string | undefined>();
  // The open dialog as of now, not as of the last render: the subscription
  // below is made once, and a second link can arrive before React re-renders.
  const dialogNow = useRef(dialog);
  dialogNow.current = dialog;
  useCoreEvent('link.open', ({ link }) => {
    if (!link) return; // guardCore blanks anything that isn't an exact invite link
    // While the join dialog is open, a new link must not replace the one the
    // person is looking at just before they press Join (security review of #80).
    if (dialogNow.current === 'join') return;
    dialogNow.current = 'join';
    setInvite(link);
    setDialog('join');
  });
  // The reminder can show the words again (the panel loads them on demand).
  const [showWords, setShowWords] = useState(false);
  const closeCheck = () => {
    setShowWords(false);
    setDialog(null);
  };
  // On a phone the channel list and the chat are two pages; this is the one shown.
  const phone = useMediaQuery(phoneQuery(uiScale));
  const [phoneView, setPhoneView] = useState<'nav' | 'chat'>('nav');
  // The phone member sheet has its own state: it is never saved, and every
  // page switch closes it, so a channel tap always lands on the chat.
  const [phoneMembers, setPhoneMembers] = useState(false);
  // Where focus goes after a phone page switch or after the sheet closes.
  const focusNext = useRef<'chat' | 'channel' | 'members-button' | null>(null);
  const shellRef = useRef<HTMLDivElement>(null);

  const channels = useStore(store, (s) => (serverId === 'home' ? undefined : s.channels[serverId]));
  // A channel that was deleted (or never chosen) falls back to the server's default one.
  const chosen = serverId === 'home' ? undefined : channelByServer[serverId];
  const channelId =
    serverId === 'home'
      ? undefined
      : channels?.some((c) => c.id === chosen)
        ? chosen
        : (channels?.find((c) => c.isDefault) ?? channels?.[0])?.id;
  // B11c: the owner's channel dialog, and a notice when the open channel is deleted.
  const [channelDialog, setChannelDialog] = useState<ChannelDialogMode | null>(null);
  // B11: the owner's server settings screen, for the open server.
  const [serverSettingsOpen, setServerSettingsOpen] = useState(false);
  // #261: the server whose leave confirmation is open.
  const [leaving, setLeaving] = useState<Id | null>(null);
  const [channelGone, setChannelGone] = useState<string | null>(null);
  const openChannel = useRef(channelId);
  openChannel.current = channelId;
  useCoreEvent('channel.removed', ({ channelId: gone, name }) => {
    if (gone === openChannel.current) setChannelGone(name);
  });
  useEffect(() => {
    if (!channelGone) return;
    const t = setTimeout(() => setChannelGone(null), 6000);
    return () => clearTimeout(t);
  }, [channelGone]);
  const server = servers.find((s) => s.id === serverId);

  useEffect(() => {
    if (serverId !== 'home' && !servers.some((s) => s.id === serverId)) setServerId(servers[0]?.id ?? 'home');
  }, [servers, serverId]);

  useEffect(() => {
    if (!serverGone) return;
    const t = setTimeout(() => store.clearServerGone(), 6000);
    return () => clearTimeout(t);
  }, [serverGone, store]);

  useEffect(() => {
    if (settingsRequest) setSettingsOpen(true);
  }, [settingsRequest]);

  useEffect(() => {
    if (!flash) return;
    const t = setTimeout(() => store.clearFlash(), 3000);
    return () => clearTimeout(t);
  }, [flash, store]);

  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => store.clearNotice(), 6000);
    return () => clearTimeout(t);
  }, [notice, store]);

  // Update notes stay until closed when there are release notes to read.
  useEffect(() => {
    if (!updateNote || (updateNote.kind === 'installed' && updateNote.notes)) return;
    const t = setTimeout(() => store.clearUpdateNote(), 6000);
    return () => clearTimeout(t);
  }, [updateNote, store]);

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
        onOpenServerSettings={(id) => {
          selectServer(id);
          setServerSettingsOpen(true);
        }}
        onLeaveServer={setLeaving}
      />
      <LeaveServerDialog server={servers.find((s) => s.id === leaving) ?? null} onClose={() => setLeaving(null)} />
      {server && channelId && (
        <ChannelSidebar
          server={server}
          channelId={channelId}
          onSelectChannel={(id) => {
            setChannelByServer((m) => ({ ...m, [server.id]: id }));
            goPhone('chat');
          }}
          onOpenSettings={() => setSettingsOpen(true)}
          onCreateChannel={() => setChannelDialog({ kind: 'create', serverId: server.id })}
          onEditChannel={(channel) => setChannelDialog({ kind: 'edit', channel })}
          onDeleteChannel={(channel) => setChannelDialog({ kind: 'edit', channel, startDelete: true })}
          onOpenServerSettings={() => setServerSettingsOpen(true)}
        />
      )}
      {server && server.ownerId === identity.id && !server.closed && (
        <ServerSettings
          server={server}
          open={serverSettingsOpen}
          onClose={() => setServerSettingsOpen(false)}
          onCreateChannel={() => setChannelDialog({ kind: 'create', serverId: server.id })}
          onEditChannel={(channel) => setChannelDialog({ kind: 'edit', channel })}
        />
      )}
      <ChannelDialog
        mode={channelDialog}
        onClose={() => setChannelDialog(null)}
        onCreated={(c) => setChannelByServer((m) => ({ ...m, [c.serverId]: c.id }))}
      />
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
        initialLink={invite}
        onClose={() => {
          setDialog(null);
          setInvite(undefined);
        }}
        onJoined={(id) => {
          setDialog(null);
          setInvite(undefined);
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
      <SettingsScreen
        open={settingsOpen}
        request={settingsRequest}
        onClose={() => setSettingsOpen(false)}
        onCheckWords={() => setDialog('check')}
      />

      {/* Toasts stack in one column, so two at once never overlap. */}
      <div className="toasts">
        {serverGone && (
          <div className="toast" role="status" key={serverGone.at}>
            <FormattedMessage id={`server.gone.${serverGone.reason}`} values={{ name: serverGone.name }} />
          </div>
        )}
        {flash && (
          <div className="toast" role="status" key={flash.at}>
            <FormattedMessage id={flash.id} />
          </div>
        )}
        {channelGone && (
          <div className="toast" role="status">
            <FormattedMessage id="channel.deletedNotice" values={{ channel: channelGone }} />
          </div>
        )}
        {notice && (
          <div className="toast" role="status">
            <ShieldIcon />
            <FormattedMessage id="notice.encryptionRenewed" />
          </div>
        )}

        {updateNote && (
          <div className="toast update-note" role="status">
            {updateNote.kind === 'installed' ? <RefreshIcon /> : <AlertIcon />}
            <div className="update-note-body">
              {updateNote.kind === 'installed' ? (
                <FormattedMessage id="update.installed" values={{ version: updateNote.version }} />
              ) : (
                <FormattedMessage id="update.rolledBack" values={{ failed: updateNote.failed, running: updateNote.running }} />
              )}
              {updateNote.kind === 'installed' && updateNote.notes && (
                // Plain text from the release manifest: rendered as text only.
                <details>
                  <summary>
                    <FormattedMessage id="update.whatsNew" />
                  </summary>
                  <p className="update-notes">
                    {paragraphs(updateNote.notes, uiDir).map((l, i) => (
                      <span key={i} className="msg-line user-text" dir={l.dir}>
                        {l.text}
                      </span>
                    ))}
                  </p>
                </details>
              )}
            </div>
            <button
              type="button"
              className="toast-close"
              aria-label={intl.formatMessage({ id: 'common.close' })}
              onClick={() => store.clearUpdateNote()}
            >
              <CloseIcon />
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
