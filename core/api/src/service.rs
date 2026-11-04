//! The core: `CoreCommands` from `ui/src/core/api.ts`, one method each.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use authority::Action;
use groupkeys::MemberSecret;
use identity::{PhraseError, RecoveryPhrase, Vault, VaultError, VaultState};
use rand_core::{OsRng, RngCore};
use records::{ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::chat;
use crate::db::Db;
use crate::model::{self, GenesisBody, Log, ServerView};
use crate::ports::{CipherError, GroupCipher, Network, Ports, SharedSecrets};
use crate::types::*;

const DEFAULT_STORAGE_LIMIT: u64 = 5 * 1024 * 1024 * 1024;
const KV_STORAGE_LIMIT: &str = "storage-limit";
const KV_UPDATE_CHANNEL: &str = "update-channel";
const KV_RECOVERY_CHALLENGE: &str = "recovery-challenge";

pub struct Config {
    /// `None` keeps everything in memory (tests).
    pub db_path: Option<PathBuf>,
    pub app_version: String,
}

/// What is stored about me, sealed in the local database.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    display_name: String,
    bio: Option<String>,
    avatar_data_url: Option<String>,
    country: String,
    birth_date: String,
    /// The UI language at sign-up; names what this identity creates
    /// (#general). Older profiles and restored identities have none: English.
    #[serde(default)]
    locale: Option<Locale>,
}

/// Kept in the store so a restart neither resets the count nor offers new
/// positions (B19 §6). Positions are 1-based. Not secret: they say which
/// words to ask for, not the words.
#[derive(Serialize, Deserialize)]
struct Challenge {
    positions: Vec<usize>,
    failures: u8,
    burned: bool,
}

pub const CHALLENGE_POSITIONS: usize = 3;
pub const MAX_CHECK_FAILURES: u8 = 5;
/// How often the background sync runs without a hint.
pub const SYNC_EVERY: Duration = Duration::from_secs(2);
/// Join re-keys are batched: at least this long apart per channel, plus
/// up to `JOIN_REKEY_JITTER_MS`: 15 s ± 20% (12-18 s). Kicks stay immediate.
pub const JOIN_REKEY_MIN_MS: u64 = 12_000;
const JOIN_REKEY_JITTER_MS: u64 = 6_001;

struct Me {
    /// Held only until the recovery check passes, so the UI can show and
    /// check the words; dropped (zeroized) after. TODO(#31): with the app
    /// lock, re-read from the vault after re-authentication instead.
    phrase: Option<RecoveryPhrase>,
    identity: identity::Identity,
    profile: Profile,
}

impl Me {
    fn key(&self) -> PublicKey {
        self.identity.public_key()
    }
}

struct Inner {
    db: Db,
    vault: Vault<SharedSecrets>,
    me: Option<Me>,
    /// Loaded on first use per server, then kept current record by record.
    logs: HashMap<ServerId, Log>,
    /// Messages already announced to the UI, per server, with what was
    /// announced (edit time, deleted), so sync emits only what is new.
    announced: HashMap<ServerId, HashMap<RecordId, Announced>>,
    /// For my messages' send state (B6).
    net: Arc<dyn Network>,
    /// Who each channel was last keyed for by our own rotation, so a
    /// moderator re-keys when members join (in memory: once after a restart).
    keyed_for: HashMap<(ServerId, ChannelId), BTreeSet<PublicKey>>,
    /// The earliest next join re-key per channel (batching, GAP-8).
    rekey_after: HashMap<(ServerId, ChannelId), u64>,
}

pub struct Core {
    inner: Mutex<Inner>,
    ports: Ports,
    app_version: String,
}

/// Events collected while the lock is held, emitted after it is released.
type Events = Vec<Event>;

fn vault_error(e: VaultError) -> CoreError {
    match e {
        VaultError::AlreadyExists => ErrorCode::NotAllowed.into(),
        VaultError::NotFound => ErrorCode::NotFound.into(),
        VaultError::RecoveryCheckRequired => ErrorCode::RecoveryCheckRequired.into(),
        // TODO(app lock): commands to unlock and set a passphrase.
        VaultError::Locked | VaultError::WrongPassphrase => ErrorCode::NotAllowed.into(),
        _ => ErrorCode::Internal.into(),
    }
}

fn phrase_error(e: PhraseError) -> CoreError {
    let positions = e.positions().into_iter().map(|p| p + 1).collect();
    CoreError { code: ErrorCode::InvalidWords, positions: Some(positions) }
}

fn parse_user(id: &str) -> CoreResult<PublicKey> {
    model::parse_hex32(id).map(PublicKey::from_bytes).ok_or_else(|| ErrorCode::NotFound.into())
}

fn parse_server(id: &str) -> CoreResult<ServerId> {
    model::parse_hex32(id).map(ServerId::from_bytes).ok_or_else(|| ErrorCode::NotFound.into())
}

impl Core {
    pub fn open(config: Config, ports: Ports) -> CoreResult<Core> {
        let db = Db::open(config.db_path.as_deref())?;
        let vault = Vault::new(SharedSecrets(ports.secrets.clone()));
        let mut inner = Inner {
            db,
            vault,
            me: None,
            logs: HashMap::new(),
            announced: HashMap::new(),
            net: ports.network.clone(),
            keyed_for: HashMap::new(),
            rekey_after: HashMap::new(),
        };
        if inner.vault.state().map_err(vault_error)? == VaultState::Ready {
            let phrase = inner.vault.open().map_err(vault_error)?;
            inner.me = Some(inner.load_me(phrase)?);
            unlock_cipher(&ports, inner.me.as_ref());
            // GAP-4: every server opened for sync before any command can
            // publish to it (the sync loop also reopens them each round).
            for server in inner.db.servers()? {
                inner.load(server)?;
                inner.open_sync(&ports, server)?;
            }
        }
        Ok(Core { inner: Mutex::new(inner), ports, app_version: config.app_version })
    }

    /// What the app lock and sign-out call: the group cipher drops our
    /// secret and every channel key. TODO(app lock): the commands.
    pub fn lock_keys(&self) {
        self.ports.cipher.lock();
        self.ports.network.lock();
    }

    /// After the app lock: the cipher gets our secret back. The next read or
    /// send replays what it lost (see `unlock_cipher`).
    pub fn unlock_keys(&self) {
        unlock_cipher(&self.ports, self.lock().me.as_ref());
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("a panic inside the core poisoned its lock")
    }

    fn emit(&self, events: Events) {
        for event in &events {
            self.ports.events.emit(event);
        }
    }

    /// Runs `f` under the lock, then emits what it collected.
    fn run<T>(
        &self,
        f: impl FnOnce(&mut Inner, &Ports, &mut Events) -> CoreResult<T>,
    ) -> CoreResult<T> {
        let mut events = Vec::new();
        let result = f(&mut self.lock(), &self.ports, &mut events);
        self.emit(events);
        result
    }

    // ---- identity -------------------------------------------------------

    pub fn get_identity(&self) -> CoreResult<Option<Identity>> {
        let inner = self.lock();
        inner.me.as_ref().map(|me| inner.identity_json(me)).transpose()
    }

    pub fn create_identity(&self, input: NewIdentityInput) -> CoreResult<CreatedIdentity> {
        self.run(|inner, ports, events| {
            if inner.me.is_some() {
                return Err(ErrorCode::NotAllowed.into());
            }
            let progress = |step, percent| Event::IdentityProgress { step, percent };
            events.push(progress(ProgressStep::Keys, 25));
            let phrase = inner.vault.create().map_err(vault_error)?;
            events.push(progress(ProgressStep::Storage, 55));
            let profile = Profile {
                country: input.country,
                birth_date: input.birth_date,
                locale: Some(input.locale),
                ..Profile::default()
            };
            let me = inner.new_me(phrase, profile)?;
            events.push(progress(ProgressStep::Encryption, 85));
            let phrase = me.phrase.as_ref().ok_or(ErrorCode::Internal)?;
            let words = phrase.words().iter().map(|w| (*w).to_owned()).collect();
            let identity = inner.identity_json(&me)?;
            inner.me = Some(me);
            unlock_cipher(ports, inner.me.as_ref());
            events.push(progress(ProgressStep::Done, 100));
            Ok(CreatedIdentity { identity, words })
        })
    }

