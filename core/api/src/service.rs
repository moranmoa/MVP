//! The core: `CoreCommands` from `ui/src/core/api.ts`, one method each.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use authority::Action;
use identity::{PhraseError, RecoveryPhrase, Vault, VaultError, VaultState};
use rand_core::{OsRng, RngCore};
use records::{ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId};
use serde::{Deserialize, Serialize};

use crate::chat;
use crate::db::Db;
use crate::model::{self, GenesisBody, Log, ServerView};
use crate::ports::{CipherError, CipherOutcome, GroupCipher, Ports, SharedSecrets};
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
        let mut inner = Inner { db, vault, me: None, logs: HashMap::new() };
        if inner.vault.state().map_err(vault_error)? == VaultState::Ready {
            let phrase = inner.vault.open().map_err(vault_error)?;
            inner.me = Some(inner.load_me(phrase)?);
        }
        Ok(Core { inner: Mutex::new(inner), ports, app_version: config.app_version })
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
        self.run(|inner, _, events| {
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
                ..Profile::default()
            };
            let me = inner.new_me(phrase, profile)?;
            events.push(progress(ProgressStep::Encryption, 85));
            let phrase = me.phrase.as_ref().ok_or(ErrorCode::Internal)?;
            let words = phrase.words().iter().map(|w| (*w).to_owned()).collect();
            let identity = inner.identity_json(&me)?;
            inner.me = Some(me);
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
        self.run(|inner, _, _| {
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
            out.push(inner.view(id)?.summary(inner.me.as_ref().map(Me::key)));
        }
        Ok(out)
    }

    pub fn list_channels(&self, server_id: &str) -> CoreResult<Vec<Channel>> {
        let mut inner = self.lock();
        let server = inner.load_mine(parse_server(server_id)?)?;
        Ok(vec![inner.view(server)?.default_channel()])
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
            if name.is_empty() || name.chars().count() > model::MAX_NAME_CHARS {
                return Err(ErrorCode::BadRequest.into());
            }
            let mut nonce = [0u8; records::GENESIS_NONCE_LEN];
            OsRng.fill_bytes(&mut nonce);
            let body = GenesisBody { v: 1, name: name.to_owned(), age_restriction };
            let genesis = authority::sign_genesis(
                me.identity.signing_key(),
                nonce,
                ports.clock.now_ms(),
                &serde_json::to_vec(&body).expect("plain struct"),
            )
            .map_err(|_| ErrorCode::BadRequest)?;
            let server = genesis.server();
            // The owner is a member from the genesis: the first invite (for
            // the link) and the profile post follow it.
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
            let invite = sign(1, vec![genesis.id()], &model::FIRST_INVITE)?;
            let profile =
                sign(2, vec![invite.id()], &model::profile_action(&me.profile.display_name))?;
            let me_key = me.key();
            inner.logs.insert(server, Log::new(server, []));
            for record in [genesis, invite, profile] {
                ports.network.publish(&record);
                inner.store(server, record)?;
            }
            inner.db.add_server(server, ports.clock.now_ms())?;
            let summary = inner.view(server)?.summary(Some(me_key));
            events.push(Event::ServerAdded(summary.clone()));
            Ok(summary)
        })
    }

    /// Resolves with the first status; the outcome follows as `join.status`.
    pub fn join_server(&self, link: &str) -> CoreResult<JoinStatus> {
        let link = link.trim().to_owned();
        let Some((server, invite)) = model::parse_invite_link(&link) else {
            return Ok(JoinStatus::Invalid { link });
        };
        let searching = JoinStatus::Searching { link: link.clone() };
        self.ports.events.emit(&Event::JoinStatus(searching.clone()));
        // Fetch without the lock, so UI commands don't wait on the network.
        let fetched: Vec<Record> =
            self.ports.network.fetch(server).into_iter().filter(|r| r.server() == server).collect();
        self.run(|inner, ports, events| {
            let status = inner.join(ports, server, invite, &link, fetched, events)?;
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
            if matches!(action, Action::Kick { .. }) {
                let members: BTreeSet<PublicKey> = after.members().iter().map(|m| m.key).collect();
                if ports.cipher.membership_changed(server, &members) == CipherOutcome::Renewed {
                    events.push(Event::Notice {
                        kind: NoticeKind::EncryptionRenewed,
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
            let cipher = &*ports.cipher;
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            let key = me.key();
            let view = inner.view(server)?;
            let members = view.state.members.clone();
            let may_rotate = view.state.role(key) >= authority::Role::Moderator;
            let auth_deps = auth_deps(&view);
            cipher.membership_changed(server, &members);
            let log = inner.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
            chat::sync_cipher(cipher, server, log);
            let sealed = match cipher.encrypt(server, channel, body.as_bytes()) {
                Err(CipherError::NoKey | CipherError::RotationRequired) if may_rotate => {
                    let parts = cipher.rotate(server, channel, &members).map_err(cipher_error)?;
                    // All parts or none, each signed once (see `GroupCipher::rotate`).
                    let seq = chat::channel_seq(log, channel, &key);
                    let deps = chat::channel_deps(log, channel);
                    let mut signed = Vec::new();
                    for (i, part) in parts.into_iter().enumerate() {
                        signed.push(inner.sign_channel(
                            server,
                            channel,
                            seq + i as u64,
                            deps.clone(),
                            auth_deps.clone(),
                            ports.clock.now_ms(),
                            Kind::KeyRotation,
                            part,
                        )?);
                    }
                    for record in signed {
                        inner.store(server, record.clone())?;
                        ports.network.publish(&record);
                    }
                    let log = inner.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
                    chat::sync_cipher(cipher, server, log);
                    cipher.encrypt(server, channel, body.as_bytes()).map_err(cipher_error)?
                }
                other => other.map_err(cipher_error)?,
            };
            let log = inner.logs.get(&server).ok_or(ErrorCode::Internal)?;
            let seq = chat::channel_seq(log, channel, &key);
            let deps = chat::channel_deps(log, channel);
            let record = inner.sign_channel(
                server,
                channel,
                seq,
                deps,
                auth_deps,
                ports.clock.now_ms(),
                Kind::Message,
                sealed,
            )?;
            inner.store(server, record.clone())?;
            ports.network.publish(&record);
            let index = inner.message_count(cipher, server, channel)?.saturating_sub(1);
            let message = message_json(&record, body, index, channel, Some(SendState::Sent));
            events.push(Event::MessageAdded(message.clone()));
            Ok(message)
        })
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
                let mine = Some(s.record.author()) == me;
                message_json(
                    &s.record,
                    &s.text,
                    start + i,
                    channel,
                    mine.then_some(SendState::Sent),
                )
            })
            .collect();
        Ok(MessageRange { channel_id: channel_id.to_owned(), start, messages })
    }

    /// Rust addition (`syncNow`): pulls every server's records from the
    /// network and reports what changed. TODO(sync): driven by the sync
    /// crate's push and pull instead of a command.
    pub fn sync_now(&self) -> CoreResult<()> {
        let servers = {
            let inner = self.lock();
            if inner.me.is_none() {
                return Ok(());
            }
            inner.db.servers()?
        };
        // Fetch without the lock, so UI commands don't wait on the network.
        let fetched: Vec<(ServerId, Vec<Record>)> =
            servers.into_iter().map(|s| (s, self.ports.network.fetch(s))).collect();
        self.run(|inner, _, events| {
            let Some(me) = inner.me.as_ref() else { return Ok(()) };
            let me = me.key();
            for (server, records) in fetched {
                if !inner.db.has_server(server)? {
                    continue;
                }
                inner.load(server)?;
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
    /// The server of my default channel `channel_id`; `not-found` for any
    /// other. Loads the server.
    fn find_channel(&mut self, channel_id: &str) -> CoreResult<(ServerId, ChannelId)> {
        let channel =
            model::parse_hex32(channel_id).map(ChannelId::from_bytes).ok_or(ErrorCode::NotFound)?;
        for server in self.db.servers()? {
            if model::default_channel(server) == channel {
                self.load(server)?;
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
            let log = Log::new(server, self.db.records(server)?);
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

    fn join(
        &mut self,
        ports: &Ports,
        server: ServerId,
        invite: RecordId,
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
        for record in &mine {
            ports.network.publish(record);
        }
        self.db.add_server(server, ports.clock.now_ms())?;
        events.push(Event::ServerAdded(self.view(server)?.summary(Some(key))));
        Ok(JoinStatus::Joined { link, server_id: server_hex })
    }
}

impl ServerView<'_> {
    /// `me` picks my own invite for the link when I have one.
    fn summary(&self, me: Option<PublicKey>) -> ServerSummary {
        let id = model::hex_id(self.id.as_bytes());
        // No usable invite left: no link, until someone makes a new one.
        let invite_link =
            self.invite(me).map(|invite| model::invite_link(self.id, invite)).unwrap_or_default();
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

    fn default_channel(&self) -> Channel {
        Channel {
            id: model::hex_id(model::default_channel(self.id).as_bytes()),
            server_id: model::hex_id(self.id.as_bytes()),
            name: "general".to_owned(),
            adult: false,
            unread: false,
            mentions: 0,
        }
    }
}

/// Control heads a channel record relies on, at most `MAX_AUTH_DEPS`.
fn auth_deps(view: &ServerView) -> Vec<RecordId> {
    let heads = view.deps(None);
    heads[heads.len().saturating_sub(records::MAX_AUTH_DEPS)..].to_vec()
}

fn message_json(
    record: &Record,
    text: &str,
    index: usize,
    channel: ChannelId,
    send_state: Option<SendState>,
) -> Message {
    Message {
        id: model::hex_id(record.id().as_bytes()),
        channel_id: model::hex_id(channel.as_bytes()),
        index,
        author_id: model::hex_id(record.author().as_bytes()),
        body: text.to_owned(),
        created_at: record.ts(),
        send_state,
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
