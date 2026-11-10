//! JSON shapes, mirroring `ui/src/core/types.ts` field for field (camelCase).
//! Fields marked "Rust addition" are not in the TS types yet; the TS side
//! ignores unknown fields, so they are safe to send.

use serde::{Deserialize, Serialize};

pub type Id = String;
/// Milliseconds since the Unix epoch.
pub type Timestamp = u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub id: Id,
    pub short_code: String,
    pub display_name: String,
    pub avatar_color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_data_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    pub country: String,
    pub birth_date: String,
    pub recovery_checked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    He,
    En,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewIdentityInput {
    pub locale: Locale,
    pub country: String,
    pub birth_date: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInput {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_data_url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgeRestriction {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "16+")]
    Sixteen,
    #[serde(rename = "18+")]
    Eighteen,
}

impl AgeRestriction {
    pub fn min_age(self) -> u32 {
        match self {
            AgeRestriction::None => 0,
            AgeRestriction::Sixteen => 16,
            AgeRestriction::Eighteen => 18,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSummary {
    pub id: Id,
    pub name: String,
    pub initials: String,
    pub color: String,
    pub age_restriction: AgeRestriction,
    pub owner_id: Id,
    pub invite_link: String,
    pub unread: bool,
    pub mentions: u32,
    /// Server lifecycle: the owner closed it. It stays listed, read-only,
    /// for 30 days after the close was received (additive).
    #[serde(default)]
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: Id,
    pub server_id: Id,
    pub name: String,
    pub adult: bool,
    pub unread: bool,
    pub mentions: u32,
    /// #general, the first channel (#150): it can't be deleted.
    pub is_default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SendState {
    Pending,
    Sent,
    Received,
    Failed,
}

/// `Message` in types.ts. TODO: replies, edits, deletes, mentions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Id,
    pub channel_id: Id,
    /// Position in the channel as shown, oldest first.
    pub index: usize,
    pub author_id: Id,
    pub body: String,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<Timestamp>,
    /// A tombstone: the message keeps its place, its body is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    /// Only on messages written on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_state: Option<SendState>,
    /// On the `message.updated` that sends a message held for its key: the
    /// provisional ID (`pending-…`) it replaces in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRange {
    pub channel_id: Id,
    pub start: usize,
    pub messages: Vec<Message>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Offline,
}

/// Rust addition: a member's role, from `authority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Moderator,
    Member,
}

impl From<authority::Role> for Role {
    fn from(role: authority::Role) -> Role {
        match role {
            authority::Role::Owner => Role::Owner,
            authority::Role::Admin => Role::Admin,
            authority::Role::Moderator => Role::Moderator,
            authority::Role::Member => Role::Member,
        }
    }
}

impl Role {
    pub(crate) fn to_authority(self) -> authority::Role {
        match self {
            Role::Owner => authority::Role::Owner,
            Role::Admin => authority::Role::Admin,
            Role::Moderator => authority::Role::Moderator,
            Role::Member => authority::Role::Member,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub id: Id,
    pub server_id: Id,
    pub display_name: String,
    pub short_code: String,
    pub avatar_color: String,
    pub presence: Presence,
    /// #187: how we reach this member while online: `direct` (LAN) or
    /// `relay`; `null` for us and for members who are offline.
    pub connection: Option<Connection>,
    pub is_owner: bool,
    /// Rust addition.
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
}

/// Every command rejects with this shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreError {
    pub code: ErrorCode,
    /// 1-based word positions that were not recognised (restore).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positions: Option<Vec<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    InvalidWords,
    NotAllowed,
    NotFound,
    RecoveryCheckRequired,
    /// OS authentication was cancelled (the app lock). TODO(app lock).
    AuthCancelled,
    /// The recovery-word challenge is burned (B19 §6).
    TooManyAttempts,
    /// Another channel of the server has this name (B11).
    NameTaken,
    /// The OS keychain (Secret Service on Linux) is missing or unusable
    /// (QA-20): the core runs, but nothing that needs the identity works.
    KeyringUnavailable,
    /// Voice: the call already has 5 participants (the mesh cap).
    CallFull,
    /// Voice: relay-only is on, so calls are off.
    RelayOnly,
    /// Voice: my own identity is already in this room from another
    /// computer (same recovery words). One computer per call until linked
    /// devices (phase 3).
    InCallElsewhere,
    /// Server lifecycle: a member still runs a version that can't read a
    /// rename, leave or delete (no `server-lifecycle-v1` cap); nothing was
    /// signed.
    UpdateNeeded,
    /// Rust addition: malformed command JSON or arguments.
    BadRequest,
    /// Rust addition: in the contract, not done by this core yet.
    NotImplemented,
    /// Rust addition: storage or keychain failure. Never carries details.
    Internal,
    /// In-app update (B8): the update host can't be reached.
    Offline,
    /// In-app update: any verification failure; details only in the log.
    InvalidUpdate,
    /// In-app update: a manifest issued in the future, or all expired.
    CheckClock,
    NothingToDownload,
    NothingStaged,
    DiskFull,
}

impl CoreError {
    pub fn new(code: ErrorCode) -> CoreError {
        CoreError { code, positions: None }
    }
}

impl From<ErrorCode> for CoreError {
    fn from(code: ErrorCode) -> CoreError {
        CoreError::new(code)
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap_or_default())
    }
}

impl std::error::Error for CoreError {}

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum JoinStatus {
    Searching {
        link: String,
    },
    NobodyOnline {
        link: String,
    },
    #[serde(rename_all = "camelCase")]
    Joined {
        link: String,
        server_id: Id,
    },
    Invalid {
        link: String,
    },
    AgeBlocked {
        link: String,
        required: AgeRestriction,
    },
    /// Server lifecycle: the server was closed by its owner (an old invite,
    /// or one a stale peer still serves).
    Closed {
        link: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub messages_bytes: u64,
    pub files_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub waiting_for_call: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Stable,
    Beta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub update_channel: UpdateChannel,
    /// The relay-only setting (`setRelayOnly`): peers are reached only
    /// through the relay, and calls are off.
    #[serde(default)]
    pub relay_only: bool,
}

/// `getRecoveryChallenge`'s result: 1-based positions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryChallenge {
    pub positions: Vec<usize>,
}

/// `RecoveryCheckResult`: `{ ok: true }` or `{ ok: false, wrong: [...] }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCheckResult {
    pub ok: bool,
    /// 1-based positions answered wrong; absent when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrong: Option<Vec<usize>>,
}

impl RecoveryCheckResult {
    pub fn passed() -> RecoveryCheckResult {
        RecoveryCheckResult { ok: true, wrong: None }
    }

