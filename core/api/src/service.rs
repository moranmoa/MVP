//! The core: `CoreCommands` from `ui/src/core/api.ts`, one method each.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use authority::{Action, Cut};
use groupkeys::MemberSecret;
use identity::{PhraseError, RecoveryPhrase, Vault, VaultError, VaultState};
use rand_core::{OsRng, RngCore};
use records::{ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::chat;
use crate::db::Db;
use crate::model::{self, GenesisBody, Log, ServerView};
use crate::ports::{CipherError, GroupCipher, Network, Ports, Reach, SharedSecrets};
use crate::rekey::JoinBucket;
use crate::types::*;
use crate::update::Updates;
use crate::voice::{self, VoiceNet};
use updater::client::UpdateEvent;

const DEFAULT_STORAGE_LIMIT: u64 = 5 * 1024 * 1024 * 1024;
const KV_STORAGE_LIMIT: &str = "storage-limit";
const KV_UPDATE_CHANNEL: &str = "update-channel";
/// GAP-6r: `[1]` while relay-only is on. Per device, not per identity: it's
/// about this computer's network.
const KV_RELAY_ONLY: &str = "relay-only";

/// Fails closed: off only when never set or set to off (`[0]`); any other
/// value is on.
fn relay_only_in(db: &Db) -> CoreResult<bool> {
    Ok(!matches!(db.get(KV_RELAY_ONLY)?.as_deref(), None | Some([0])))
}

/// GAP-6r: the stored relay-only setting in the core database at
/// `db_path`, read before the network starts so the node starts with it.
/// Fails closed: a database that can't be opened or read means on, so an
/// error never starts a relay-only user with direct connections. A fresh
/// data dir (no `core.db` yet) is off: the database is created empty.
pub fn stored_relay_only(db_path: &Path) -> bool {
    match Db::open(Some(db_path)).and_then(|db| relay_only_in(&db)) {
        Ok(on) => on,
        Err(e) => {
            // No path: it names the user's profile folder.
            eprintln!("mvs-core: can't read the relay-only setting ({:?}); starting relay-only", e.code);
            true
        }
    }
}
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
/// How long a message held for its channel key (the re-key a moderator
/// makes when they read the join) waits before it is marked `failed`.
pub const KEY_PENDING_FOR: Duration = Duration::from_secs(120);
/// Provisional IDs of messages held for their key.
pub const KEY_PENDING_PREFIX: &str = "pending-";
/// Messages held for their key per channel at once; more get `not-allowed`.
pub const MAX_KEY_PENDING: usize = 50;
/// A join that found nobody online is retried in the background: first
/// after `JOIN_RETRY_FIRST`, doubling up to `JOIN_RETRY_MAX`, for at most
/// `JOIN_RETRY_FOR` (until then the inviter may still come online).
pub const JOIN_RETRY_FIRST: Duration = Duration::from_secs(2);
pub const JOIN_RETRY_MAX: Duration = Duration::from_secs(30);
pub const JOIN_RETRY_FOR: Duration = Duration::from_secs(10 * 60);
/// Server lifecycle: how long a closed server stays listed, read-only,
/// after its close was received (the Tech Lead's #229 answer).
pub const CLOSED_RETENTION_MS: u64 = 30 * 24 * 3600 * 1000;
/// The shortest wait the sync loop takes for a due join re-key.
pub const REKEY_WAKE_FLOOR: Duration = Duration::from_millis(250);

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
    /// The OS keychain failed at startup (QA-20).
    keyring_unavailable: bool,
    /// Loaded on first use per server, then kept current record by record.
    logs: HashMap<ServerId, Log>,
    /// Messages already announced to the UI, per server, with what was
    /// announced (edit time, deleted), so sync emits only what is new.
    announced: HashMap<ServerId, HashMap<RecordId, Announced>>,
    /// For my messages' send state (B6).
    net: Arc<dyn Network>,
    /// The online members last reported per server (M2).
    presence_seen: HashMap<ServerId, Reach>,
    /// Who each channel was last keyed for by our own rotation, so a
    /// moderator re-keys when members join (in memory: once after a restart).
    keyed_for: HashMap<(ServerId, ChannelId), BTreeSet<PublicKey>>,
    /// Join re-key tokens per server (GAP-8, `rekey`), and the servers
    /// whose joiners wait for the next token.
    join_buckets: HashMap<ServerId, JoinBucket>,
    join_waiting: BTreeSet<ServerId>,
    /// Voice calls: mine, and who is in which call.
    calls: voice::Calls,
    /// Messages written before their channel key reached me (a member who
    /// just joined), oldest first: sent by the sync loop when it comes.
    /// In memory only (never the plain text on disk): lost on restart.
    key_pending: Vec<KeyPending>,
}

/// A message held for its channel key (`Core::send_message`).
struct KeyPending {
    /// Provisional, `pending-` + 16 random hex: never a record ID.
    id: String,
    server: ServerId,
    channel: ChannelId,
    author: PublicKey,
    body: Zeroizing<String>,
    created_at: u64,
    failed: bool,
}

impl KeyPending {
    fn message(&self, index: usize) -> Message {
        Message {
            id: self.id.clone(),
            channel_id: model::hex_id(self.channel.as_bytes()),
            index,
            author_id: model::hex_id(self.author.as_bytes()),
            body: self.body.as_str().to_owned(),
            created_at: self.created_at,
            edited_at: None,
            deleted: None,
            send_state: Some(if self.failed { SendState::Failed } else { SendState::Pending }),
            replaces: None,
        }
    }
}

pub struct Core {
    inner: Mutex<Inner>,
    ports: Ports,
    app_version: String,
    /// In-app update; `None` unless `--update-root` (see `with_updates`).
    updates: Option<Arc<Updates>>,
    /// Joins that found nobody online yet, by link: retried by `sync_now`.
    pending_joins: Mutex<HashMap<String, PendingJoin>>,
}

struct PendingJoin {
    next: Instant,
    wait: Duration,
    until: Instant,
}

/// Events collected while the lock is held, emitted after it is released.
type Events = Vec<Event>;