    /// The current word check (B19 §6): [`CHALLENGE_POSITIONS`] 1-based
    /// positions, the same until passed or burned. `too-many-attempts`
    /// while burned; `not-allowed` once the check has passed.
    pub fn get_recovery_challenge(&self) -> CoreResult<RecoveryChallenge> {
        let inner = self.lock();
        let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
        // Already passed: the phrase is gone (see `Me::phrase`).
        me.phrase.as_ref().ok_or(ErrorCode::NotAllowed)?;
        let challenge = match inner.challenge()? {
            Some(c) if c.burned => return Err(ErrorCode::TooManyAttempts.into()),
            Some(c) => c,
            None => {
                let positions = identity::check_positions(CHALLENGE_POSITIONS)
                    .into_iter()
                    .map(|p| p + 1)
                    .collect();
                let fresh = Challenge { positions, failures: 0, burned: false };
                inner.save_challenge(&fresh)?;
                fresh
            }
        };
        Ok(RecoveryChallenge { positions: challenge.positions })
    }

    /// Answers for exactly the current challenge's positions (1-based),
    /// else `not-allowed`. The [`MAX_CHECK_FAILURES`]th failure burns the
    /// challenge and itself rejects with `too-many-attempts`.
    pub fn check_recovery_words(
        &self,
        answers: BTreeMap<usize, String>,
    ) -> CoreResult<RecoveryCheckResult> {
        self.run(|inner, _, events| {
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            let phrase = me.phrase.as_ref().ok_or(ErrorCode::NotAllowed)?;
            let mut challenge = inner.challenge()?.ok_or(ErrorCode::NotAllowed)?;
            if challenge.burned {
                return Err(ErrorCode::TooManyAttempts.into());
            }
            if !answers.keys().copied().eq(challenge.positions.iter().copied()) {
                return Err(ErrorCode::NotAllowed.into());
            }
            let wrong: Vec<usize> = answers
                .iter()
                .filter(|(p, w)| !phrase.check_words(&[(*p - 1, w.as_str())]))
                .map(|(p, _)| *p)
                .collect();
            if wrong.is_empty() {
                let answers: Vec<(usize, &str)> =
                    answers.iter().map(|(p, w)| (p - 1, w.as_str())).collect();
                if !inner.vault.confirm_recovery_check(phrase, &answers).map_err(vault_error)? {
                    return Err(ErrorCode::Internal.into());
                }
                inner.db.delete(KV_RECOVERY_CHALLENGE)?;
                inner.me.as_mut().expect("checked above").phrase = None;
                let me = inner.me.as_ref().expect("checked above");
                events.push(Event::IdentityChanged(inner.identity_json(me)?));
                return Ok(RecoveryCheckResult::passed());
            }
            challenge.failures += 1;
            challenge.burned = challenge.failures >= MAX_CHECK_FAILURES;
            inner.save_challenge(&challenge)?;
            if challenge.burned {
                return Err(ErrorCode::TooManyAttempts.into());
            }
            Ok(RecoveryCheckResult::wrong(wrong))
        })
    }

    /// Only before the recovery check passes: one XSS in the UI must not be
    /// able to read the words any time later. Success lifts a burned
    /// challenge (the next one has new positions and a fresh count).
    /// TODO(app lock, #31): OS authentication first (`auth-cancelled`),
    /// and the words again after the check, behind it.
    pub fn get_recovery_words(&self) -> CoreResult<Vec<String>> {
        let inner = self.lock();
        let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let phrase = me.phrase.as_ref().ok_or(ErrorCode::NotAllowed)?;
        if inner.challenge()?.is_some_and(|c| c.burned) {
            inner.db.delete(KV_RECOVERY_CHALLENGE)?;
        }
        Ok(phrase.words().iter().map(|w| (*w).to_owned()).collect())
    }

    pub fn restore_identity(&self, words: Vec<String>) -> CoreResult<Identity> {
        self.run(|inner, ports, _| {
            if inner.me.is_some() {
                return Err(ErrorCode::NotAllowed.into());
            }
            let phrase = RecoveryPhrase::parse(&words.join(" ")).map_err(phrase_error)?;
            inner.vault.restore(&phrase).map_err(vault_error)?;
            inner.db.delete(KV_RECOVERY_CHALLENGE)?;
            // TODO(backup): the spec restores the birth date from the
            // encrypted identity backup; until then the UI asks again.
            let me = inner.new_me(phrase, Profile::default())?;
            let identity = inner.identity_json(&me)?;
            inner.me = Some(me);
            unlock_cipher(ports, inner.me.as_ref());
            Ok(identity)
        })
    }

    pub fn update_profile(&self, input: ProfileInput) -> CoreResult<Identity> {
        self.run(|inner, _, events| {
            model::check_profile(&input)?;
            let me = inner.me.as_mut().ok_or(ErrorCode::NotFound)?;
            me.profile.display_name = input.display_name.trim().to_owned();
            me.profile.bio = input.bio;
            me.profile.avatar_data_url = input.avatar_data_url;
            // TODO(profiles): publish the new name to my servers; members
            // see the name from my profile post until then.
            let profile = me.profile.clone();
            let key = *me.identity.storage_key().expose_secret();
            inner.db.save_profile(&key, &serde_json::to_vec(&profile).expect("plain struct"))?;
            let identity = inner.identity_json(inner.me.as_ref().expect("checked above"))?;
            events.push(Event::IdentityChanged(identity.clone()));
            Ok(identity)
        })
    }

    // ---- servers ----------------------------------------------------------

    pub fn list_servers(&self) -> CoreResult<Vec<ServerSummary>> {
        let mut inner = self.lock();
        let mut out = Vec::new();
        for id in inner.db.servers()? {
            inner.load(id)?;
            out.push(inner.summary(id)?);
        }
        Ok(out)
    }

    pub fn list_channels(&self, server_id: &str) -> CoreResult<Vec<Channel>> {
        let mut inner = self.lock();
        let server = inner.load_mine(parse_server(server_id)?)?;
        let me = inner.me.as_ref().map(Me::key);
        inner.view(server).map(|view| view.channels(me))
    }

    pub fn list_members(&self, server_id: &str) -> CoreResult<Vec<Member>> {
        let mut inner = self.lock();
        let server = inner.load_mine(parse_server(server_id)?)?;
        inner.members_json(&inner.view(server)?)
    }

    pub fn create_server(
        &self,
        name: &str,
        age_restriction: AgeRestriction,
    ) -> CoreResult<ServerSummary> {
        self.run(|inner, ports, events| {
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            inner.vault.require_recovery_check(&me.identity).map_err(vault_error)?;
            let name = name.trim();
            if name.is_empty()
                || name.chars().count() > model::MAX_NAME_CHARS
                || !authority::display_text_ok(name)
            {
                return Err(ErrorCode::BadRequest.into());
            }
            let mut nonce = [0u8; records::GENESIS_NONCE_LEN];
            OsRng.fill_bytes(&mut nonce);
            let body = GenesisBody { v: 1, name: name.to_owned(), age_restriction };
            // The genesis and #general (UX-04). #general keeps the derived ID,
            // so peers and sync agree on its log without reading the record.
            let server = ServerId::derive(&me.key(), &nonce);
            let [genesis, general] = authority::sign_new_server(
                me.identity.signing_key(),
                nonce,
                ports.clock.now_ms(),
                &serde_json::to_vec(&body).expect("plain struct"),
                model::default_channel(server),
                model::general_name(me.profile.locale),
            )
            .map_err(|_| ErrorCode::BadRequest)?;
            debug_assert_eq!(genesis.server(), server);
            // The owner is a member from the genesis: the first invite (for
            // the link) and the profile post follow.

            let sign = |seq, deps, action: &Action| {
                authority::sign_control(
                    me.identity.signing_key(),
                    server,
                    seq,
                    deps,
                    ports.clock.now_ms(),
                    action,
                )
                .map_err(|_| CoreError::from(ErrorCode::BadRequest))
            };
            let invite = sign(2, vec![general.id()], &model::FIRST_INVITE)?;
            let profile =
                sign(3, vec![invite.id()], &model::profile_action(&me.profile.display_name))?;
            inner.logs.insert(server, Log::new(server, []));
            // GAP-4: a fresh sync secret, sealed, and sync opened with it
            // before anything is published.
            let mut secret = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(secret.as_mut());
            inner.save_secret(server, &secret)?;
            ports.network.open_server(server, &secret, &inner.known(server)?);
            for record in [genesis, general, invite, profile] {
                ports.network.publish(&record);
                inner.store(server, record)?;
            }
            let claim = inner.sign_age_claim(ports, server)?;
            ports.network.publish(&claim);
            inner.store(server, claim)?;
            inner.db.add_server(server, ports.clock.now_ms())?;
            let summary = inner.summary(server)?;
            events.push(Event::ServerAdded(summary.clone()));
            Ok(summary)
        })
    }

