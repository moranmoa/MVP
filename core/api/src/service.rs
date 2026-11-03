//! The core: `CoreCommands` from `ui/src/core/api.ts`, one method each.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use authority::Action;
use identity::{PhraseError, RecoveryPhrase, Vault, VaultError, VaultState};
use rand_core::{OsRng, RngCore};
use records::{PublicKey, Record, ServerId};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::model::{self, GenesisBody, JoinBody, ServerView};
use crate::ports::{CipherOutcome, Ports, SharedSecrets};
use crate::types::*;

const DEFAULT_STORAGE_LIMIT: u64 = 5 * 1024 * 1024 * 1024;
const KV_STORAGE_LIMIT: &str = "storage-limit";
const KV_UPDATE_CHANNEL: &str = "update-channel";

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

struct Me {
    phrase: RecoveryPhrase,
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
        let mut inner = Inner { db, vault, me: None };
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
            let words = me.phrase.words().iter().map(|w| (*w).to_owned()).collect();
            let identity = inner.identity_json(&me)?;
            inner.me = Some(me);
            events.push(progress(ProgressStep::Done, 100));
            Ok(CreatedIdentity { identity, words })
        })
    }

    /// `answers`: 1-based position → typed word.
    pub fn check_recovery_words(&self, answers: BTreeMap<usize, String>) -> CoreResult<bool> {
        self.run(|inner, _, events| {
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            if answers.keys().any(|p| *p == 0) {
                return Ok(false);
            }
            let answers: Vec<(usize, &str)> =
                answers.iter().map(|(p, w)| (p - 1, w.as_str())).collect();
            let ok =
                inner.vault.confirm_recovery_check(&me.phrase, &answers).map_err(vault_error)?;
            if ok {
                events.push(Event::IdentityChanged(inner.identity_json(me)?));
            }
            Ok(ok)
        })
    }

    pub fn get_recovery_words(&self) -> CoreResult<Vec<String>> {
        let inner = self.lock();
        let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
        Ok(me.phrase.words().iter().map(|w| (*w).to_owned()).collect())
    }

    pub fn restore_identity(&self, words: Vec<String>) -> CoreResult<Identity> {
        self.run(|inner, _, _| {
            if inner.me.is_some() {
                return Err(ErrorCode::NotAllowed.into());
            }
            let phrase = RecoveryPhrase::parse(&words.join(" ")).map_err(phrase_error)?;
            inner.vault.restore(&phrase).map_err(vault_error)?;
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
            let me = inner.me.as_mut().ok_or(ErrorCode::NotFound)?;
            me.profile.display_name = input.display_name.trim().to_owned();
            me.profile.bio = input.bio;
            me.profile.avatar_data_url = input.avatar_data_url;
            // TODO(profiles): publish the new name to my servers; members
            // see the name from my join post until then.
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
        let inner = self.lock();
        let mut out = Vec::new();
        for id in inner.db.servers()? {
            out.push(inner.view(id)?.summary());
        }
        Ok(out)
    }

    pub fn list_channels(&self, server_id: &str) -> CoreResult<Vec<Channel>> {
        let inner = self.lock();
        let view = inner.my_view(parse_server(server_id)?)?;
        Ok(vec![view.default_channel()])
    }

    pub fn list_members(&self, server_id: &str) -> CoreResult<Vec<Member>> {
        let inner = self.lock();
        let view = inner.my_view(parse_server(server_id)?)?;
        inner.members_json(&view)
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
            let join = authority::sign_control(
                me.identity.signing_key(),
                server,
                1,
                vec![genesis.id()],
                ports.clock.now_ms(),
                &Action::Post { text: JoinBody::new(&me.profile.display_name).text() },
            )
            .map_err(|_| ErrorCode::BadRequest)?;
            for record in [&genesis, &join] {
                inner.db.put_record(record)?;
                ports.network.publish(record);
            }
            inner.db.add_server(server, ports.clock.now_ms())?;
            let summary = inner.view(server)?.summary();
            events.push(Event::ServerAdded(summary.clone()));
            Ok(summary)
        })
    }

    /// Resolves with the first status; the outcome follows as `join.status`.
    pub fn join_server(&self, link: &str) -> CoreResult<JoinStatus> {
        let link = link.trim().to_owned();
        let Some(server) = link
            .strip_prefix(model::INVITE_PREFIX)
            .and_then(model::parse_hex32)
            .map(ServerId::from_bytes)
        else {
            return Ok(JoinStatus::Invalid { link });
        };
        let searching = JoinStatus::Searching { link: link.clone() };
        self.ports.events.emit(&Event::JoinStatus(searching.clone()));
        self.run(|inner, ports, events| {
            let status = inner.join(ports, server, &link, events)?;
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
        self.control_action(server_id, Action::Revoke { target: parse_user(user_id)? })
    }

    /// Allowed for moderators and up, against anyone ranked lower
    /// (`authority`'s rule; the mock only lets the owner kick).
    pub fn kick_member(&self, server_id: &str, user_id: &str) -> CoreResult<()> {
        self.control_action(server_id, Action::Kick { target: parse_user(user_id)? })
    }

    fn control_action(&self, server_id: &str, action: Action) -> CoreResult<()> {
        let server = parse_server(server_id)?;
        self.run(|inner, ports, events| {
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            let view = inner.my_view(server)?;
            if action.clone() == (Action::Kick { target: me.key() }) {
                return Err(ErrorCode::NotAllowed.into());
            }
            let record = authority::sign_control(
                me.identity.signing_key(),
                server,
                view.next_seq(&me.key()),
                view.heads(),
                ports.clock.now_ms(),
                &action,
            )
            .map_err(|_| ErrorCode::BadRequest)?;
            let mut records = view.records.clone();
            records.push(record.clone());
            if !authority::resolve(server, &records).accepted.contains(&record.id()) {
                return Err(ErrorCode::NotAllowed.into());
            }
            inner.db.put_record(&record)?;
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

    /// Rust addition (`syncNow`): pulls every server's records from the
    /// network and reports what changed. TODO(sync): driven by the sync
    /// crate's push and pull instead of a command.
    pub fn sync_now(&self) -> CoreResult<()> {
        self.run(|inner, ports, events| {
            let Some(me) = inner.me.as_ref() else { return Ok(()) };
            let me = me.key();
            for server in inner.db.servers()? {
                let before = inner.view(server)?;
                let fresh: Vec<Record> = ports
                    .network
                    .fetch(server)
                    .into_iter()
                    .filter(|r| r.server() == server)
                    .collect();
                for record in &fresh {
                    inner.db.put_record(record)?;
                }
                let after = inner.view(server)?;
                if after.is_kicked(&me) {
                    inner.db.remove_server(server)?;
                    events
                        .push(Event::ServerRemoved { server_id: model::hex_id(server.as_bytes()) });
                } else if after.records.len() != before.records.len() {
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
    fn new_me(&self, phrase: RecoveryPhrase, profile: Profile) -> CoreResult<Me> {
        let identity = phrase.identity();
        let key = *identity.storage_key().expose_secret();
        self.db.save_profile(&key, &serde_json::to_vec(&profile).expect("plain struct"))?;
        Ok(Me { phrase, identity, profile })
    }

    fn load_me(&self, phrase: RecoveryPhrase) -> CoreResult<Me> {
        let identity = phrase.identity();
        let key = *identity.storage_key().expose_secret();
        let profile = match self.db.load_profile(&key)? {
            Some(json) => serde_json::from_slice(&json).map_err(|_| ErrorCode::Internal)?,
            None => Profile::default(),
        };
        Ok(Me { phrase, identity, profile })
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

    fn view(&self, server: ServerId) -> CoreResult<ServerView> {
        ServerView::build(server, self.db.records(server)?)
            .ok_or_else(|| ErrorCode::NotFound.into())
    }

    /// A server I'm in.
    fn my_view(&self, server: ServerId) -> CoreResult<ServerView> {
        if !self.db.has_server(server)? {
            return Err(ErrorCode::NotFound.into());
        }
        self.view(server)
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
        link: &str,
        events: &mut Events,
    ) -> CoreResult<JoinStatus> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let link = link.to_owned();
        let server_hex = model::hex_id(server.as_bytes());
        if self.db.has_server(server)? {
            return Ok(JoinStatus::Joined { link, server_id: server_hex });
        }
        let fetched: Vec<Record> =
            ports.network.fetch(server).into_iter().filter(|r| r.server() == server).collect();
        if fetched.is_empty() {
            return Ok(JoinStatus::NobodyOnline { link });
        }
        let Some(view) = ServerView::build(server, fetched.clone()) else {
            return Ok(JoinStatus::Invalid { link });
        };
        let required = view.genesis.age_restriction;
        let age = model::age_on(&me.profile.birth_date, ports.clock.now_ms());
        if required.min_age() > 0 && age.is_none_or(|age| age < required.min_age()) {
            return Ok(JoinStatus::AgeBlocked { link, required });
        }
        if view.is_kicked(&me.key()) {
            // TODO(ui): a "you were removed from this server" phase.
            return Ok(JoinStatus::Invalid { link });
        }
        let join = authority::sign_control(
            me.identity.signing_key(),
            server,
            view.next_seq(&me.key()),
            view.heads(),
            ports.clock.now_ms(),
            &Action::Post { text: JoinBody::new(&me.profile.display_name).text() },
        )
        .map_err(|_| ErrorCode::BadRequest)?;
        for record in fetched.iter().chain([&join]) {
            self.db.put_record(record)?;
        }
        ports.network.publish(&join);
        self.db.add_server(server, ports.clock.now_ms())?;
        events.push(Event::ServerAdded(self.view(server)?.summary()));
        Ok(JoinStatus::Joined { link, server_id: server_hex })
    }
}

impl ServerView {
    fn summary(&self) -> ServerSummary {
        let id = model::hex_id(self.id.as_bytes());
        ServerSummary {
            name: self.genesis.name.clone(),
            initials: model::initials(&self.genesis.name),
            color: model::color_for(self.id.as_bytes()),
            age_restriction: self.genesis.age_restriction,
            owner_id: model::hex_id(self.owner.as_bytes()),
            invite_link: format!("{}{id}", model::INVITE_PREFIX),
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