fn vault_error(e: VaultError) -> CoreError {
    match e {
        // QA-20: the OS keychain failed (no Secret Service, locked, denied).
        VaultError::Store(_) => ErrorCode::KeyringUnavailable.into(),
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

/// Server lifecycle's rollout gate (#229): every current, non-kicked member
/// but `me` has `server-lifecycle-v1` in their latest profile post (none
/// counts as needing an update).
fn gate_open(view: &ServerView, me: PublicKey) -> bool {
    let caps = view.lifecycle_caps();
    view.state
        .members
        .iter()
        .filter(|m| **m != me && !view.state.kicked.contains(m))
        .all(|m| caps.get(m) == Some(&true))
}

impl Core {
    pub fn open(config: Config, ports: Ports) -> CoreResult<Core> {
        let db = Db::open(config.db_path.as_deref())?;
        let vault = Vault::new(SharedSecrets(ports.secrets.clone()));
        let mut inner = Inner {
            db,
            vault,
            me: None,
            keyring_unavailable: false,
            logs: HashMap::new(),
            announced: HashMap::new(),
            net: ports.network.clone(),
            presence_seen: HashMap::new(),
            keyed_for: HashMap::new(),
            join_buckets: HashMap::new(),
            join_waiting: BTreeSet::new(),
            calls: voice::Calls::default(),
            key_pending: Vec::new(),
        };
        // GAP-6r: the stored relay-only setting, applied before anything
        // else reaches the network.
        let relay_only = relay_only_in(&inner.db)?;
        inner.calls.set_relay_only(relay_only);
        ports.network.set_relay_only(relay_only);
        // QA-20: without a usable keychain the core still starts, and says so
        // (`keyring-unavailable`) instead of crashing or looking empty.
        let state = match inner.vault.state() {
            Ok(state) => Some(state),
            Err(VaultError::Store(e)) => {
                eprintln!("mvs-core: the OS keychain is unavailable: {}", e.0);
                inner.keyring_unavailable = true;
                None
            }
            Err(e) => return Err(vault_error(e)),
        };
        if state == Some(VaultState::Ready) {
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
        Ok(Core {
            inner: Mutex::new(inner),
            ports,
            app_version: config.app_version,
            updates: None,
            pending_joins: Mutex::default(),
        })
    }

    /// What the app lock and sign-out call: the group cipher drops our
    /// secret and every channel key, and we leave any call. TODO(app lock):
    /// the commands.
    pub fn lock_keys(&self) {
        let _ = self.leave_call();
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
        if inner.keyring_unavailable {
            return Err(ErrorCode::KeyringUnavailable.into());
        }
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
        self.restore_identity_with(words, None)
    }

    /// `restoreIdentity(words, input?)`: the words don't carry the country
    /// or birth date (spec, age §), so the UI asks them again before the
    /// words and passes them here, as `createIdentity` takes them. Without
    /// `input` this device has neither, and a rejoin keeps what my servers
    /// already hold (no age claim from no date).
    pub fn restore_identity_with(
        &self,
        words: Vec<String>,
        input: Option<NewIdentityInput>,
    ) -> CoreResult<Identity> {
        self.run(|inner, ports, _| {
            if inner.me.is_some() {
                return Err(ErrorCode::NotAllowed.into());
            }
            let phrase = RecoveryPhrase::parse(&words.join(" ")).map_err(phrase_error)?;
            inner.vault.restore(&phrase).map_err(vault_error)?;
            inner.db.delete(KV_RECOVERY_CHALLENGE)?;
            // TODO(backup): the spec restores these from the encrypted
            // identity backup (docs/design, restore servers); until then the
            // UI asks again. The name comes after, from `updateProfile`.
            let profile = match input {
                Some(input) => Profile {
                    country: input.country,
                    birth_date: input.birth_date,
                    locale: Some(input.locale),
                    ..Profile::default()
                },
                None => Profile::default(),
            };
            let me = inner.new_me(phrase, profile)?;
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
        if crate::invite_link::parse(&link).is_err() {
            return Ok(JoinStatus::Invalid { link });
        }
        let searching = JoinStatus::Searching { link: link.clone() };
        self.ports.events.emit(&Event::JoinStatus(searching.clone()));
        let status = self.try_join(&link)?;
        if matches!(status, JoinStatus::NobodyOnline { .. }) {
            // The inviter may not be reachable yet: keep trying (`retry_joins`).
            let now = Instant::now();
            let retry = PendingJoin {
                next: now + JOIN_RETRY_FIRST,
                wait: JOIN_RETRY_FIRST,
                until: now + JOIN_RETRY_FOR,
            };
            self.pending_joins.lock().expect("not poisoned").insert(link, retry);
        }
        self.ports.events.emit(&Event::JoinStatus(status));
        Ok(searching)
    }

    /// One attempt, for either link format (`crate::invite_link`).
    fn try_join(&self, link: &str) -> CoreResult<JoinStatus> {
        use crate::invite_link::InviteLink;
        match crate::invite_link::parse(link) {
            Ok(InviteLink::V0 { server, invite, secret }) => {
                // A server closed while we were in it: never opened again.
                if self.lock().db.closed_at(server)?.is_some() {
                    return Ok(JoinStatus::Closed { link: link.to_owned() });
                }
                let fetched = self.open_and_fetch(server, &secret);
                self.run(|inner, ports, events| {
                    inner.join(ports, server, invite, &secret, link, fetched, events)
                })
            }
            Ok(InviteLink::V1 { key, server_prefix }) => self.try_join_short(link, &key, &server_prefix),
            Err(_) => Ok(JoinStatus::Invalid { link: link.to_owned() }),
        }
    }

    /// Opens `server` with its secret and fetches what reachable peers hold.
    fn open_and_fetch(&self, server: ServerId, secret: &[u8; 32]) -> Vec<Record> {
        // GAP-4: only the secret lets us ask for the server's logs.
        let general = BTreeSet::from([model::default_channel(server)]);
        self.ports.network.open_server(server, secret, &general);
        // Fetch without the lock, so UI commands don't wait on the network.
        self.ports.network.fetch(server).into_iter().filter(|r| r.server() == server).collect()
    }

    /// T8-02: a short link carries only k and a server-ID prefix. Ask peers
    /// for the envelope under `h`, and try each answer (C3, untrusted):
    /// it must be named `h`, open under k for this prefix, name its own
    /// invite (`names`), and, once the server's control log is in, be
    /// signed by that invite's author (C1). The genesis check against the
    /// full server ID is the join's own. The first answer that joins wins;
    /// a bad one only moves on to the next.
    fn try_join_short(&self, link: &str, key: &[u8; 16], prefix: &[u8; 16]) -> CoreResult<JoinStatus> {
        use crate::crypto::invite::{handle, open, InvitePublication};
        let h = handle(key);
        let mut status = JoinStatus::NobodyOnline { link: link.to_owned() };
        for answer in self.ports.network.fetch_invite(h) {
            let Some(publication) = InvitePublication::decode(&answer) else { continue };
            if *publication.handle() != h {
                continue;
            }
            let Some(opened) = open(key, prefix, publication.envelope()) else { continue };
            if !publication.names(key, &opened) {
                continue;
            }
            let server = opened.server;
            // A server closed while we were in it: never opened again.
            if self.lock().db.closed_at(server)?.is_some() {
                return Ok(JoinStatus::Closed { link: link.to_owned() });
            }
            let fetched = self.open_and_fetch(server, &opened.secret);
            let author = ServerView::build(server, &Log::new(server, fetched.clone()))
                .and_then(|view| view.state.invites.get(&opened.invite).map(|invite| invite.author));
            let joined = if fetched.is_empty() {
                JoinStatus::NobodyOnline { link: link.to_owned() }
            } else if !author.is_some_and(|author| publication.verify(&author)) {
                JoinStatus::Invalid { link: link.to_owned() }
            } else {
                self.run(|inner, ports, events| {
                    inner.join(ports, server, opened.invite, &opened.secret, link, fetched, events)
                })?
            };
            match joined {
                JoinStatus::Invalid { .. } | JoinStatus::NobodyOnline { .. } => {
                    // Opened for this answer only (tech lead): don't leave a
                    // candidate a bad answer named subscribed. A real one is
                    // opened again by the next attempt.
                    self.close_candidate(server)?;
                    if !matches!(status, JoinStatus::Invalid { .. }) {
                        status = joined;
                    }
                }
                _ => return Ok(joined),
            }
        }
        Ok(status)
    }

    /// Closes a server opened only to try a join, unless I'm in it.
    fn close_candidate(&self, server: ServerId) -> CoreResult<()> {
        if !self.lock().db.has_server(server)? {
            self.ports.network.close_server(server);
        }
        Ok(())
    }

    /// Joins that found nobody online, tried again with backoff until they
    /// get an answer, are cancelled, or `JOIN_RETRY_FOR` runs out. "Nobody
    /// online" again isn't re-announced: the UI still shows the first one.
    fn retry_joins(&self) {
        let now = Instant::now();
        let due: Vec<String> = {
            let mut pending = self.pending_joins.lock().expect("not poisoned");
            pending.retain(|_, p| now < p.until);
            pending.iter().filter(|(_, p)| now >= p.next).map(|(link, _)| link.clone()).collect()
        };
        for link in due {
            let status = match self.try_join(&link) {
                Ok(status) => status,
                Err(e) => {
                    eprintln!("mvs-core: retrying a join failed: {:?}", e.code);
                    continue;
                }
            };
            let mut pending = self.pending_joins.lock().expect("not poisoned");
            if matches!(status, JoinStatus::NobodyOnline { .. }) {
                if let Some(p) = pending.get_mut(&link) {
                    p.wait = (p.wait * 2).min(JOIN_RETRY_MAX);
                    p.next = Instant::now() + p.wait;
                }
                continue;
            }
            // Cancelled meanwhile: say nothing.
            if pending.remove(&link).is_some() {
                drop(pending);
                self.ports.events.emit(&Event::JoinStatus(status));
            }
        }
    }

    /// Stops retrying a join that found nobody online (`retry_joins`).
    pub fn cancel_join(&self, link: &str) -> CoreResult<()> {
        self.pending_joins.lock().expect("not poisoned").remove(link.trim());
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

    // ---- server lifecycle (#229, #251) -----------------------------------
    //
    // Every command checks the rollout gate (`update-needed`) before it
    // signs: an older member would refuse the record and stall on every
    // control record after it. Each syncs first, so the gate (and a
    // close's causal past) is as current as the network allows.

    /// `renameServer`: the owner or an admin. The name is trimmed, then must
    /// pass the strict server-name rule (`bad-request`).
    pub fn rename_server(&self, server_id: &str, name: &str) -> CoreResult<ServerSummary> {
        let name = name.trim();
        if !authority::server_name_ok(name) {
            return Err(ErrorCode::BadRequest.into());
        }
        let server = parse_server(server_id)?;
        let _ = self.sync_now();
        self.run(|inner, ports, events| {
            inner.load_mine(server)?;
            inner.lifecycle_role(server, authority::Role::Admin)?;
            inner.lifecycle_gate(server)?;
            inner.publish_control(ports, server, &Action::ServerEdit { name: name.to_owned() })?;
            let summary = inner.summary(server)?;
            events.push(Event::ServerUpdated(summary.clone()));
            Ok(summary)
        })
    }

    /// `leaveServer`: any member but the owner (`not-allowed`; the owner
    /// deletes instead). The `Leave` goes out, so the others re-key; then
    /// the server leaves this device (`server.removed`, `left`). A leaver
    /// isn't banned: a new invite lets them back, as a plain member.
    pub fn leave_server(&self, server_id: &str) -> CoreResult<()> {
        let server = parse_server(server_id)?;
        let _ = self.sync_now();
        self.run(|inner, ports, events| {
            inner.load_mine(server)?;
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
            if inner.view(server)?.state.role(me) == authority::Role::Owner {
                return Err(ErrorCode::NotAllowed.into());
            }
            // Never blocked by the rollout gate (both leads, #261): the
            // signed `Leave` waits in a durable outbox and goes out once the
            // gate opens (`flush_leaves`), retried until a member has it.
            let leave =
                inner.publish_control(ports, server, &Action::Leave { cut: Cut::default() })?;
            inner.db.set_leave(server, &leave)?;
            if inner.lifecycle_gate(server).is_ok() {
                ports.network.publish(&leave);
            }
            // Gone from the UI at once; only the sync secret and the control
            // subscription stay, to deliver the `Leave`.
            inner.purge_server(ports, server, RemovedReason::Left, &[], true, events)
        })?;
        self.flush_leaves();
        Ok(())
    }

    /// The leave outbox, once per sync round: each `Leave` not yet
    /// delivered is checked against what reachable peers hold (without the
    /// lock), then dropped, kept, or (re)published (`Inner::flush_leave`).
    fn flush_leaves(&self) {
        let pending = {
            let inner = self.lock();
            let Ok(leaves) = inner.db.leaves() else { return };
            leaves
                .into_iter()
                .filter_map(|(server, leave)| {
                    let secret = inner.secret(server).ok()??;
                    Some((server, leave, secret))
                })
                .collect::<Vec<_>>()
        };
        for (server, leave, secret) in pending {
            self.ports.network.open_server(server, &secret, &BTreeSet::new());
            let fetched: Vec<Record> = self
                .ports
                .network
                .fetch(server)
                .into_iter()
                .filter(|r| r.server() == server && r.scope() == Scope::Control)
                .collect();
            let _ = self.run(|inner, ports, _| inner.flush_leave(ports, server, &leave, fetched));
        }
    }

    /// `getUpdateNeeded`: the members whose latest profile post lacks
    /// `server-lifecycle-v1` (or who have none), so the UI can say who
    /// blocks a rename or a delete (`update-needed`). Empty: the gate is
    /// open.
    pub fn get_update_needed(&self, server_id: &str) -> CoreResult<Vec<Id>> {
        let server = parse_server(server_id)?;
        self.run(|inner, _, _| {
            inner.load_mine(server)?;
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
            let view = inner.view(server)?;
            let caps = view.lifecycle_caps();
            Ok(view
                .state
                .members
                .iter()
                .filter(|m| **m != me && !view.state.kicked.contains(m))
                .filter(|m| caps.get(m) != Some(&true))
                .map(|m| model::hex_id(m.as_bytes()))
                .collect())
        })
    }

    /// `closeServer`: the owner deletes the server for good. Syncs first,
    /// so recent records are in the close's past (only those stay); then
    /// the close goes out and the server is closed here (`server.closed`,
    /// see `close_locally`), as it is for every member when they read it.
    pub fn close_server(&self, server_id: &str) -> CoreResult<()> {
        let server = parse_server(server_id)?;
        let _ = self.sync_now();
        self.run(|inner, ports, events| {
            inner.load_mine(server)?;
            inner.lifecycle_role(server, authority::Role::Owner)?;
            inner.lifecycle_gate(server)?;
            inner.publish_control(ports, server, &Action::ServerClose)?;
            inner.close_locally(ports, server, events)
        })
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
        self.control_record(server_id, action).map(|_| ())
    }

    /// `control_action`, returning the accepted record's ID.
    fn control_record(&self, server_id: &str, action: Action) -> CoreResult<RecordId> {
        let server = parse_server(server_id)?;
        self.run(|inner, ports, events| {
            inner.load_mine(server)?;
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            if matches!(&action, Action::Kick { target, .. } if *target == me.key()) {
                return Err(ErrorCode::NotAllowed.into());
            }
            let record = inner.publish_control(ports, server, &action)?;
            if matches!(action, Action::Invite { .. } | Action::RevokeInvite { .. }) {
                // The server's link may name another invite now.
                events.push(Event::ServerUpdated(inner.summary(server)?));
                return Ok(record.id());
            }
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
            Ok(record.id())
        })
    }

    // ---- voice ------------------------------------------------------------

    /// Voice: sync's voice transport (piece A). Without one, `joinCall`
    /// answers `not-implemented`.
    pub fn set_voice(&self, net: Arc<dyn VoiceNet>) {
        self.lock().calls.set_net(net);
    }

    /// In a call: `applyUpdate` refuses.
    pub fn in_call(&self) -> bool {
        self.lock().calls.in_call()
    }

    /// `joinCall(roomId)`: leaves any other call first. Calls happen only in
    /// a server's voice room: a text channel answers `not-allowed`.
    pub fn join_call(&self, room_id: &str) -> CoreResult<CallState> {
        let room =
            model::parse_hex32(room_id).map(ChannelId::from_bytes).ok_or(ErrorCode::NotFound)?;
        self.voice(|calls, ctx, inner| {
            let server = match inner.room_server(room)? {
                Some(server) => server,
                None => {
                    inner.channel_server(room)?;
                    return Err(ErrorCode::NotAllowed.into());
                }
            };
            calls.join(ctx, inner, server, room)
        })
    }

    /// `getVoiceRoom`: who is in the server's voice room. `not-found` for a
    /// server I don't have, `not-allowed` if I'm not a member (or kicked).
    pub fn get_voice_room(&self, server_id: &str) -> CoreResult<VoiceRoom> {
        let server = parse_server(server_id)?;
        self.voice(|calls, ctx, inner| {
            if !inner.db.has_server(server)? {
                return Err(ErrorCode::NotFound.into());
            }
            let me = ctx.key.verifying_key().to_bytes();
            let room = voice::room_id(server);
            if !voice::Members::may_call(inner, PublicKey::from_bytes(me), server, room) {
                return Err(ErrorCode::NotAllowed.into());
            }
            Ok(calls.voice_room(ctx, inner, server))
        })
    }

    /// `leaveCall`: idempotent.
    pub fn leave_call(&self) -> CoreResult<()> {
        if !self.in_call() {
            return Ok(());
        }
        self.voice(|calls, ctx, inner| {
            calls.leave(ctx, inner);
            Ok(())
        })
    }

    /// `setMuted`: `not-allowed` outside a call.
    pub fn set_muted(&self, muted: bool) -> CoreResult<()> {
        self.voice(|calls, ctx, inner| calls.set_muted(ctx, inner, muted))
    }

    /// `sendSignal`: to a participant of my call.
    pub fn send_signal(&self, to: &str, kind: SignalKind, payload: String) -> CoreResult<()> {
        let to = parse_user(to)?;
        self.voice(|calls, ctx, _| calls.send_signal(ctx, to, kind, payload))
    }

    /// A blob from sync's voice transport; `from` is the identity sync bound
    /// the sending peer to. Anything that doesn't check out is dropped.
    pub fn voice_received(&self, from: Option<PublicKey>, blob: &[u8]) {
        let _ = self.voice(|calls, ctx, inner| {
            calls.received(ctx, inner, from, blob);
            Ok(())
        });
    }

    /// Hellos and timeouts; the sync loop calls it every round.
    pub fn voice_tick(&self) {
        let _ = self.voice(|calls, ctx, inner| {
            calls.tick(ctx, inner);
            Ok(())
        });
    }

    fn voice<T>(
        &self,
        f: impl FnOnce(&mut voice::Calls, &mut voice::Ctx, &mut Inner) -> CoreResult<T>,
    ) -> CoreResult<T> {
        self.run(|inner, ports, events| {
            let me = inner.me.as_ref().ok_or(ErrorCode::NotFound)?;
            let key = me.identity.signing_key().clone();
            // Out of `inner` while it answers who may call.
            let mut calls = std::mem::take(&mut inner.calls);
            let mut ctx = voice::Ctx { key: &key, now: ports.clock.now_ms(), events };
            let result = f(&mut calls, &mut ctx, inner);
            inner.calls = calls;
            result
        })
    }

    // ---- invites ------------------------------------------------------------

    /// Rust addition (`createInvite`, #208): a new invite, moderator and up.
    /// `expires_ms`: the last time a join through it is accepted, in the
    /// future; none for never. `max_uses`: none or 0 for unlimited.
    pub fn create_invite(
        &self,
        server_id: &str,
        expires_ms: Option<u64>,
        max_uses: Option<u32>,
    ) -> CoreResult<CreatedInvite> {
        if expires_ms.is_some_and(|at| at <= self.ports.clock.now_ms()) {
            return Err(ErrorCode::BadRequest.into());
        }
        let action =
            Action::Invite { expires_ms: expires_ms.unwrap_or(0), max_uses: max_uses.unwrap_or(0) };
        let invite = self.control_record(server_id, action)?;
        let server = parse_server(server_id)?;
        let mut inner = self.lock();
        let secret = inner.secret(server)?.ok_or(ErrorCode::Internal)?;
        let short_link = inner.publish_invite(&self.ports, server, invite, &secret)?;
        Ok(CreatedInvite {
            id: model::hex_id(invite.as_bytes()),
            link: model::invite_link(server, invite, &secret),
            short_link: short_link.as_str().to_owned(),
            expires_at: expires_ms,
            max_uses: max_uses.filter(|n| *n > 0),
        })
    }

    /// Rust addition (`listInvites`, #208): every invite of the server,
    /// oldest first, revoked ones included. Moderator and up.
    pub fn list_invites(&self, server_id: &str) -> CoreResult<Vec<InviteInfo>> {
        let mut inner = self.lock();
        let server = inner.load_mine(parse_server(server_id)?)?;
        let me = inner.me.as_ref().map(Me::key).ok_or(ErrorCode::NotFound)?;
        let view = inner.view(server)?;
        if view.state.role(me) < authority::Role::Moderator {
            return Err(ErrorCode::NotAllowed.into());
        }
        let storage = inner.me.as_ref().map(|me| *me.identity.storage_key().expose_secret());
        let now = self.ports.clock.now_ms();
        let mut out = Vec::new();
        for r in view.records.iter().filter(|r| r.kind() == Kind::Invite) {
            let Some(invite) = view.state.invites.get(&r.id()) else { continue };
            // A short link only while live (S1), and only where its key is.
            let short_link = match (storage, view.live_invite_author(r.id(), now)) {
                (Some(storage), Some(_)) => inner
                    .db
                    .load_invite_key(&storage, server, r.id())?
                    .map(|k| crate::invite_link::encode_v1(&k, &server).as_str().to_owned())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            out.push(InviteInfo {
                id: model::hex_id(r.id().as_bytes()),
                created_by: model::hex_id(invite.author.as_bytes()),
                expires_at: (invite.expires_ms > 0).then_some(invite.expires_ms),
                max_uses: invite.max_uses,
                uses: invite.uses,
                revoked: invite.revoked,
                short_link,
            });
        }
        Ok(out)
    }

    /// Rust addition (`revokeInvite`, #208): its author, or someone ranked
    /// above its author. `not-found` for an unknown or already revoked
    /// invite.
    pub fn revoke_invite(&self, server_id: &str, invite_id: &str) -> CoreResult<()> {
        let invite =
            model::parse_hex32(invite_id).map(RecordId::from_bytes).ok_or(ErrorCode::NotFound)?;
        {
            let mut inner = self.lock();
            let server = inner.load_mine(parse_server(server_id)?)?;
            let view = inner.view(server)?;
            if view.state.invites.get(&invite).is_none_or(|i| i.revoked) {
                return Err(ErrorCode::NotFound.into());
            }
        }
        self.control_action(server_id, Action::RevokeInvite { invite })?;
        // S1: its short link stops being served at once.
        let mut inner = self.lock();
        inner.refresh_invites(&self.ports, parse_server(server_id)?)
    }

    // ---- messages ---------------------------------------------------------
    //
    // TODO(chat): replies, edit, delete, retry; channels other than the
    // default one; auth_deps that always include our latest control heads
    // (today: up to MAX_AUTH_DEPS of the current heads).

    /// Seals `body` for the channel's members and sends it. The first
    /// message of a channel, or the first after a removal, needs a key
    /// rotation: a moderator or above makes one on the way. A plain member
    /// without the key yet (just joined: the moderator's re-key is on its
    /// way) gets the message back at once as `sendState: pending`, with a
    /// provisional `pending-…` ID. The sync loop sends it when the key comes
    /// (`message.updated` carrying `replaces`), or after
    /// [`KEY_PENDING_FOR`] marks it `failed` (`message.updated`, and
    /// `notice: rekey-waiting`). Never a wait on the request thread.
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
        let message = self.run(|inner, ports, events| {
            let (server, channel) = inner.find_channel(channel_id)?;
            let sealed = match inner.seal(ports, server, channel, body) {
                Ok(sealed) => sealed,
                Err(e) if e.code == ErrorCode::NotAllowed && inner.awaiting_key(server)? => {
                    let message = inner.hold_for_key(ports, server, channel, body)?;
                    events.push(Event::MessageAdded(message.clone()));
                    return Ok(message);
                }
                Err(e) => return Err(e),
            };
            let record =
                inner.sign_in_channel(ports, server, channel, None, Kind::Message, sealed)?;
            inner.store(server, record.clone())?;
            ports.network.publish(&record);
            let message = inner.message(&*ports.cipher, server, channel, record.id())?;
            events.push(Event::MessageAdded(message.clone()));
            Ok(message)
        })?;
        if message.send_state == Some(SendState::Pending) && message.id.starts_with(KEY_PENDING_PREFIX) {
            // A round of the background sync now: it brings the re-key in.
            self.ports.network.wake();
        }
        Ok(message)
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

    /// Drops a message held for its key (`pending-…`, pending or failed)
    /// before it was sent: its text is wiped from memory now, and nothing
    /// was ever sent. For "send again" (discard, then a new `sendMessage`)
    /// or dismissing a failed one. `not-found` for any other ID, or one
    /// already sent or discarded.
    pub fn discard_message(&self, message_id: &str) -> CoreResult<()> {
        let mut inner = self.lock();
        let at = inner.key_pending.iter().position(|p| p.id == message_id).ok_or(ErrorCode::NotFound)?;
        inner.key_pending.remove(at);
        Ok(())
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
        // Messages held for their key are listed after the stored ones.
        let held = inner.key_pending.iter().filter(|p| p.channel == channel).count();
        Ok(inner.message_count(&*self.ports.cipher, server, channel)? + held)
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
        // Messages held for their key come after the stored ones.
        let held: Vec<&KeyPending> = inner.key_pending.iter().filter(|p| p.channel == channel).collect();
        let end = end.min(shown.len() + held.len());
        let start = start.min(end);
        let messages = (start..end)
            .map(|index| match shown.get(index) {
                Some(s) => message_json(s, index, channel, me, &Status { net: &*inner.net, db: &inner.db }),
                None => held[index - shown.len()].message(index),
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
            self.voice_tick();
            // A waiting join re-key fires exactly when its token is due.
            let wait = self.next_rekey_in().map_or(SYNC_EVERY, |due| due.min(SYNC_EVERY));
            match hints.recv_timeout(wait) {
                // Several hints at once are one round.
                Ok(()) => while hints.try_recv().is_ok() {},
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(SYNC_EVERY),
            }
        }
    }

    /// How long until the next join re-key token is due on a server whose
    /// joiners wait for one (GAP-8); `None` when nobody waits. The sync loop
    /// wakes then, so the shared re-key fires on time. Never below
    /// [`REKEY_WAKE_FLOOR`], so a re-key that keeps not happening can't spin
    /// the loop into back-to-back syncs.
    pub fn next_rekey_in(&self) -> Option<Duration> {
        let now = self.ports.clock.now_ms();
        let inner = self.lock();
        inner
            .join_waiting
            .iter()
            .filter_map(|server| inner.join_buckets.get(server))
            .map(|bucket| Duration::from_millis(bucket.due().saturating_sub(now)))
            .min()
            .map(|due| due.max(REKEY_WAKE_FLOOR))
    }

    /// Rust addition (`syncNow`): pulls every server's records from the
    /// network and reports what changed. TODO(sync): driven by the sync
    /// crate's push and pull instead of a command.
    pub fn sync_now(&self) -> CoreResult<()> {
        self.retry_joins();
        self.flush_leaves();
        let servers = {
            let mut inner = self.lock();
            if inner.me.is_none() {
                return Ok(());
            }
            // A closed server is no longer synced.
            let mut servers = inner.db.servers()?;
            servers.retain(|s| inner.db.closed_at(*s).ok().flatten().is_none());
            // GAP-4: (re)open each server with its secret and channels first.
            for server in &servers {
                inner.load(*server)?;
                inner.open_sync(&self.ports, *server)?;
            }
            // T8-02: publications members pushed (C1), then serve only what
            // is still live (S1).
            for (server, bytes) in self.ports.network.take_invite_publications() {
                inner.accept_pushed_invite(&self.ports, server, &bytes)?;
            }
            for server in &servers {
                inner.refresh_invites(&self.ports, *server)?;
            }
            servers
        };
        // Fetch without the lock, so UI commands don't wait on the network.
        let fetched: Vec<(ServerId, Vec<Record>)> =
            servers.into_iter().map(|s| (s, self.ports.network.fetch(s))).collect();
        self.run(|inner, ports, events| {
            let Some(me) = inner.me.as_ref() else { return Ok(()) };
            let me = me.key();
            // Closed servers past their 30 days: purged but for their
            // genesis and close.
            inner.expire_closed(ports, events)?;
            for (server, records) in fetched {
                if !inner.db.has_server(server)? {
                    continue;
                }
                inner.load(server)?;
                inner.seed_announced(&*ports.cipher, server)?;
                let name_before = inner.view(server)?.state.server_name.clone();
                let mut changed = false;
                for record in records {
                    if record.server() == server {
                        changed |= inner.store(server, record)?;
                    }
                }
                // Server lifecycle: the owner deleted it; it leaves this device.
                if inner.view(server)?.state.closed.is_some() {
                    inner.close_locally(ports, server, events)?;
                    continue;
                }
                if inner.view(server)?.state.server_name != name_before {
                    events.push(Event::ServerUpdated(inner.summary(server)?));
                }
                // Release N: say this version reads lifecycle records. Never
                // aborts the other servers; the next round retries.
                let _ = inner.post_caps(ports, server);
                let after = inner.view(server)?;
                if after.is_kicked(&me) {
                    inner.join_waiting.remove(&server);
                    // Purges its invite publications and keys too.
                    inner.db.remove_server(server)?;
                    ports.network.drop_server_invites(server);
                    events.push(Event::ServerRemoved {
                        server_id: model::hex_id(server.as_bytes()),
                        reason: None,
                    });
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
                    // A join re-key the token bucket deferred: retried each round.
                    let _ = inner.rekey(ports, server, events);
                } else {
                    // No longer a rotator (demoted): nothing of ours waits.
                    inner.join_waiting.remove(&server);
                }
                // M2: someone came online or went offline (members only).
                if inner.db.has_server(server)? && inner.presence_changed(server)? && !changed {
                    let view = inner.view(server)?;
                    events.push(Event::MembersChanged {
                        server_id: model::hex_id(server.as_bytes()),
                        members: inner.members_json(&view)?,
                    });
                }
            }
            // Messages held for their key: sent now if it came.
            inner.send_key_pending(ports, events)
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
        let relay_only = relay_only_in(&self.lock().db)?;
        Ok(AppInfo { version: self.app_version.clone(), update_channel: channel, relay_only })
    }

    /// `setRelayOnly(on)` (GAP-6r): stored, then applied to the network. On
    /// ends my call first (its media would go direct) and refuses calls
    /// until it's off. Each of my servers gets a `voice.room` with the new
    /// `relayOnly` at the next tick. Needs no identity.
    pub fn set_relay_only(&self, on: bool) -> CoreResult<()> {
        if on {
            self.lock().calls.set_relay_only(true);
            self.leave_call()?;
        }
        self.run(|inner, ports, _| {
            inner.db.set(KV_RELAY_ONLY, &[u8::from(on)])?;
            inner.calls.set_relay_only(on);
            for server in inner.db.servers()? {
                inner.calls.mark_dirty(server);
            }
            ports.network.set_relay_only(on);
            Ok(())
        })
    }

    pub fn set_update_channel(&self, channel: UpdateChannel) -> CoreResult<()> {
        let value: &[u8] = match channel {
            UpdateChannel::Stable => b"stable",
            UpdateChannel::Beta => b"beta",
        };
        self.lock().db.set(KV_UPDATE_CHANNEL, value)?;
        // A new channel is checked now, not in 6 hours.
        if let Some(updates) = &self.updates {
            updates.wake();
        }
        Ok(())
    }

    /// Turns in-app update on (B8-5): `mvs-core --update-root`. Without
    /// it the update commands answer `not-implemented` and no check runs.
    pub fn with_updates(mut self, updates: Updates) -> Core {
        self.updates = Some(Arc::new(updates));
        self
    }

    fn updates(&self) -> CoreResult<&Updates> {
        self.updates.as_deref().ok_or_else(|| ErrorCode::NotImplemented.into())
    }

    fn ui_channel(&self) -> CoreResult<UpdateChannel> {
        Ok(self.get_app_info()?.update_channel)
    }

    fn now_secs(&self) -> u64 {
        self.ports.clock.now_ms() / 1000
    }

    fn emit_update(&self, events: Vec<UpdateEvent>) {
        self.emit(events.into_iter().map(update_event).collect());
    }

    /// `checkForUpdate` (main/core only, A2).
    pub fn check_for_update(&self) -> CoreResult<serde_json::Value> {
        self.updates()?.check(self.ui_channel()?, self.now_secs())
    }

    /// `downloadUpdate` (main/core only): emits `update.ready` once staged.
    pub fn download_update(&self) -> CoreResult<serde_json::Value> {
        let outcome = self.updates()?.download(self.ui_channel()?, self.now_secs())?;
        self.emit_update(outcome.events);
        Ok(outcome.ok)
    }

    /// `applyUpdate` (main/core only): `not-allowed` during a call.
    pub fn apply_update(&self) -> CoreResult<serde_json::Value> {
        self.updates()?.apply(self.ui_channel()?, self.in_call())
    }

    /// `markHealthy` (main only): commits a trial version; `update.installed`
    /// carries the notes in the profile's language.
    pub fn mark_healthy(&self) -> CoreResult<serde_json::Value> {
        let updates = self.updates()?;
        let lang = match self.lock().me.as_ref().and_then(|me| me.profile.locale) {
            Some(Locale::En) => "en",
            _ => "he",
        };
        let outcome = updates.mark_healthy(self.ui_channel()?, lang)?;
        self.emit_update(outcome.events);
        Ok(outcome.ok)
    }

    /// The background check: once the UI is up (or after a fallback wait),
    /// then every 6 h ± 30 min, and at once after a channel switch. Downloads
    /// whatever is available; the apply waits for the user (or next start).
    /// Returns when `Updates::stop` is called; without updates, at once.
    pub fn run_update_loop(&self) {
        let Some(updates) = &self.updates else { return };
        if !updates.wait(crate::update::FIRST_CHECK_AFTER) {
            return;
        }
        loop {
            if let Ok(channel) = self.ui_channel() {
                self.emit_update(updates.background_round(channel, self.now_secs()));
            }
            if !updates.wait(crate::update::next_interval()) {
                return;
            }
        }
    }

    /// Ends `run_update_loop` (stdin closed).
    pub fn stop_updates(&self) {
        if let Some(updates) = &self.updates {
            updates.stop();
        }
    }
}

fn update_event(e: UpdateEvent) -> Event {
    match e {
        UpdateEvent::Ready { version, waiting_for_call } => {
            Event::UpdateReady(UpdateInfo { version: version.to_string(), waiting_for_call })
        }
        UpdateEvent::Installed { version, notes } => {
            Event::UpdateInstalled { version: version.to_string(), notes }
        }
        UpdateEvent::RolledBack { failed, running } => {
            Event::UpdateRolledBack { failed: failed.to_string(), running: running.to_string() }
        }
    }
}

impl voice::Members for Inner {
    /// Room-only: the server's voice room, a server of mine whose genesis
    /// is accepted (its ID checked against it), a member, not kicked. No
    /// 18+ gate: the room isn't a channel record.
    fn may_call(&mut self, who: PublicKey, server: ServerId, channel: ChannelId) -> bool {
        if channel != voice::room_id(server) {
            return false;
        }
        let mine = self.db.servers().is_ok_and(|servers| servers.contains(&server));
        if !mine || self.load(server).is_err() || self.view(server).is_err() {
            return false;
        }
        let state = self.logs[&server].resolver.state();
        state.members.contains(&who) && !state.kicked.contains(&who)
    }

    fn audience(&mut self, server: ServerId) -> Vec<PublicKey> {
        if self.load(server).is_err() {
            return Vec::new();
        }
        let me = self.me.as_ref().map(Me::key);
        let online = self.net.online(server);
        let state = self.logs[&server].resolver.state();
        state
            .members
            .iter()
            .filter(|key| Some(**key) != me && !state.kicked.contains(key))
            .filter(|key| online.as_ref().is_none_or(|online| online.contains(key)))
            .copied()
            .collect()
    }
}

impl Inner {
    /// Whether the online members of `server` changed since last reported
    /// (and remembers the new set). Members only: authority's, not kicked.
    fn presence_changed(&mut self, server: ServerId) -> CoreResult<bool> {
        let view = self.view(server)?;
        let mut reach = self.net.presence(server).unwrap_or_default();
        let member = |key: &PublicKey| view.state.members.contains(key) && !view.is_kicked(key);
        reach.online.retain(member);
        reach.direct.retain(|key| reach.online.contains(key));
        Ok(self.presence_seen.insert(server, reach.clone()) != Some(reach))
    }

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

    /// T8-02: a short link for my new invite. A fresh k (kept sealed, S4),
    /// the envelope sealed under it, the publication signed by me (the
    /// invite's author, C1), stored and served and pushed to members.
    fn publish_invite(
        &mut self,
        ports: &Ports,
        server: ServerId,
        invite: RecordId,
        secret: &[u8; 32],
    ) -> CoreResult<Zeroizing<String>> {
        use crate::crypto::invite::{handle, seal, InvitePublication};
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let mut k = Zeroizing::new([0u8; crate::invite_link::KEY_LEN]);
        OsRng.fill_bytes(k.as_mut());
        let h = handle(&k);
        let envelope = seal(&k, &server, &invite, secret);
        let publication = InvitePublication::sign(me.identity.signing_key(), invite, h, envelope)
            .ok_or(ErrorCode::Internal)?;
        let storage = *me.identity.storage_key().expose_secret();
        self.db.save_invite_key(&storage, server, invite, &k)?;
        let bytes = publication.encode();
        self.db.put_invite_publication(server, invite, &bytes)?;
        ports.network.serve_invite(server, h, &bytes, true);
        Ok(crate::invite_link::encode_v1(&k, &server))
    }

    /// T8-02 (S1): serve the publications of `server`'s live invites, and
    /// drop and forget those whose invite died (revoked, expired, used up,
    /// author kicked). Run on every authority change: each sync round, and
    /// right after a revoke. Serving again is harmless (first wins).
    fn refresh_invites(&mut self, ports: &Ports, server: ServerId) -> CoreResult<()> {
        use crate::crypto::invite::InvitePublication;
        let now = ports.clock.now_ms();
        let held = self.db.invite_publications(server)?;
        // A closed server stays listed for 30 days, but none of its invites
        // is live: nobody can join it.
        let closed = self.db.closed_at(server)?.is_some();
        let view = self.view(server)?;
        let mut dead = Vec::new();
        for (invite, bytes) in held {
            let Some(publication) = InvitePublication::decode(&bytes) else {
                dead.push((invite, None));
                continue;
            };
            if !closed && view.live_invite_author(invite, now).is_some() {
                ports.network.serve_invite(server, *publication.handle(), &bytes, false);
            } else {
                dead.push((invite, Some(*publication.handle())));
            }
        }
        for (invite, handle) in dead {
            if let Some(handle) = handle {
                ports.network.drop_invite(handle);
            }
            self.db.delete_invite(server, invite)?;
        }
        Ok(())
    }

    /// T8-02 (C1): a publication a member pushed, kept and served only if
    /// it is for a live invite of a server I'm in and signed by that
    /// invite's author. Served without pushing it on (no loops); members
    /// that bind later get it from sync.
    fn accept_pushed_invite(&mut self, ports: &Ports, server: ServerId, bytes: &[u8]) -> CoreResult<()> {
        use crate::crypto::invite::InvitePublication;
        if !self.db.has_server(server)? || self.db.closed_at(server)?.is_some() {
            return Ok(());
        }
        let Some(publication) = InvitePublication::decode(bytes) else { return Ok(()) };
        let view = self.view(server)?;
        let Some(author) = view.live_invite_author(publication.invite(), ports.clock.now_ms()) else {
            return Ok(());
        };
        if !publication.verify(&author) {
            return Ok(());
        }
        if self.db.put_invite_publication(server, publication.invite(), bytes)? {
            ports.network.serve_invite(server, *publication.handle(), bytes, false);
        }
        Ok(())
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
    /// Whether a refused seal in `server` only means the key hasn't reached
    /// me yet: I'm a current member of an open server who can't rotate
    /// (`seal`'s `not-allowed` for anyone else is final).
    /// Holds a message until its channel key reaches me (`send_message`),
    /// shown after the channel's stored messages.
    fn hold_for_key(
        &mut self,
        ports: &Ports,
        server: ServerId,
        channel: ChannelId,
        body: &str,
    ) -> CoreResult<Message> {
        let author = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        // Bounded (tech lead): past the cap, the plain refusal.
        if self.key_pending.iter().filter(|p| p.channel == channel && !p.failed).count() >= MAX_KEY_PENDING {
            return Err(ErrorCode::NotAllowed.into());
        }
        let mut tag = [0u8; 8];
        OsRng.fill_bytes(&mut tag);
        let held = KeyPending {
            id: format!("{KEY_PENDING_PREFIX}{}", hex::encode(tag)),
            server,
            channel,
            author,
            body: Zeroizing::new(body.to_owned()),
            created_at: ports.clock.now_ms(),
            failed: false,
        };
        let index = self.message_count(&*ports.cipher, server, channel)?
            + self.key_pending.iter().filter(|p| p.channel == channel).count();
        let message = held.message(index);
        self.key_pending.push(held);
        Ok(message)
    }

    /// The sync loop's part (`sync_now`): each held message whose key has
    /// come is sealed and sent (`message.updated` with `replaces`, then
    /// listed like any other); one still without it after
    /// [`KEY_PENDING_FOR`], or that can no longer be sent (kicked, left,
    /// closed), turns `failed` (`message.updated`, `notice: rekey-waiting`)
    /// and stays listed until a restart.
    fn send_key_pending(&mut self, ports: &Ports, events: &mut Events) -> CoreResult<()> {
        let now = ports.clock.now_ms();
        let mut i = 0;
        while i < self.key_pending.len() {
            if self.key_pending[i].failed {
                i += 1;
                continue;
            }
            let (server, channel) = (self.key_pending[i].server, self.key_pending[i].channel);
            let body = self.key_pending[i].body.clone();
            let waiting = self.db.has_server(server)? && {
                self.load(server)?;
                match self.seal(ports, server, channel, &body) {
                    Ok(sealed) => {
                        let held = self.key_pending.remove(i);
                        let record =
                            self.sign_in_channel(ports, server, channel, None, Kind::Message, sealed)?;
                        self.store(server, record.clone())?;
                        ports.network.publish(&record);
                        let mut message = self.message(&*ports.cipher, server, channel, record.id())?;
                        message.replaces = Some(held.id);
                        events.push(Event::MessageUpdated(message));
                        continue;
                    }
                    Err(e) => e.code == ErrorCode::NotAllowed && self.awaiting_key(server)?,
                }
            };
            let held = &self.key_pending[i];
            let expired = now.saturating_sub(held.created_at) >= KEY_PENDING_FOR.as_millis() as u64;
            if !waiting || expired {
                self.key_pending[i].failed = true;
                let index = self.message_count(&*ports.cipher, server, channel)?
                    + self.key_pending[..i].iter().filter(|p| p.channel == channel).count();
                events.push(Event::MessageUpdated(self.key_pending[i].message(index)));
                events.push(Event::Notice {
                    kind: NoticeKind::RekeyWaiting,
                    server_id: model::hex_id(server.as_bytes()),
                });
            }
            i += 1;
        }
        // Failed ones are bounded too (sec-lead): beyond MAX_KEY_PENDING in
        // a channel, the oldest are dropped, their text with them.
        let channels: BTreeSet<ChannelId> = self.key_pending.iter().map(|p| p.channel).collect();
        for channel in channels {
            let failed = self.key_pending.iter().filter(|p| p.channel == channel && p.failed).count();
            let mut extra = failed.saturating_sub(MAX_KEY_PENDING);
            self.key_pending.retain(|p| {
                let drop = extra > 0 && p.channel == channel && p.failed;
                extra -= usize::from(drop);
                !drop
            });
        }
        Ok(())
    }

    fn awaiting_key(&self, server: ServerId) -> CoreResult<bool> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        let view = self.view(server)?;
        Ok(view.state.is_member(me)
            && !view.is_kicked(&me)
            && view.state.closed.is_none()
            && view.state.role(me) < authority::Role::Moderator)
    }

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
    /// learns it waits for a moderator (`notice: rekey-waiting`). After
    /// joins: re-keyed for the new members as the server's token bucket
    /// allows (`crate::rekey`).
    fn rekey(&mut self, ports: &Ports, server: ServerId, events: &mut Events) -> CoreResult<()> {
        // Waiting only if this round says so: an early return or error (no
        // longer a member, may no longer rotate) never leaves it waiting.
        self.join_waiting.remove(&server);
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
        // Channels a join left stale: re-keyed together, for one token.
        let mut joined = Vec::new();
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
                    joined.push(channel);
                } else if !may_rotate && no_key {
                    // A joiner without a key yet: waiting for a moderator, not ready.
                    let kind = NoticeKind::RekeyWaiting;
                    events.push(Event::Notice { kind, server_id: server_id.clone() });
                }
                continue;
            }
            // A kick, leave or revoke: never waits, spends no token.
            // Fail closed: a rotation that didn't happen is never announced as
            // one, and the channel stays blocked by `RotationRequired` until the
            // next sync or send retries.
            let renewed = may_rotate
                && self.rotate_channel(ports, server, channel, &members, &auth_deps).is_ok();
            let kind =
                if renewed { NoticeKind::EncryptionRenewed } else { NoticeKind::RekeyWaiting };
            events.push(Event::Notice { kind, server_id: server_id.clone() });
        }
        if joined.is_empty() {
            return Ok(());
        }
        // GAP-8 (`rekey`): a token re-keys now; without one, everyone who
        // joined meanwhile shares the re-key fired when the next is due.
        let now = ports.clock.now_ms();
        let bucket = self.join_buckets.entry(server).or_insert_with(|| JoinBucket::new(now));
        if !bucket.take(now) {
            self.join_waiting.insert(server);
            return Ok(());
        }
        for channel in joined {
            // Fail closed, as for a kick: a failed rotation is never announced
            // as one, and the UI learns the channel waits.
            if self.rotate_channel(ports, server, channel, &members, &auth_deps).is_err() {
                let kind = NoticeKind::RekeyWaiting;
                events.push(Event::Notice { kind, server_id: server_id.clone() });
            }
        }
        Ok(())
    }

    /// The server of my default channel `channel_id`; `not-found` for any
    /// other. Loads the server.
    /// The server a channel of mine is in, 18+ ones included (whether I may
    /// use it is the caller's check).
    /// The server of mine whose voice room is `room`, if any.
    fn room_server(&self, room: ChannelId) -> CoreResult<Option<ServerId>> {
        Ok(self.db.servers()?.into_iter().find(|server| voice::room_id(*server) == room))
    }

    fn channel_server(&mut self, channel: ChannelId) -> CoreResult<ServerId> {
        for server in self.db.servers()? {
            self.load(server)?;
            if self.known(server)?.contains(&channel) {
                return Ok(server);
            }
        }
        Err(ErrorCode::NotFound.into())
    }

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

    /// Signs `action` on our heads in loaded `server`, dry-runs it on the
    /// live resolver, then stores and publishes it. Its deps are our heads,
    /// so it joins at once; if `authority` doesn't accept it, it was never
    /// stored or sent (`not-allowed`), and the log is rebuilt from the store
    /// so it leaves no trace.
    // TODO(records B4): `Resolver::would_accept` instead.
    fn publish_control(
        &mut self,
        ports: &Ports,
        server: ServerId,
        action: &Action,
    ) -> CoreResult<Record> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?;
        let view = self.view(server)?;
        // A revoke-invite lists its invite in its deps.
        let extra = match action {
            Action::RevokeInvite { invite } => Some(*invite),
            _ => None,
        };
        let record = authority::sign_control(
            me.identity.signing_key(),
            server,
            view.next_seq(&me.key()),
            view.deps(extra),
            ports.clock.now_ms(),
            action,
        )
        .map_err(|_| ErrorCode::BadRequest)?;
        let log = self.logs.get_mut(&server).ok_or(ErrorCode::Internal)?;
        let joined = log.add(record.clone());
        if !log.accepted(&record.id()) {
            self.logs.remove(&server);
            return Err(ErrorCode::NotAllowed.into());
        }
        if matches!(action, Action::Leave { .. }) {
            // Never stored: it goes to the leave outbox (`leave_server`).
            return Ok(record);
        }
        for record in &joined {
            self.db.put_record(record)?;
        }
        ports.network.publish(&record);
        Ok(record)
    }

    /// One round of the leave outbox for `server` (#261, both leads), on
    /// what reachable peers hold (`fetched`; nothing of it is stored):
    /// - kicked first, or the server closed: the `Leave` is dropped;
    /// - a member has it (B6 `sent` or `received`; on a transport that
    ///   doesn't track that, it took it): done, the entry and the sync
    ///   secret go, and the subscription closes;
    /// - otherwise, once the gate is open, it is (re)published.
    fn flush_leave(
        &mut self,
        ports: &Ports,
        server: ServerId,
        leave: &Record,
        fetched: Vec<Record>,
    ) -> CoreResult<()> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        let finish = |db: &Db| -> CoreResult<()> {
            db.drop_leave(server)?;
            ports.network.close_server(server);
            Ok(())
        };
        let log = Log::new(server, fetched);
        let Some(view) = ServerView::build(server, &log) else { return Ok(()) };
        if view.is_kicked(&me) || view.state.closed.is_some() {
            return finish(&self.db);
        }
        let state = ports.network.send_state(leave.id());
        if matches!(state, Some(SendState::Sent | SendState::Received)) {
            return finish(&self.db);
        }
        if !gate_open(&view, me) || state == Some(SendState::Pending) {
            return Ok(());
        }
        ports.network.publish(leave);
        let took = ports.network.send_state(leave.id()).is_none()
            && ports.network.fetch(server).iter().any(|r| r.id() == leave.id());
        if took {
            return finish(&self.db);
        }
        Ok(())
    }

    /// `not-allowed` below `role` in loaded `server` (before the gate, so a
    /// member learns they can't, not that others must update).
    fn lifecycle_role(&self, server: ServerId, role: authority::Role) -> CoreResult<()> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        if self.view(server)?.state.role(me) < role {
            return Err(ErrorCode::NotAllowed.into());
        }
        Ok(())
    }

    /// Server lifecycle's rollout gate (#229): `update-needed` unless every
    /// other current member's latest profile post carries
    /// `server-lifecycle-v1` (no profile post counts as needing an update).
    /// This version reads them itself. Checked before anything is signed.
    fn lifecycle_gate(&self, server: ServerId) -> CoreResult<()> {
        let me = self.me.as_ref().ok_or(ErrorCode::NotFound)?.key();
        if !gate_open(&self.view(server)?, me) {
            return Err(ErrorCode::UpdateNeeded.into());
        }
        Ok(())
    }

    /// Release N of server lifecycle: my latest profile post in `server`
    /// lacks the cap (posted by an older version), so post it again, once.
    fn post_caps(&mut self, ports: &Ports, server: ServerId) -> CoreResult<()> {
        let Some(me) = self.me.as_ref() else { return Ok(()) };
        let (key, name) = (me.key(), me.profile.display_name.clone());
        let view = self.view(server)?;
        let member = view.state.is_member(key) && !view.state.kicked.contains(&key);
        if !member || view.state.closed.is_some() || view.lifecycle_caps().get(&key) == Some(&true)
        {
            return Ok(());
        }
        self.publish_control(ports, server, &model::profile_action(&name)).map(drop)
    }

    /// Server lifecycle (#229, the security lead's rule 5): after I leave,
    /// or the server is deleted, removes it from this device. My call there
    /// ends first (with a bye); then its records, sync secret, channel keys
    /// and sync subscription go, and every per-server loop forgets it.
    /// `server.removed` follows. Not erasure: other members keep copies.
    ///
    /// `keep`: for a closed server's last purge, its genesis and close.
    /// `leaving`: my `Leave` is in the outbox, so the sync secret and
    /// subscription stay until it is delivered (`flush_leave`).
    #[allow(clippy::too_many_arguments)]
    fn purge_server(
        &mut self,
        ports: &Ports,
        server: ServerId,
        reason: RemovedReason,
        keep: &[RecordId],
        leaving: bool,
        events: &mut Events,
    ) -> CoreResult<()> {
        self.stop_using(ports, server, !leaving, events);
        self.db.purge_server(server, keep, leaving)?;
        self.logs.remove(&server);
        self.announced.remove(&server);
        events.push(Event::ServerRemoved {
            server_id: model::hex_id(server.as_bytes()),
            reason: Some(reason),
        });
        Ok(())
    }

    /// Server lifecycle: the owner closed `server` (read, or my own close).
    /// It isn't purged at once (the Tech Lead's #229 answer): any call there
    /// ends, its keys and sync subscription go, and it's no longer synced;
    /// it stays listed, read-only (`closed`), for [`CLOSED_RETENTION_MS`]
    /// after we received the close. Then the rest goes, and its genesis and
    /// close stay forever, so it reads as closed, never unknown.
    fn close_locally(
        &mut self,
        ports: &Ports,
        server: ServerId,
        events: &mut Events,
    ) -> CoreResult<()> {
        self.stop_using(ports, server, true, events);
        self.db.mark_closed(server, ports.clock.now_ms())?;
        events.push(Event::ServerClosed { server_id: model::hex_id(server.as_bytes()) });
        Ok(())
    }

    /// Closed servers whose 30 days are up: purged but for their genesis
    /// and close (`server.removed`, `closed`).
    fn expire_closed(&mut self, ports: &Ports, events: &mut Events) -> CoreResult<()> {
        let now = ports.clock.now_ms();
        for (server, at) in self.db.closed_servers()? {
            if now.saturating_sub(at) < CLOSED_RETENTION_MS {
                continue;
            }
            self.load(server)?;
            let log = self.logs.get(&server).ok_or(ErrorCode::Internal)?;
            let genesis = log.records.iter().find(|r| r.kind() == Kind::Genesis).map(Record::id);
            let close = log.resolver.state().closed;
            let keep: Vec<RecordId> = genesis.into_iter().chain(close).collect();
            self.purge_server(ports, server, RemovedReason::Closed, &keep, false, events)?;
        }
        Ok(())
    }

    /// What every way out of a server shares: my call there ends (with a
    /// bye), and its keys, sync subscription (unless `close_sync` is false:
    /// a `Leave` still to deliver) and per-server state go, and its short
    /// links stop being served (T8-02; the rows go with `Db::purge_server`).
    fn stop_using(
        &mut self,
        ports: &Ports,
        server: ServerId,
        close_sync: bool,
        events: &mut Events,
    ) {
        if self.calls.in_server(server) {
            if let Some(key) = self.me.as_ref().map(|me| me.identity.signing_key().clone()) {
                let mut calls = std::mem::take(&mut self.calls);
                let mut ctx = voice::Ctx { key: &key, now: ports.clock.now_ms(), events };
                calls.leave(&mut ctx, self);
                self.calls = calls;
            }
        }
        self.calls.forget_server(server);
        ports.cipher.forget(server);
        if close_sync {
            ports.network.close_server(server);
        }
        ports.network.drop_server_invites(server);
        self.presence_seen.remove(&server);
        self.keyed_for.retain(|(s, _), _| *s != server);
        // Its join re-key tokens and any re-key waiting on them (#250).
        self.join_buckets.remove(&server);
        self.join_waiting.remove(&server);
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
        // M2: online from sync's presence; only authority's members are
        // listed, so a bound non-member or a kicked key never shows.
        let reach = self.net.presence(view.id).unwrap_or_default();
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
                presence: if Some(member.key) == me || reach.online.contains(&member.key) {
                    Presence::Online
                } else {
                    Presence::Offline
                },
                connection: (Some(member.key) != me && reach.online.contains(&member.key)).then(
                    || match reach.direct.contains(&member.key) {
                        true => Connection::Direct,
                        false => Connection::Relay,
                    },
                ),
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
        // Server lifecycle: an old invite (or a stale peer) for a server its
        // owner closed. Its genesis and close say so: closed, not unknown.
        if view.state.closed.is_some() {
            return Ok(JoinStatus::Closed { link });
        }
        let key = me.key();
        // Already a member (this device lost its copy, e.g. a restore on a
        // new computer): no new join, and nothing that would overwrite what
        // the server already holds about me.
        let member = view.state.is_member(key);
        let required = view.genesis.age_restriction;
        let age = model::age_on(&me.profile.birth_date, ports.clock.now_ms());
        // A member's earlier age claim stands while this device has no birth
        // date (a restore doesn't carry it yet); a known age too low still
        // blocks.
        let age_unknown_member = member && age.is_none();
        if required.min_age() > 0
            && !age_unknown_member
            && age.is_none_or(|age| age < required.min_age())
        {
            return Ok(JoinStatus::AgeBlocked { link, required });
        }
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
        let mut mine = Vec::new();
        if !member {
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
        // A member without a name here (a restore) keeps the profile the
        // server holds: an empty one would replace my name for everyone.
        let post_profile = !(member && me.profile.display_name.trim().is_empty());
        let profile = post_profile
            .then(|| sign(&view, view.deps(None), &model::profile_action(&me.profile.display_name)))
            .transpose()?;
        for record in log.records.iter().chain(&log.channel) {
            self.db.put_record(record)?;
        }
        self.logs.insert(server, log);
        if let Some(profile) = profile {
            mine.push(profile.clone());
            self.store(server, profile)?;
        }
        // Likewise a member without a birth date keeps its earlier claim:
        // a claim from no date would say "not adult", which counts at once.
        if !age_unknown_member {
            let claim = self.sign_age_claim(ports, server)?;
            mine.push(claim.clone());
            self.store(server, claim)?;
        }
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
        // The latest accepted rename, else the genesis name.
        let name = self.state.server_name.clone().unwrap_or_else(|| self.genesis.name.clone());
        ServerSummary {
            initials: model::initials(&name),
            name,
            color: model::color_for(self.id.as_bytes()),
            age_restriction: self.genesis.age_restriction,
            owner_id: model::hex_id(self.owner.as_bytes()),
            invite_link,
            unread: false,
            mentions: 0,
            closed: self.state.closed.is_some(),
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
        replaces: None,
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

#[cfg(test)]
mod relay_only_tests {
    use super::{stored_relay_only, Db, KV_RELAY_ONLY};

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mvs-relayonly-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Security lead (#272): stored_relay_only fails closed.
    #[test]
    fn the_stored_setting_fails_closed() {
        let dir = dir("unit");
        let db = dir.join("core.db");
        // A fresh data dir: no setting yet, so off.
        assert!(!stored_relay_only(&db));
        // Off and on as stored; anything else is on.
        for (value, on) in [(&[0u8][..], false), (&[1], true), (&[2], true), (b"", true)] {
            Db::open(Some(&db)).unwrap().set(KV_RELAY_ONLY, value).unwrap();
            assert_eq!(stored_relay_only(&db), on, "{value:?}");
        }
        // Can't be opened (its folder is missing) or isn't a database: on.
        assert!(stored_relay_only(&dir.join("missing").join("core.db")));
        let junk = dir.join("junk.db");
        std::fs::write(&junk, vec![0x5a; 4096]).unwrap();
        assert!(stored_relay_only(&junk));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