    /// Resolves with the first status; the outcome follows as `join.status`.
    pub fn join_server(&self, link: &str) -> CoreResult<JoinStatus> {
        let link = link.trim().to_owned();
        let Some((server, invite, secret)) = model::parse_invite_link(&link) else {
            return Ok(JoinStatus::Invalid { link });
        };
        let searching = JoinStatus::Searching { link: link.clone() };
        self.ports.events.emit(&Event::JoinStatus(searching.clone()));
        // GAP-4: only the secret lets us ask for the server's logs.
        let general = BTreeSet::from([model::default_channel(server)]);
        self.ports.network.open_server(server, &secret, &general);
        // Fetch without the lock, so UI commands don't wait on the network.
        let fetched: Vec<Record> =
            self.ports.network.fetch(server).into_iter().filter(|r| r.server() == server).collect();
        self.run(|inner, ports, events| {
            let status = inner.join(ports, server, invite, &secret, &link, fetched, events)?;
            events.push(Event::JoinStatus(status));
            Ok(searching)
        })
    }

    /// Nothing to cancel: joining is synchronous in-process.
    /// TODO(sync): cancel the peer search.
    pub fn cancel_join(&self, _link: &str) -> CoreResult<()> {
        Ok(())
    }

    /// Rust addition (`grantRole`).
    pub fn grant_role(&self, server_id: &str, user_id: &str, role: Role) -> CoreResult<()> {
        let target = parse_user(user_id)?;
        let action = Action::Grant { target, role: role.to_authority() };
        self.control_action(server_id, action)
    }

    /// Rust addition (`revokeRole`): back to plain member.
    pub fn revoke_role(&self, server_id: &str, user_id: &str) -> CoreResult<()> {
        self.control_action(server_id, Action::revoke(parse_user(user_id)?))
    }

    /// Allowed for moderators and up, against anyone ranked lower
    /// (`authority`'s rule; the mock only lets the owner kick).
    pub fn kick_member(&self, server_id: &str, user_id: &str) -> CoreResult<()> {
        self.control_action(server_id, Action::kick(parse_user(user_id)?))
    }

    /// B11: the owner creates a channel. `name-taken` if another channel of
    /// the server has the name; `bad-request` for a name authority refuses.
    pub fn create_channel(&self, server_id: &str, name: &str, adult: bool) -> CoreResult<Channel> {
        let name = model::channel_name(name).ok_or(ErrorCode::BadRequest)?;
        let server = parse_server(server_id)?;
        self.check_name_free(server, &name, None)?;
        self.check_may_mark_adult(server, adult)?;
        let mut id = [0u8; 32];
        OsRng.fill_bytes(&mut id);
        let channel = ChannelId::from_bytes(id);
        self.control_action(server_id, Action::ChannelCreate { channel, adult, name })?;
        let json = self.channel_json(server, channel)?;
        self.ports.events.emit(&Event::ChannelAdded(json.clone()));
        Ok(json)
    }

    /// B11: the owner renames a channel or changes its 18+ flag.
    pub fn update_channel(
        &self,
        channel_id: &str,
        name: Option<&str>,
        adult: Option<bool>,
    ) -> CoreResult<Channel> {
        let (server, channel, current) = self.live_channel(channel_id)?;
        let name = match name {
            Some(raw) => model::channel_name(raw).ok_or(ErrorCode::BadRequest)?,
            None => current.name.clone(),
        };
        self.check_name_free(server, &name, Some(channel))?;
        let adult = adult.unwrap_or(current.adult);
        self.check_may_mark_adult(server, adult)?;
        let server_id = model::hex_id(server.as_bytes());
        self.control_action(&server_id, Action::ChannelEdit { channel, adult, name })?;
        let json = self.channel_json(server, channel)?;
        self.ports.events.emit(&Event::ChannelUpdated(json.clone()));
        Ok(json)
    }

    /// B11: the owner deletes a channel; never #general (`not-allowed`).
    /// TODO(sync): close its log in sync; until then it is only no longer
    /// opened (the next `open_server` leaves it out).
    pub fn delete_channel(&self, channel_id: &str) -> CoreResult<()> {
        let (server, channel, current) = self.live_channel(channel_id)?;
        if current.general {
            return Err(ErrorCode::NotAllowed.into());
        }
        let server_id = model::hex_id(server.as_bytes());
        self.control_action(&server_id, Action::ChannelDelete { channel })?;
        self.ports.events.emit(&Event::ChannelRemoved {
            server_id,
            channel_id: model::hex_id(channel.as_bytes()),
            name: current.name,
        });
        Ok(())
    }

    fn live_channel(
        &self,
        channel_id: &str,
    ) -> CoreResult<(ServerId, ChannelId, authority::Channel)> {
        let mut inner = self.lock();
        let (server, channel) = inner.find_channel(channel_id)?;
        let view = inner.view(server)?;
        let current = view.state.channels.get(&channel).cloned().ok_or(ErrorCode::NotFound)?;
        Ok((server, channel, current))
    }

    /// An 18+ channel only from someone whose own age claim says 18+: else
    /// they couldn't see the channel they made (B7). `not-allowed` before
    /// anything is signed or sent.
    fn check_may_mark_adult(&self, server: ServerId, adult: bool) -> CoreResult<()> {
        if !adult {
            return Ok(());
        }
        let mut inner = self.lock();
        let server = inner.load_mine(server)?;
        let me = inner.me.as_ref().map(Me::key).ok_or(ErrorCode::NotFound)?;
        if !inner.view(server)?.state.adults.contains(&me) {
            return Err(ErrorCode::NotAllowed.into());
        }
        Ok(())
    }

    fn check_name_free(
        &self,
        server: ServerId,
        name: &str,
        except: Option<ChannelId>,
    ) -> CoreResult<()> {
        let mut inner = self.lock();
        let server = inner.load_mine(server)?;
        let view = inner.view(server)?;
        let taken = view.state.channels.iter().any(|(id, c)| Some(*id) != except && c.name == name);
        if taken {
            return Err(ErrorCode::NameTaken.into());
        }
        Ok(())
    }

    fn channel_json(&self, server: ServerId, channel: ChannelId) -> CoreResult<Channel> {
        let inner = self.lock();
        inner.open_sync(&self.ports, server)?;
        let me = inner.me.as_ref().map(Me::key);
        let id = model::hex_id(channel.as_bytes());
        let view = inner.view(server)?;
        view.channels(me).into_iter().find(|c| c.id == id).ok_or_else(|| ErrorCode::Internal.into())
    }

    /// Rust addition (`undoKick`, B4 kick-undo): the owner lets a kicked
    /// account back. It isn't made a member: it may join again through an
    /// invite. `not-allowed` for anyone but the owner, or someone not kicked.
    pub fn undo_kick(&self, server_id: &str, user_id: &str) -> CoreResult<()> {
        self.control_action(server_id, Action::Unban { target: parse_user(user_id)? })
    }

    /// Rust addition (`listRemovedMembers`): the owner's list of removed
    /// accounts, with when each was kicked. `not-allowed` for anyone else.
    pub fn list_removed_members(&self, server_id: &str) -> CoreResult<Vec<RemovedMember>> {
        let mut inner = self.lock();
        let server = inner.load_mine(parse_server(server_id)?)?;
        let me = inner.me.as_ref().map(Me::key);
        let view = inner.view(server)?;
        if me != Some(view.owner) {
            return Err(ErrorCode::NotAllowed.into());
        }
        let names = view.display_names();
        let kicked_at: HashMap<RecordId, u64> =
            view.records.iter().map(|r| (r.id(), r.ts())).collect();
        Ok(view
            .state
            .removed
            .iter()
            .filter(|(key, _)| view.state.kicked.contains(key))
            .map(|(key, kick)| RemovedMember {
                id: model::hex_id(key.as_bytes()),
                short_code: model::short_code(key),
                display_name: names.get(key).cloned().unwrap_or_default(),
                removed_at: kicked_at.get(kick).copied().unwrap_or_default(),
            })
            .collect())
    }

