//! The JSON boundary every shell uses (Tauri command, Electron addon, IPC):
//!
//! ```text
//! request:  { "cmd": "createServer", "args": { "name": "...", "ageRestriction": "none" } }
//! response: { "ok": <result> }  or  { "error": { "code": "not-allowed", "positions"?: [...] } }
//! event:    { "event": "server.added", "payload": <ServerSummary> }   (via EventSink)
//! ```
//!
//! `args` holds the TS method's parameters by name (`listChannels(serverId)`
//! is `{ "serverId": ... }`); commands without parameters take `{}` or no
//! `args` at all. A `void` result is `{ "ok": null }`.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::service::Core;
use crate::types::*;

#[derive(Deserialize)]
struct Request {
    cmd: String,
    #[serde(default)]
    args: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerArg {
    server_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelIdArg {
    channel_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RangeArg {
    channel_id: String,
    start: usize,
    end: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendArg {
    channel_id: String,
    body: String,
    #[serde(default)]
    reply_to_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageIdArg {
    message_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditArg {
    message_id: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerUserArgs {
    server_id: String,
    user_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelInput {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    adult: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChannelArgs {
    server_id: String,
    input: ChannelInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateChannelArgs {
    channel_id: String,
    input: ChannelInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantArgs {
    server_id: String,
    user_id: String,
    role: Role,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateServerArgs {
    input: CreateServerInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateServerInput {
    name: String,
    age_restriction: AgeRestriction,
}

#[derive(Deserialize)]
struct InputArg<T> {
    input: T,
}

#[derive(Deserialize)]
struct AnswersArg {
    answers: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct WordsArg {
    words: Vec<String>,
}

#[derive(Deserialize)]
struct LinkArg {
    link: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteInput {
    expires_ms: Option<u64>,
    max_uses: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteArgs {
    server_id: String,
    input: Option<CreateInviteInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeInviteArgs {
    server_id: String,
    invite_id: String,
}

#[derive(Deserialize)]
struct MutedArg {
    muted: bool,
}

#[derive(Deserialize)]
struct SignalArgs {
    to: String,
    kind: SignalKind,
    payload: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserArg {
    user_id: String,
}

#[derive(Deserialize)]
struct BytesArg {
    bytes: u64,
}

#[derive(Deserialize)]
struct ChannelArg {
    channel: UpdateChannel,
}

fn args<T: DeserializeOwned>(value: Value) -> CoreResult<T> {
    serde_json::from_value(value).map_err(|_| ErrorCode::BadRequest.into())
}

fn ok<T: serde::Serialize>(value: CoreResult<T>) -> CoreResult<Value> {
    value.map(|v| serde_json::to_value(v).expect("result types serialize"))
}

/// Every command in `CoreCommands`, plus the Rust additions. Commands from
/// the contract this core doesn't do yet answer `not-implemented`.
pub const NOT_YET: &[&str] = &[];

impl Core {
    /// Takes a request JSON text, returns the response JSON text.
    pub fn handle(&self, request: &str) -> String {
        let response = match serde_json::from_str::<Request>(request) {
            Ok(request) => self.handle_value(&request.cmd, request.args),
            Err(_) => Err(ErrorCode::BadRequest.into()),
        };
        match response {
            Ok(value) => json!({ "ok": value }),
            Err(error) => json!({ "error": error }),
        }
        .to_string()
    }

    pub fn handle_value(&self, cmd: &str, raw: Value) -> CoreResult<Value> {
        let raw = if raw.is_null() { json!({}) } else { raw };
        match cmd {
            "getIdentity" => ok(self.get_identity()),
            "createIdentity" => {
                let a: InputArg<NewIdentityInput> = args(raw)?;
                ok(self.create_identity(a.input))
            }
            "checkRecoveryWords" => {
                let a: AnswersArg = args(raw)?;
                let mut answers = BTreeMap::new();
                for (position, word) in a.answers {
                    let position = position.parse().map_err(|_| ErrorCode::BadRequest)?;
                    answers.insert(position, word);
                }
                ok(self.check_recovery_words(answers))
            }
            "getRecoveryChallenge" => ok(self.get_recovery_challenge()),
            "getRecoveryWords" => ok(self.get_recovery_words()),
            "restoreIdentity" => ok(self.restore_identity(args::<WordsArg>(raw)?.words)),
            "updateProfile" => ok(self.update_profile(args::<InputArg<ProfileInput>>(raw)?.input)),

            "listServers" => ok(self.list_servers()),
            "listChannels" => ok(self.list_channels(&args::<ServerArg>(raw)?.server_id)),
            "getMessageCount" => ok(self.get_message_count(&args::<ChannelIdArg>(raw)?.channel_id)),
            "getMessages" => {
                let a: RangeArg = args(raw)?;
                ok(self.get_messages(&a.channel_id, a.start, a.end))
            }
            "sendMessage" => {
                let a: SendArg = args(raw)?;
                ok(self.send_message(&a.channel_id, &a.body, a.reply_to_id.as_deref()))
            }
            "editMessage" => {
                let a: EditArg = args(raw)?;
                ok(self.edit_message(&a.message_id, &a.body))
            }
            "deleteMessage" => ok(self.delete_message(&args::<MessageIdArg>(raw)?.message_id)),
            "retryMessage" => ok(self.retry_message(&args::<MessageIdArg>(raw)?.message_id)),
            "listMembers" => ok(self.list_members(&args::<ServerArg>(raw)?.server_id)),
            "createServer" => {
                let a: CreateServerArgs = args(raw)?;
                ok(self.create_server(&a.input.name, a.input.age_restriction))
            }
            "joinServer" => ok(self.join_server(&args::<LinkArg>(raw)?.link)),
            "cancelJoin" => ok(self.cancel_join(&args::<LinkArg>(raw)?.link)),

            "markRead" => Ok(Value::Null),
            "blockUser" => ok(self.block_user(&args::<UserArg>(raw)?.user_id)),
            "kickMember" => {
                let a: ServerUserArgs = args(raw)?;
                ok(self.kick_member(&a.server_id, &a.user_id))
            }
            "undoKick" => {
                let a: ServerUserArgs = args(raw)?;
                ok(self.undo_kick(&a.server_id, &a.user_id))
            }
            "createChannel" => {
                let a: CreateChannelArgs = args(raw)?;
                let name = a.input.name.ok_or(ErrorCode::BadRequest)?;
                ok(self.create_channel(&a.server_id, &name, a.input.adult.unwrap_or(false)))
            }
            "updateChannel" => {
                let a: UpdateChannelArgs = args(raw)?;
                ok(self.update_channel(&a.channel_id, a.input.name.as_deref(), a.input.adult))
            }
            "deleteChannel" => ok(self.delete_channel(&args::<ChannelIdArg>(raw)?.channel_id)),
            "listRemovedMembers" => {
                ok(self.list_removed_members(&args::<ServerArg>(raw)?.server_id))
            }
            "createInvite" => {
                let a: CreateInviteArgs = args(raw)?;
                let input = a.input.unwrap_or_default();
                ok(self.create_invite(&a.server_id, input.expires_ms, input.max_uses))
            }
            "listInvites" => ok(self.list_invites(&args::<ServerArg>(raw)?.server_id)),
            "revokeInvite" => {
                let a: RevokeInviteArgs = args(raw)?;
                ok(self.revoke_invite(&a.server_id, &a.invite_id))
            }

            "getStorageUsage" => ok(self.get_storage_usage()),
            "setStorageLimit" => ok(self.set_storage_limit(args::<BytesArg>(raw)?.bytes)),
            "clearCache" => ok(self.clear_cache()),
            "exportDiagnostics" => ok(self.export_diagnostics()),
            "getAppInfo" => ok(self.get_app_info()),
            "setUpdateChannel" => ok(self.set_update_channel(args::<ChannelArg>(raw)?.channel)),
            // The shell restarts; nothing for the core to do.
            "restartToUpdate" => Ok(Value::Null),

            // Rust additions.
            "grantRole" => {
                let a: GrantArgs = args(raw)?;
                ok(self.grant_role(&a.server_id, &a.user_id, a.role))
            }
            "revokeRole" => {
                let a: ServerUserArgs = args(raw)?;
                ok(self.revoke_role(&a.server_id, &a.user_id))
            }
            "syncNow" => ok(self.sync_now()),

            // Voice (piece B). `joinCall` takes the room ID as `channelId`.
            "getVoiceRoom" => ok(self.get_voice_room(&args::<ServerArg>(raw)?.server_id)),
            "joinCall" => ok(self.join_call(&args::<ChannelIdArg>(raw)?.channel_id)),
            "leaveCall" => ok(self.leave_call()),
            "setMuted" => ok(self.set_muted(args::<MutedArg>(raw)?.muted)),
            "sendSignal" => {
                let a: SignalArgs = args(raw)?;
                ok(self.send_signal(&a.to, a.kind, a.payload))
            }
            // In-app update (B8-5). Main/core only: never in CoreCommands or
            // the renderer allow-list (A2). `not-implemented` without
            // `--update-root`.
            "checkForUpdate" => self.check_for_update(),
            "downloadUpdate" => self.download_update(),
            "applyUpdate" => self.apply_update(),
            "markHealthy" => self.mark_healthy(),

            cmd if NOT_YET.contains(&cmd) => Err(ErrorCode::NotImplemented.into()),
            _ => Err(ErrorCode::BadRequest.into()),
        }
    }
}