    pub fn wrong(positions: Vec<usize>) -> RecoveryCheckResult {
        RecoveryCheckResult { ok: false, wrong: Some(positions) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedIdentity {
    pub identity: Identity,
    pub words: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgressStep {
    Keys,
    Storage,
    Encryption,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeKind {
    EncryptionRenewed,
    /// Rust addition: removed members still hold a channel key and only a
    /// moderator can renew it; the channel can't be written to until then.
    RekeyWaiting,
}

/// `CoreEvents` in api.ts, as `{ "event": name, "payload": ... }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
pub enum Event {
    #[serde(rename = "identity.progress")]
    IdentityProgress { step: ProgressStep, percent: u8 },
    #[serde(rename = "identity.changed")]
    IdentityChanged(Identity),
    #[serde(rename = "server.added")]
    ServerAdded(ServerSummary),
    #[serde(rename = "server.updated")]
    ServerUpdated(ServerSummary),
    /// I'm no longer in the server. `reason` (additive): `left` or
    /// `closed`; absent for a kick (as before).
    /// Server lifecycle: the owner closed the server (read, or my own
    /// `closeServer`). It stays listed, read-only, until its 30 days are up;
    /// then `server.removed` (`closed`).
    #[serde(rename = "server.closed", rename_all = "camelCase")]
    ServerClosed { server_id: Id },
    #[serde(rename = "server.removed", rename_all = "camelCase")]
    ServerRemoved {
        server_id: Id,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        reason: Option<RemovedReason>,
    },
    #[serde(rename = "channel.added")]
    ChannelAdded(Channel),
    #[serde(rename = "channel.removed", rename_all = "camelCase")]
    ChannelRemoved { server_id: Id, channel_id: Id, name: String },
    #[serde(rename = "channel.updated")]
    ChannelUpdated(Channel),
    #[serde(rename = "members.changed", rename_all = "camelCase")]
    MembersChanged { server_id: Id, members: Vec<Member> },
    #[serde(rename = "join.status")]
    JoinStatus(JoinStatus),
    #[serde(rename = "message.updated")]
    MessageUpdated(Message),
    #[serde(rename = "message.added")]
    MessageAdded(Message),
    #[serde(rename = "update.ready")]
    UpdateReady(UpdateInfo),
    /// Voice: my call changed (join, leave, mute, timeout). After a leave,
    /// or a refusal by a full call, its participants are empty.
    #[serde(rename = "call.state")]
    CallState(CallState),
    /// Voice: WebRTC signalling from a participant of my call. Offers and
    /// answers come only verified (see `voice`), so the flag is always true.
    #[serde(rename = "call.signal", rename_all = "camelCase")]
    CallSignal { from: Id, kind: SignalKind, payload: String, fingerprint_verified: bool },
    /// Voice: who is in a server's room changed (join, leave, mute,
    /// timeout, kick), mine or not. At most one per room per sync round,
    /// only while I'm a member. A hint: `getVoiceRoom` re-reads it.
    #[serde(rename = "voice.room")]
    VoiceRoom(VoiceRoom),
    /// Once, after `markHealthy` commits a new version. Text only.
    #[serde(rename = "update.installed")]
    UpdateInstalled { version: String, notes: Option<String> },
    /// Once, on the first healthy start after the launcher rolled back.
    #[serde(rename = "update.rolledBack")]
    UpdateRolledBack { failed: String, running: String },
    #[serde(rename = "notice", rename_all = "camelCase")]
    Notice { kind: NoticeKind, server_id: Id },
}

/// Rust addition (`listRemovedMembers`): an account the owner may let back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedMember {
    pub id: Id,
    pub short_code: String,
    /// Its last display name, if it ever posted one.
    pub display_name: String,
    /// When it was kicked (the kick's claimed time).
    pub removed_at: Timestamp,
}

/// Rust addition (`createInvite`, #208): the new invite and its link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedInvite {
    pub id: Id,
    pub link: String,
    /// T8-02: the short v1 link (`mvs://join/` + at most 51 base58
    /// characters). Additive: `link` stays v0 until release N+1 switches
    /// creation to v1 (design §7).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub short_link: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
}

/// Rust addition (`listInvites`, #208): one invite of a server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteInfo {
    pub id: Id,
    /// Who made it (a user ID).
    pub created_by: Id,
    /// The last time a join through it is accepted; absent for never.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// 0 for unlimited.
    pub max_uses: u32,
    pub uses: u32,
    pub revoked: bool,
    /// T8-02: its short v1 link, for an invite I made here that is still
    /// live (its key is kept only on the inviter's device); absent otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub short_link: String,
}

/// Voice: a call, me first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallState {
    pub channel_id: Id,
    pub participants: Vec<CallParticipant>,
    /// 5 participants: nobody else can join.
    pub full: bool,
    /// Relay-only is on: calls are off. An ended call (no participants)
    /// with this set ended because relay-only went on.
    #[serde(default)]
    pub relay_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallParticipant {
    /// A user ID.
    pub identity: Id,
    pub muted: bool,
}

/// Why `server.removed` fired (server lifecycle).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemovedReason {
    /// I left (`leaveServer`).
    Left,
    /// The owner closed (deleted) it, mine or read; sent when its 30 days
    /// are up and the rest of it is purged.
    Closed,
}

/// Voice: a server's one voice room and who is in it, from verified hellos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRoom {
    pub server_id: Id,
    /// What `joinCall` takes; `CallState.channelId` in a call.
    pub room_id: Id,
    /// At most 5: me first when I'm in it, then the others.
    pub participants: Vec<VoiceParticipant>,
    /// Relay-only is on: `joinCall` answers `relay-only`, so the UI can
    /// disable "Join" up front. A new `voice.room` follows each change.
    #[serde(default)]
    pub relay_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceParticipant {
    pub user_id: Id,
    /// A display hint only.
    pub muted: bool,
}

/// Voice: the WebRTC signalling `sendSignal` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalKind {
    Offer,
    Answer,
    Ice,
}

/// #187: a direct connection (LAN, later hole-punched) or through the relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    Direct,
    Relay,
}