    // TODO(records B4):
    // - Kick, Leave and Revoke go out with an empty cut today (`Action::kick`,
    //   `Action::revoke`). B4-4: fill it with up to 64 channel heads, syncing
    //   them first, starting with the channels where the target has records.
    // - When an invite expires, send `RevokeInvite` automatically if we are
    //   its inviter or a moderator and up.
    fn control_action(&self, server_id: &str, action: Action) -> CoreResult<()> {
        let server = parse_server(server_id)?;
        self.run(|inner, ports, events| {
            inner.load_mine(server)?;
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            if matches!(&action, Action::Kick { target, .. } if *target == me.key()) {
                return Err(ErrorCode::NotAllowed.into());
            }
            let view = inner.view(server)?;
            let record = authority::sign_control(
                me.identity.signing_key(),
                server,
                view.next_seq(&me.key()),
                view.deps(None),
                ports.clock.now_ms(),
                &action,
            )
            .map_err(|_| ErrorCode::BadRequest)?;
            // Dry run on the live resolver. Its deps are our heads, so it
            // joins at once; if `authority` doesn't accept it, it was never
            // stored or sent, and the log is rebuilt from the store so it
            // leaves no trace.
            // TODO(records B4): `Resolver::would_accept` instead.
            let log = inner.logs.get_mut(&server).expect("loaded above");
            let joined = log.add(record.clone());
            if !log.accepted(&record.id()) {
                inner.logs.remove(&server);
                return Err(ErrorCode::NotAllowed.into());
            }
            for record in &joined {
                inner.db.put_record(record)?;
            }
            ports.network.publish(&record);
            let after = inner.view(server)?;
            events.push(Event::MembersChanged {
                server_id: server_id.to_owned(),
                members: inner.members_json(&after)?,
            });
            // M2: removed members must not read what comes next.
            if matches!(action, Action::Kick { .. } | Action::Revoke { .. }) {
                // The kick is stored and sent: a failed re-key must not undo that.
                if inner.rekey(ports, server, events).is_err() {
                    events.push(Event::Notice {
                        kind: NoticeKind::RekeyWaiting,
                        server_id: server_id.to_owned(),
                    });
                }
            }
            Ok(())
        })
    }

    // ---- messages ---------------------------------------------------------
    //
    // TODO(chat): replies, edit, delete, retry; channels other than the
    // default one; auth_deps that always include our latest control heads
    // (today: up to MAX_AUTH_DEPS of the current heads).

    /// Seals `body` for the channel's members and sends it. The first
    /// message of a channel, or the first after a removal, needs a key
    /// rotation: a moderator or above makes one on the way; anyone else
    /// gets `not-allowed` until one arrives.
    pub fn send_message(
        &self,
        channel_id: &str,
        body: &str,
        reply_to: Option<&str>,
    ) -> CoreResult<Message> {
        if reply_to.is_some() {
            return Err(ErrorCode::NotImplemented.into());
        }
        if body.trim().is_empty() || !authority::message_text_ok(body) {
            return Err(ErrorCode::BadRequest.into());
        }
        self.run(|inner, ports, events| {
            let (server, channel) = inner.find_channel(channel_id)?;
            let sealed = inner.seal(ports, server, channel, body)?;
            let record =
                inner.sign_in_channel(ports, server, channel, None, Kind::Message, sealed)?;
            inner.store(server, record.clone())?;
            ports.network.publish(&record);
            let message = inner.message(&*ports.cipher, server, channel, record.id())?;
            events.push(Event::MessageAdded(message.clone()));
            Ok(message)
        })
    }

    /// The author only (authority's rule). The new text is sealed like a
    /// message; the edit names its target in its body and deps.
    pub fn edit_message(&self, message_id: &str, body: &str) -> CoreResult<()> {
        if body.trim().is_empty() || !authority::message_text_ok(body) {
            return Err(ErrorCode::BadRequest.into());
        }
        self.run(|inner, ports, events| {
            let (server, channel, target, _) = inner.find_message(message_id)?;
            let sealed = inner.seal(ports, server, channel, body)?;
            let body = [target.as_bytes().as_slice(), &sealed].concat();
            inner.tombstone_or_edit(ports, events, server, channel, target, Kind::MessageEdit, body)
        })
    }

    /// The author, or a moderator or above who outranks them. The message
    /// keeps its place as a tombstone (`deleted: true`, no text).
    pub fn delete_message(&self, message_id: &str) -> CoreResult<()> {
        self.run(|inner, ports, events| {
            let (server, channel, target, _) = inner.find_message(message_id)?;
            let body = target.as_bytes().to_vec();
            inner.tombstone_or_edit(
                ports,
                events,
                server,
                channel,
                target,
                Kind::MessageDelete,
                body,
            )
        })
    }

    /// Sends one of my messages again: the same stored record, never
    /// re-signed (another record would be another message).
    pub fn retry_message(&self, message_id: &str) -> CoreResult<()> {
        let mut inner = self.lock();
        // A pruned message (B6-p) has no record left to send.
        let (_, _, _, record) = inner.find_message(message_id)?;
        let record = record.ok_or(ErrorCode::NotAllowed)?;
        let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        if record.author() != me {
            return Err(ErrorCode::NotAllowed.into());
        }
        drop(inner);
        self.ports.network.publish(&record);
        Ok(())
    }
    pub fn get_message_count(&self, channel_id: &str) -> CoreResult<usize> {
        let mut inner = self.lock();
        let (server, channel) = inner.find_channel(channel_id)?;
        inner.message_count(&*self.ports.cipher, server, channel)
    }

