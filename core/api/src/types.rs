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
    /// Rust addition: malformed command JSON or arguments.
    BadRequest,
    /// Rust addition: in the contract, not done by this core yet.
    NotImplemented,
    /// Rust addition: storage or keychain failure. Never carries details.
    Internal,
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
    #[serde(rename = "server.removed", rename_all = "camelCase")]
    ServerRemoved { server_id: Id },
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