    /// Messages `start..end` (0-based, end exclusive) of the channel.
    pub fn get_messages(
        &self,
        channel_id: &str,
        start: usize,
        end: usize,
    ) -> CoreResult<MessageRange> {
        let mut inner = self.lock();
        let (server, channel) = inner.find_channel(channel_id)?;
        let me = inner.me.as_ref().map(Me::key);
        let shown = inner.shown(&*self.ports.cipher, server, channel)?;
        let end = end.min(shown.len());
        let start = start.min(end);
        let messages = shown[start..end]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                message_json(s, start + i, channel, me, &Status { net: &*inner.net, db: &inner.db })
            })
            .collect();
        Ok(MessageRange { channel_id: channel_id.to_owned(), start, messages })
    }

    /// The background sync the app runs (`mvs-core` starts it when a
    /// network is configured): what `syncNow` does, every [`SYNC_EVERY`]
    /// and at once on each hint (sync stored something), until `stop` is
    /// set. A failed round is logged and retried on the next one. Holds the
    /// core lock only to store, never while fetching.
    pub fn run_sync_loop(&self, hints: &Receiver<()>, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            if let Err(e) = self.sync_now() {
                eprintln!("mvs-core: sync round failed: {:?}", e.code);
            }
            match hints.recv_timeout(SYNC_EVERY) {
                // Several hints at once are one round.
                Ok(()) => while hints.try_recv().is_ok() {},
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(SYNC_EVERY),
            }
        }
    }

    /// Rust addition (`syncNow`): pulls every server's records from the
    /// network and reports what changed. TODO(sync): driven by the sync
    /// crate's push and pull instead of a command.
    pub fn sync_now(&self) -> CoreResult<()> {
        let servers = {
            let mut inner = self.lock();
            if inner.me.is_none() {
                return Ok(());
            }
            let servers = inner.db.servers()?;
            // GAP-4: (re)open each server with its secret and channels first.
            for server in &servers {
                inner.load(*server)?;
                inner.open_sync(&self.ports, *server)?;
            }
            servers
        };
        // Fetch without the lock, so UI commands don't wait on the network.
        let fetched: Vec<(ServerId, Vec<Record>)> =
            servers.into_iter().map(|s| (s, self.ports.network.fetch(s))).collect();
        self.run(|inner, ports, events| {
            let Some(me) = inner.me.as_ref() else { return Ok(()) };
            let me = me.key();
            for (server, records) in fetched {
                if !inner.db.has_server(server)? {
                    continue;
                }
                inner.load(server)?;
                inner.seed_announced(&*ports.cipher, server)?;
                let mut changed = false;
                for record in records {
                    if record.server() == server {
                        changed |= inner.store(server, record)?;
                    }
                }
                let after = inner.view(server)?;
                if after.is_kicked(&me) {
                    inner.db.remove_server(server)?;
                    events
                        .push(Event::ServerRemoved { server_id: model::hex_id(server.as_bytes()) });
                } else if changed {
                    events.push(Event::MembersChanged {
                        server_id: model::hex_id(server.as_bytes()),
                        members: inner.members_json(&after)?,
                    });
                    // M2: a kick or revoke we read, that nobody re-keyed for.
                    // Never aborts the other servers; the next sync retries.
                    if inner.rekey(ports, server, events).is_err() {
                        events.push(Event::Notice {
                            kind: NoticeKind::RekeyWaiting,
                            server_id: model::hex_id(server.as_bytes()),
                        });
                    }
                    inner.announce(&*ports.cipher, server, events)?;
                    inner.prune(ports, server)?;
                } else if inner.may_rotate(server)? {
                    // A join re-key a batch window deferred: retried each round.
                    let _ = inner.rekey(ports, server, events);
                }
            }
            Ok(())
        })
    }

    pub fn block_user(&self, user_id: &str) -> CoreResult<()> {
        let user = parse_user(user_id)?;
        self.run(|inner, _, events| {
            inner.db.toggle_block(&user)?;
            for server in inner.db.servers()? {
                inner.load(server)?;
                let view = inner.view(server)?;
                if view.members().iter().any(|m| m.key == user) {
                    events.push(Event::MembersChanged {
                        server_id: model::hex_id(server.as_bytes()),
                        members: inner.members_json(&view)?,
                    });
                }
            }
            Ok(())
        })
    }

    // ---- app --------------------------------------------------------------

    pub fn get_storage_usage(&self) -> CoreResult<StorageUsage> {
        let inner = self.lock();
        let used = inner.db.size_bytes()?;
        let limit = inner
            .db
            .get(KV_STORAGE_LIMIT)?
            .and_then(|v| v.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(DEFAULT_STORAGE_LIMIT);
        Ok(StorageUsage {
            used_bytes: used,
            limit_bytes: limit,
            messages_bytes: used,
            files_bytes: 0,
        })
    }

    pub fn set_storage_limit(&self, bytes: u64) -> CoreResult<()> {
        self.lock().db.set(KV_STORAGE_LIMIT, &bytes.to_le_bytes())
    }

    pub fn clear_cache(&self) -> CoreResult<()> {
        Ok(())
    }

    /// For bug reports: counts and versions only, never words, keys, names
    /// or the birth date.
    pub fn export_diagnostics(&self) -> CoreResult<String> {
        let info = self.get_app_info()?;
        let inner = self.lock();
        let report = serde_json::json!({
            "app": info,
            "exportedAtMs": self.ports.clock.now_ms(),
            "hasIdentity": inner.me.is_some(),
            "servers": inner.db.servers()?.len(),
            "storageBytes": inner.db.size_bytes()?,
        });
        Ok(serde_json::to_string_pretty(&report).expect("json value"))
    }

    pub fn get_app_info(&self) -> CoreResult<AppInfo> {
        let channel = match self.lock().db.get(KV_UPDATE_CHANNEL)?.as_deref() {
            Some(b"beta") => UpdateChannel::Beta,
            _ => UpdateChannel::Stable,
        };
        Ok(AppInfo { version: self.app_version.clone(), update_channel: channel })
    }

    pub fn set_update_channel(&self, channel: UpdateChannel) -> CoreResult<()> {
        let value: &[u8] = match channel {
            UpdateChannel::Stable => b"stable",
            UpdateChannel::Beta => b"beta",
        };
        self.lock().db.set(KV_UPDATE_CHANNEL, value)
    }
}

impl Inner {
    /// Whether we are a moderator or above in a loaded server.
    fn may_rotate(&self, server: ServerId) -> CoreResult<bool> {
        let Some(me) = self.me.as_ref().map(Me::key) else { return Ok(false) };
        Ok(self.view(server)?.state.role(me) >= authority::Role::Moderator)
    }

    /// The known channels this identity may read and write: 18+ channels
    /// only with an accepted 18+ claim (B7).
    fn visible(&self, server: ServerId) -> CoreResult<BTreeSet<ChannelId>> {
        let me = self.me.as_ref().map(Me::key);
        let state = self.logs.get(&server).ok_or(ErrorCode::Internal)?.resolver.state();
        let adult = me.is_some_and(|me| state.adults.contains(&me));
        let mut visible = self.known(server)?;
        visible.retain(|c| adult || !state.channels.get(c).is_some_and(|c| c.adult));
        Ok(visible)
    }

    /// Our age claim for `server` (B7): 18+ or not, from the sealed birth
    /// date, which never leaves this device. A restored identity has no
    /// birth date yet and claims "not 18+".
    fn sign_age_claim(&self, ports: &Ports, server: ServerId) -> CoreResult<Record> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let now = ports.clock.now_ms();
        let adult = model::age_on(&me.profile.birth_date, now).is_some_and(|age| age >= 18);
        let view = self.view(server)?;
        authority::sign_control(
            me.identity.signing_key(),
            server,
            view.next_seq(&me.key()),
            view.deps(None),
            now,
            &Action::AgeClaim { adult },
        )
        .map_err(|_| ErrorCode::BadRequest.into())
    }

    /// B6-p: every full record an accepted delete or edit replaced is pruned
    /// to its stub, in memory, in the store and in sync's store. Nothing of
    /// its body (the ciphertext) is kept; nothing of its text was ever
    /// stored.
    fn prune(&mut self, ports: &Ports, server: ServerId) -> CoreResult<()> {
        for channel in self.known(server)? {
            let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
            for id in chat::prunable(log, channel) {
                let log = self.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
                if let Some(stub) = log.prune(id) {
                    self.db.prune_record(&stub)?;
                    ports.network.prune(id);
                }
            }
        }
        Ok(())
    }

    /// This server's sync secret, from the sealed store.
    fn secret(&self, server: ServerId) -> CoreResult<Option<Zeroizing<[u8; 32]>>> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let key = *me.identity.storage_key().expose_secret();
        self.db.load_server_secret(&key, server)
    }

    fn save_secret(&self, server: ServerId, secret: &[u8; 32]) -> CoreResult<()> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let key = *me.identity.storage_key().expose_secret();
        self.db.save_server_secret(&key, server, secret)
    }

    fn summary(&self, server: ServerId) -> CoreResult<ServerSummary> {
        let me = self.me.as_ref().map(Me::key);
        let secret = self.secret(server)?;
        Ok(self.view(server)?.summary(me, secret.as_deref()))
    }

    /// Syncs a loaded server members-only (GAP-4) with its secret and the
    /// channels authority knows. Harmless to repeat; a server without a
    /// stored secret (from before GAP-4) isn't synced.
    fn open_sync(&self, ports: &Ports, server: ServerId) -> CoreResult<()> {
        if let Some(secret) = self.secret(server)? {
            ports.network.open_server(server, &secret, &self.known(server)?);
        }
        Ok(())
    }

    /// The channels authority knows for a loaded server.
    fn known(&self, server: ServerId) -> CoreResult<BTreeSet<ChannelId>> {
        let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
        Ok(chat::known_channels(server, log.resolver.state()))
    }

    /// The first time a server is synced: what is already there counts as
    /// announced (the UI reads history with `getMessages`).
    fn seed_announced(&mut self, cipher: &dyn GroupCipher, server: ServerId) -> CoreResult<()> {
        if self.announced.contains_key(&server) {
            return Ok(());
        }
        let me = self.me.as_ref().map(Me::key);
        let mut seen = HashMap::new();
        for channel in self.known(server)? {
            for s in self.shown(cipher, server, channel)? {
                seen.insert(
                    s.record.id(),
                    announced(&s, me, &Status { net: &*self.net, db: &self.db }),
                );
            }
        }
        self.announced.insert(server, seen);
        Ok(())
    }

    /// `message.added` for messages not announced yet, `message.updated` for
    /// those whose edit or deletion changed since.
    fn announce(
        &mut self,
        cipher: &dyn GroupCipher,
        server: ServerId,
        events: &mut Events,
    ) -> CoreResult<()> {
        let me = self.me.as_ref().map(Me::key);
        for channel in self.known(server)? {
            let shown = self.shown(cipher, server, channel)?;
            let seen = self.announced.entry(server).or_default();
            for (index, s) in shown.iter().enumerate() {
                let state = announced(s, me, &Status { net: &*self.net, db: &self.db });
                let event = match seen.insert(s.record.id(), state) {
                    None => Event::MessageAdded(message_json(
                        s,
                        index,
                        channel,
                        me,
                        &Status { net: &*self.net, db: &self.db },
                    )),
                    Some(old) if old != state => Event::MessageUpdated(message_json(
                        s,
                        index,
                        channel,
                        me,
                        &Status { net: &*self.net, db: &self.db },
                    )),
                    Some(_) => continue,
                };
                events.push(event);
            }
        }
        Ok(())
    }

    /// Seals `text` for `channel`'s members. If no key is usable yet, a
    /// moderator or above rotates first; anyone else gets `not-allowed`.
    fn seal(
        &mut self,
        ports: &Ports,
        server: ServerId,
        channel: ChannelId,
        text: &str,
    ) -> CoreResult<Vec<u8>> {
        let cipher = &*ports.cipher;
        let key = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        let view = self.view(server)?;
        let members = view.state.members.clone();
        let may_rotate = view.state.role(key) >= authority::Role::Moderator;
        let auth_deps = auth_deps(&view);
        cipher.membership_changed(server, &members);
        chat::sync_cipher(cipher, server, self.logs.get(&server).ok_or(ErrorCode::Internal)?);
        match cipher.encrypt(server, channel, text.as_bytes()) {
            Err(CipherError::NoKey | CipherError::RotationRequired) if may_rotate => {
                // Fail closed: no rotation, no message (`not-allowed`).
                self.rotate_channel(ports, server, channel, &members, &auth_deps)
                    .map_err(|_| CoreError::from(ErrorCode::NotAllowed))?;
                cipher.encrypt(server, channel, text.as_bytes()).map_err(cipher_error)
            }
            other => other.map_err(cipher_error),
        }
    }

    /// A new record of mine in `channel`, on its heads (plus `target` for an
    /// edit or delete) and our latest control heads.
    fn sign_in_channel(
        &self,
        ports: &Ports,
        server: ServerId,
        channel: ChannelId,
        target: Option<RecordId>,
        kind: Kind,
        body: Vec<u8>,
    ) -> CoreResult<Record> {
        let key = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
        let seq = chat::channel_seq(log, channel, &key);
        let deps = match target {
            Some(target) => chat::deps_with(log, channel, target),
            None => chat::channel_deps(log, channel),
        };
        let auth_deps = auth_deps(&self.view(server)?);
        self.sign_channel(server, channel, seq, deps, auth_deps, ports.clock.now_ms(), kind, body)
    }

    /// Signs an edit or delete of `target`; `not-allowed` unless authority
    /// would accept it (dry run), so a refused one is never stored or sent.
    #[allow(clippy::too_many_arguments)]
    fn tombstone_or_edit(
        &mut self,
        ports: &Ports,
        events: &mut Events,
        server: ServerId,
        channel: ChannelId,
        target: RecordId,
        kind: Kind,
        body: Vec<u8>,
    ) -> CoreResult<()> {
        let record = self.sign_in_channel(ports, server, channel, Some(target), kind, body)?;
        let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
        if !chat::would_accept(log, &record) {
            return Err(ErrorCode::NotAllowed.into());
        }
        self.store(server, record.clone())?;
        ports.network.publish(&record);
        let message = self.message(&*ports.cipher, server, channel, target)?;
        self.prune(ports, server)?;
        events.push(Event::MessageUpdated(message));
        Ok(())
    }

    /// The `Message` record `message_id` in one of my servers' known
    /// channels; `not-found` otherwise.
    /// Also a message pruned to its stub (B6-p): then no full record.
    fn find_message(
        &mut self,
        message_id: &str,
    ) -> CoreResult<(ServerId, ChannelId, RecordId, Option<Record>)> {
        let id =
            model::parse_hex32(message_id).map(RecordId::from_bytes).ok_or(ErrorCode::NotFound)?;
        for server in self.db.servers()? {
            self.load(server)?;
            let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
            let known = chat::known_channels(server, log.resolver.state());
            let in_known = |kind: Kind, scope: Scope| {
                kind == Kind::Message && matches!(scope, Scope::Channel(c) if known.contains(&c))
            };
            if let Some(r) =
                log.channel.iter().find(|r| r.id() == id && in_known(r.kind(), r.scope()))
            {
                let Scope::Channel(channel) = r.scope() else { unreachable!("matched") };
                return Ok((server, channel, id, Some(r.clone())));
            }
            if let Some(s) = log.stubs.get(&id).filter(|s| in_known(s.kind(), s.scope())) {
                let Scope::Channel(channel) = s.scope() else { unreachable!("matched") };
                return Ok((server, channel, id, None));
            }
        }
        Err(ErrorCode::NotFound.into())
    }

    /// One shown message as JSON, with its index; `internal` if it isn't
    /// shown (it should be: we just wrote it or it was found).
    fn message(
        &mut self,
        cipher: &dyn GroupCipher,
        server: ServerId,
        channel: ChannelId,
        id: RecordId,
    ) -> CoreResult<Message> {
        self.seed_announced(cipher, server)?;
        let me = self.me.as_ref().map(Me::key);
        let shown = self.shown(cipher, server, channel)?;
        let (index, s) = shown
            .iter()
            .enumerate()
            .find(|(_, s)| s.record.id() == id)
            .ok_or(ErrorCode::Internal)?;
        // Announced by the command itself; sync won't repeat it.
        let state = announced(s, me, &Status { net: &*self.net, db: &self.db });
        self.announced.entry(server).or_default().insert(s.record.id(), state);
        Ok(message_json(s, index, channel, me, &Status { net: &*self.net, db: &self.db }))
    }

    /// A fresh key for `channel`, for `members` (we must be a moderator or
    /// above). Every part is signed once, stored with the others in one
    /// transaction, and only then published: a failure publishes nothing
    /// (#91). A later attempt makes a new rotation, never re-signs a part.
    fn rotate_channel(
        &mut self,
        ports: &Ports,
        server: ServerId,
        channel: ChannelId,
        members: &BTreeSet<PublicKey>,
        auth_deps: &[RecordId],
    ) -> CoreResult<()> {
        let cipher = &*ports.cipher;
        let parts = cipher.rotate(server, channel, members).map_err(cipher_error)?;
        let key = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
        let seq = chat::channel_seq(log, channel, &key);
        let deps = chat::channel_deps(log, channel);
        let mut signed = Vec::new();
        for (i, part) in parts.into_iter().enumerate() {
            signed.push(self.sign_channel(
                server,
                channel,
                seq + i as u64,
                deps.clone(),
                auth_deps.to_vec(),
                ports.clock.now_ms(),
                Kind::KeyRotation,
                part,
            )?);
        }
        self.db.put_records(&signed)?;
        let log = self.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
        for record in &signed {
            log.add(record.clone());
        }
        for record in &signed {
            ports.network.publish(record);
        }
        chat::sync_cipher(cipher, server, log);
        self.keyed_for.insert((server, channel), members.clone());
        Ok(())
    }

    /// After members were removed (a kick or revoke, ours or read): every
    /// known channel whose key removed members still hold gets a fresh one
    /// if we may rotate (`notice: encryption-renewed`); otherwise the UI
    /// learns it waits for a moderator (`notice: rekey-waiting`).
    fn rekey(&mut self, ports: &Ports, server: ServerId, events: &mut Events) -> CoreResult<()> {
        let Some(me) = self.me.as_ref().map(Me::key) else { return Ok(()) };
        let view = self.view(server)?;
        if !view.state.is_member(me) {
            return Ok(());
        }
        let members = view.state.members.clone();
        let may_rotate = view.state.role(me) >= authority::Role::Moderator;
        let auth_deps = auth_deps(&view);
        let cipher = &*ports.cipher;
        cipher.membership_changed(server, &members);
        let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
        chat::sync_cipher(cipher, server, log);
        let server_id = model::hex_id(server.as_bytes());
        for channel in self.known(server)? {
            // A probe: an empty plaintext, never sent.
            let probe = cipher.encrypt(server, channel, b"");
            if probe != Err(CipherError::RotationRequired) {
                // v0.2.0 smoke: no key yet, or members joined since we keyed it
                // (unknown after a restart: once). A moderator or above keys the
                // channel for everyone now, so a joiner can write and read
                // without waiting for a moderator to send first. A failure only
                // leaves things as they were: the next round tries again.
                let stale = self.keyed_for.get(&(server, channel)) != Some(&members);
                let no_key = probe == Err(CipherError::NoKey);
                if may_rotate && (no_key || (probe.is_ok() && stale)) && members.len() > 1 {
                    // Batched (GAP-8): at most one join re-key per channel every
                    // 12-18 s (jitter), covering everyone who joined meanwhile;
                    // later rounds pick up what this one defers.
                    let now = ports.clock.now_ms();
                    let due = self.rekey_after.get(&(server, channel)).is_none_or(|t| now >= *t);
                    if due {
                        let jitter =
                            JOIN_REKEY_MIN_MS + u64::from(OsRng.next_u32()) % JOIN_REKEY_JITTER_MS;
                        self.rekey_after.insert((server, channel), now + jitter);
                        // Fail closed, as for a kick: a failed rotation is never
                        // announced as one, and the UI learns the channel waits.
                        if self
                            .rotate_channel(ports, server, channel, &members, &auth_deps)
                            .is_err()
                        {
                            let kind = NoticeKind::RekeyWaiting;
                            events.push(Event::Notice { kind, server_id: server_id.clone() });
                        }
                    }
                } else if !may_rotate && no_key {
                    // A joiner without a key yet: waiting for a moderator, not ready.
                    let kind = NoticeKind::RekeyWaiting;
                    events.push(Event::Notice { kind, server_id: server_id.clone() });
                }
                continue;
            }
            // Fail closed: a rotation that didn't happen is never announced as
            // one, and the channel stays blocked by `RotationRequired` until the
            // next sync or send retries.
            let renewed = may_rotate
                && self.rotate_channel(ports, server, channel, &members, &auth_deps).is_ok();
            let kind =
                if renewed { NoticeKind::EncryptionRenewed } else { NoticeKind::RekeyWaiting };
            events.push(Event::Notice { kind, server_id: server_id.clone() });
        }
        Ok(())
    }

    /// The server of my default channel `channel_id`; `not-found` for any
    /// other. Loads the server.
    fn find_channel(&mut self, channel_id: &str) -> CoreResult<(ServerId, ChannelId)> {
        let channel =
            model::parse_hex32(channel_id).map(ChannelId::from_bytes).ok_or(ErrorCode::NotFound)?;
        for server in self.db.servers()? {
            self.load(server)?;
            if self.visible(server)?.contains(&channel) {
                return Ok((server, channel));
            }
        }
        Err(ErrorCode::NotFound.into())
    }

    #[allow(clippy::too_many_arguments)]
    fn sign_channel(
        &self,
        server: ServerId,
        channel: ChannelId,
        seq: u64,
        deps: Vec<RecordId>,
        auth_deps: Vec<RecordId>,
        ts: u64,
        kind: Kind,
        body: Vec<u8>,
    ) -> CoreResult<Record> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        Draft { server, scope: Scope::Channel(channel), seq, deps, auth_deps, ts, kind, body }
            .sign(me.identity.signing_key())
            .map_err(|_| ErrorCode::BadRequest.into())
    }

    /// The channel's shown messages, after giving the cipher what's new.
    fn shown(
        &mut self,
        cipher: &dyn GroupCipher,
        server: ServerId,
        channel: ChannelId,
    ) -> CoreResult<Vec<chat::Shown>> {
        let members = self.view(server)?.state.members.clone();
        cipher.membership_changed(server, &members);
        let log = self.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
        chat::sync_cipher(cipher, server, log);
        Ok(chat::messages(cipher, server, channel, log))
    }

    fn message_count(
        &mut self,
        cipher: &dyn GroupCipher,
        server: ServerId,
        channel: ChannelId,
    ) -> CoreResult<usize> {
        Ok(self.shown(cipher, server, channel)?.len())
    }

    fn challenge(&self) -> CoreResult<Option<Challenge>> {
        match self.db.get(KV_RECOVERY_CHALLENGE)? {
            None => Ok(None),
            // Unreadable: start over rather than lock the user out.
            Some(json) => Ok(serde_json::from_slice(&json).ok()),
        }
    }

    fn save_challenge(&self, challenge: &Challenge) -> CoreResult<()> {
        self.db.set(KV_RECOVERY_CHALLENGE, &serde_json::to_vec(challenge).expect("plain struct"))
    }

    fn new_me(&self, phrase: RecoveryPhrase, profile: Profile) -> CoreResult<Me> {
        let identity = phrase.identity();
        let key = *identity.storage_key().expose_secret();
        self.db.save_profile(&key, &serde_json::to_vec(&profile).expect("plain struct"))?;
        self.me_from(phrase, identity, profile)
    }

    /// Keeps the phrase only while the recovery check is still to do.
    fn me_from(
        &self,
        phrase: RecoveryPhrase,
        identity: identity::Identity,
        profile: Profile,
    ) -> CoreResult<Me> {
        let checked = self.vault.recovery_check_done(&identity).map_err(vault_error)?;
        Ok(Me { phrase: (!checked).then_some(phrase), identity, profile })
    }

    fn load_me(&self, phrase: RecoveryPhrase) -> CoreResult<Me> {
        let identity = phrase.identity();
        let key = *identity.storage_key().expose_secret();
        let profile = match self.db.load_profile(&key)? {
            Some(json) => serde_json::from_slice(&json).map_err(|_| ErrorCode::Internal)?,
            None => Profile::default(),
        };
        self.me_from(phrase, identity, profile)
    }

    fn identity_json(&self, me: &Me) -> CoreResult<Identity> {
        let key = me.key();
        Ok(Identity {
            id: model::hex_id(key.as_bytes()),
            short_code: model::short_code(&key),
            display_name: me.profile.display_name.clone(),
            avatar_color: model::color_for(key.as_bytes()),
            avatar_data_url: me.profile.avatar_data_url.clone(),
            bio: me.profile.bio.clone(),
            country: me.profile.country.clone(),
            birth_date: me.profile.birth_date.clone(),
            recovery_checked: self.vault.recovery_check_done(&me.identity).map_err(vault_error)?,
        })
    }

    /// Loads `server`'s log from the store unless it is already in memory.
    fn load(&mut self, server: ServerId) -> CoreResult<()> {
        if !self.logs.contains_key(&server) {
            let mut log = Log::new(server, self.db.records(server)?);
            for stub in self.db.stubs(server)? {
                log.add_stub(stub);
            }
            self.logs.insert(server, log);
        }
        Ok(())
    }

    /// Loads a server I'm in; `not-found` for any other.
    fn load_mine(&mut self, server: ServerId) -> CoreResult<ServerId> {
        if !self.db.has_server(server)? {
            return Err(ErrorCode::NotFound.into());
        }
        self.load(server)?;
        Ok(server)
    }

    /// A loaded server's view.
    fn view(&self, server: ServerId) -> CoreResult<ServerView<'_>> {
        let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
        ServerView::build(server, log).ok_or_else(|| ErrorCode::NotFound.into())
    }

    /// Offers a record to the loaded log and stores whatever joined because
    /// of it; false if nothing joined (a duplicate, or still waiting on deps).
    fn store(&mut self, server: ServerId, record: Record) -> CoreResult<bool> {
        let log = self.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
        let joined = log.add(record);
        for record in &joined {
            self.db.put_record(record)?;
        }
        Ok(!joined.is_empty())
    }

    fn members_json(&self, view: &ServerView) -> CoreResult<Vec<Member>> {
        let me = self.me.as_ref().map(Me::key);
        let server_id = model::hex_id(view.id.as_bytes());
        let mut out = Vec::new();
        for member in view.members() {
            let blocked = self.db.is_blocked(&member.key)?;
            out.push(Member {
                id: model::hex_id(member.key.as_bytes()),
                server_id: server_id.clone(),
                display_name: member.display_name,
                short_code: model::short_code(&member.key),
                avatar_color: model::color_for(member.key.as_bytes()),
                // TODO(sync): real presence from connections.
                presence: if Some(member.key) == me { Presence::Online } else { Presence::Offline },
                is_owner: member.key == view.owner,
                role: member.role,
                blocked: blocked.then_some(true),
                bio: None,
            });
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn join(
        &mut self,
        ports: &Ports,
        server: ServerId,
        invite: RecordId,
        secret: &[u8; 32],
        link: &str,
        fetched: Vec<Record>,
        events: &mut Events,
    ) -> CoreResult<JoinStatus> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let link = link.to_owned();
        let server_hex = model::hex_id(server.as_bytes());
        if self.db.has_server(server)? {
            return Ok(JoinStatus::Joined { link, server_id: server_hex });
        }
        if fetched.is_empty() {
            return Ok(JoinStatus::NobodyOnline { link });
        }
        let mut log = Log::new(server, fetched);
        let Some(view) = ServerView::build(server, &log) else {
            return Ok(JoinStatus::Invalid { link });
        };
        let required = view.genesis.age_restriction;
        let age = model::age_on(&me.profile.birth_date, ports.clock.now_ms());
        if required.min_age() > 0 && age.is_none_or(|age| age < required.min_age()) {
            return Ok(JoinStatus::AgeBlocked { link, required });
        }
        let key = me.key();
        if view.is_kicked(&key) {
            // TODO(ui): a "you were removed from this server" phase.
            return Ok(JoinStatus::Invalid { link });
        }
        let sign = |view: &ServerView, deps, action: &Action| {
            authority::sign_control(
                me.identity.signing_key(),
                server,
                view.next_seq(&key),
                deps,
                ports.clock.now_ms(),
                action,
            )
            .map_err(|_| CoreError::from(ErrorCode::BadRequest))
        };
        // Already a member (this device lost its copy): no new join needed.
        let mut mine = Vec::new();
        if !view.state.is_member(key) {
            // An invite we don't hold, revoked or used up: `authority`
            // refuses the join, and nothing is stored or sent.
            // TODO(ui): phases for "invite expired" and "invite used up".
            let join = sign(&view, view.deps(Some(invite)), &Action::Join { invite })?;
            log.add(join.clone());
            if !log.accepted(&join.id()) {
                return Ok(JoinStatus::Invalid { link });
            }
            mine.push(join);
        }
        let view = ServerView::build(server, &log).ok_or(ErrorCode::Internal)?;
        let profile =
            sign(&view, view.deps(None), &model::profile_action(&me.profile.display_name))?;
        for record in log.records.iter().chain(&log.channel) {
            self.db.put_record(record)?;
        }
        self.logs.insert(server, log);
        mine.push(profile.clone());
        self.store(server, profile)?;
        let claim = self.sign_age_claim(ports, server)?;
        mine.push(claim.clone());
        self.store(server, claim)?;
        for record in &mine {
            ports.network.publish(record);
        }
        self.save_secret(server, secret)?;
        self.db.add_server(server, ports.clock.now_ms())?;
        events.push(Event::ServerAdded(self.summary(server)?));
        Ok(JoinStatus::Joined { link, server_id: server_hex })
    }
}

impl ServerView<'_> {
    /// `me` picks my own invite for the link when I have one.
    /// `secret`: the server's sync secret, which the link carries (GAP-4).
    fn summary(&self, me: Option<PublicKey>, secret: Option<&[u8; 32]>) -> ServerSummary {
        let id = model::hex_id(self.id.as_bytes());
        // No usable invite left, or no secret: no link.
        let invite_link = match (self.invite(me), secret) {
            (Some(invite), Some(secret)) => model::invite_link(self.id, invite, secret),
            _ => String::new(),
        };
        ServerSummary {
            name: self.genesis.name.clone(),
            initials: model::initials(&self.genesis.name),
            color: model::color_for(self.id.as_bytes()),
            age_restriction: self.genesis.age_restriction,
            owner_id: model::hex_id(self.owner.as_bytes()),
            invite_link,
            unread: false,
            mentions: 0,
            id,
        }
    }

    /// The channels authority knows, with their names as created; a server
    /// from before #113 shows its derived #general.
    /// TODO(#116): hide 18+ channels from members without an 18+ claim.
    fn channels(&self, me: Option<PublicKey>) -> Vec<Channel> {
        let server_id = model::hex_id(self.id.as_bytes());
        let channel = |id: ChannelId, name: &str, adult: bool, general: bool| Channel {
            id: model::hex_id(id.as_bytes()),
            server_id: server_id.clone(),
            name: name.to_owned(),
            adult,
            unread: false,
            mentions: 0,
            is_default: general,
        };
        if self.state.channels.is_empty() && self.state.deleted_channels.is_empty() {
            return vec![channel(model::default_channel(self.id), model::GENERAL, false, true)];
        }
        // B7: 18+ channels only for members with an accepted 18+ claim.
        let adult = me.is_some_and(|me| self.state.adults.contains(&me));
        self.state
            .channels
            .iter()
            .filter(|(_, c)| adult || !c.adult)
            .map(|(id, c)| channel(*id, &c.name, c.adult, c.general))
            .collect()
    }
}

/// Control heads a channel record relies on, at most `MAX_AUTH_DEPS`.
fn auth_deps(view: &ServerView) -> Vec<RecordId> {
    let heads = view.deps(None);
    heads[heads.len().saturating_sub(records::MAX_AUTH_DEPS)..].to_vec()
}

/// What was last announced of a message: edit time, deleted, my send state.
type Announced = (Option<u64>, bool, Option<SendState>);

/// Where my messages' send states come from: the network's live state and
/// the last one stored, so a restart doesn't lose it.
struct Status<'a> {
    net: &'a dyn Network,
    db: &'a Db,
}

fn announced(s: &chat::Shown, me: Option<PublicKey>, st: &Status) -> Announced {
    (s.edited_at, s.deleted, send_state(s, me, st))
}

/// B6, on my own messages only: `pending` until it left this device,
/// `sent` once it did (`NodeEvent::Sent`), `received` once a member's node
/// acknowledged it (`NodeEvent::Delivered`). The higher of the live state
/// and the stored one counts, and a live advance is stored, so a message
/// written offline is still `pending` after a restart. A network that
/// doesn't track this (in process) says `sent`.
fn send_state(s: &chat::Shown, me: Option<PublicKey>, st: &Status) -> Option<SendState> {
    if Some(s.record.author()) != me {
        return None;
    }
    let id = s.record.id();
    let live = st.net.send_state(id);
    let stored = st.db.send_state(id).ok().flatten();
    if live.map(rank) > stored.map(rank) {
        // Best effort: losing it only shows an older state.
        let _ = st.db.set_send_state(id, live.expect("compared above"));
    }
    Some(match (live, stored) {
        (None, None) => SendState::Sent,
        (a, b) => a.into_iter().chain(b).max_by_key(|s| rank(*s)).expect("one is set"),
    })
}

fn rank(state: SendState) -> u8 {
    match state {
        SendState::Pending | SendState::Failed => 0,
        SendState::Sent => 1,
        SendState::Received => 2,
    }
}

fn message_json(
    s: &chat::Shown,
    index: usize,
    channel: ChannelId,
    me: Option<PublicKey>,
    st: &Status,
) -> Message {
    Message {
        id: model::hex_id(s.record.id().as_bytes()),
        channel_id: model::hex_id(channel.as_bytes()),
        index,
        author_id: model::hex_id(s.record.author().as_bytes()),
        body: s.text.clone(),
        created_at: s.record.ts(),
        edited_at: s.edited_at,
        deleted: s.deleted.then_some(true),
        send_state: send_state(s, me, st),
    }
}

/// Can't write yet (no key, or a rotation only a moderator can make):
/// `not-allowed`. No group encryption: `not-implemented`.
fn cipher_error(error: CipherError) -> CoreError {
    match error {
        CipherError::NoKey | CipherError::RotationRequired => ErrorCode::NotAllowed.into(),
        CipherError::Unavailable => ErrorCode::NotImplemented.into(),
        CipherError::Rejected => ErrorCode::Internal.into(),
    }
}

/// Hands the group cipher our member secret: at identity load, create and
/// restore, and again after the app lock (`Core::unlock_keys`). The next
/// read or send gives it the channels, the accepted set and the stored
/// rotations again (`chat::sync_cipher`).
fn unlock_cipher(ports: &Ports, me: Option<&Me>) {
    if let Some(me) = me {
        ports.cipher.unlock(Arc::new(MemberSecret::new(me.identity.signing_key())));
        // GAP-4: peer bindings with the identity key.
        ports.network.unlock(me.identity.signing_key());
    }
}
