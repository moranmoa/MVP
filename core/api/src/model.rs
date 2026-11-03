//! A server as the records describe it: derived IDs, the genesis and join
//! bodies, and members from `authority`'s resolved state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use authority::{Action, Resolver, State};
use records::{ChannelId, Kind, PublicKey, Record, RecordId, Scope, ServerId};
use serde::{Deserialize, Serialize};

use crate::types::{AgeRestriction, Role};

// BLAKE3 derive_key contexts. Frozen for the alpha.
const DEFAULT_CHANNEL_CTX: &str = "mvs 2026-11 default channel id v1";
const SHORT_CODE_CTX: &str = "mvs 2026-11 short code v1";

/// Join posts start with this, so ordinary posts are never read as joins.
const JOIN_TAG: &str = "mvs/join/v1";
pub(crate) const INVITE_PREFIX: &str = "mvs://join/";
pub(crate) const MAX_NAME_CHARS: usize = 100;

const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub(crate) const COLORS: [&str; 8] =
    ["#5865f2", "#2f9e44", "#e8590c", "#d6336c", "#1098ad", "#7048e8", "#f08c00", "#0ca678"];

pub(crate) fn default_channel(server: ServerId) -> ChannelId {
    ChannelId::from_bytes(blake3::derive_key(DEFAULT_CHANNEL_CTX, server.as_bytes()))
}

/// "ABCD-EFGH": 40 bits of a hash of the key, shown next to names.
pub(crate) fn short_code(key: &PublicKey) -> String {
    let hash = blake3::derive_key(SHORT_CODE_CTX, key.as_bytes());
    let bits = u64::from_be_bytes(hash[..8].try_into().expect("8 bytes"));
    let mut out = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            out.push('-');
        }
        out.push(CODE_ALPHABET[((bits >> (59 - 5 * i)) & 31) as usize] as char);
    }
    out
}

pub(crate) fn color_for(bytes: &[u8; 32]) -> String {
    COLORS[usize::from(bytes[31]) % COLORS.len()].to_owned()
}

/// Up to two letters: first two of one word, or the first of two words.
pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    match words.as_slice() {
        [] => "?".to_owned(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first.chars().take(1).chain(second.chars().take(1)).collect(),
    }
}

pub(crate) fn hex_id(bytes: &[u8; 32]) -> String {
    hex::encode(bytes)
}

pub(crate) fn parse_hex32(id: &str) -> Option<[u8; 32]> {
    hex::decode(id).ok()?.try_into().ok()
}

/// Server metadata in the genesis body, after the nonce that `records`
/// checks against the server ID. `authority` doesn't read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenesisBody {
    pub v: u8,
    pub name: String,
    pub age_restriction: AgeRestriction,
}

// ---- Joins ------------------------------------------------------------------
//
// TODO(records B4, GAP-3): `Kind::Join` and a members set in `State`. Until
// then a control-log `Post` tagged `mvs/join/v1` is borrowed. These two
// functions are the only place that knows; swap them when B4 lands.
// TODO(groupkeys): the display name is readable by anyone holding the
// records (relays too) until control-log content is encrypted.

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JoinBody {
    mvs: String,
    display_name: String,
}

/// The control action a member signs on joining.
pub(crate) fn join_action(display_name: &str) -> Action {
    let body = JoinBody { mvs: JOIN_TAG.to_owned(), display_name: display_name.to_owned() };
    Action::Post { text: serde_json::to_string(&body).expect("plain struct") }
}

/// The display name, if `record` is an accepted-kind join.
fn parse_join(record: &Record) -> Option<String> {
    let Some(Action::Post { text }) = Action::from_record(record) else { return None };
    let body: JoinBody = serde_json::from_str(&text).ok()?;
    (body.mvs == JOIN_TAG).then_some(body.display_name)
}

/// One server's records as held locally, with its resolver kept up to date
/// (one `insert` per record, never a full re-resolve per action).
pub(crate) struct Log {
    pub records: Vec<Record>,
    pub resolver: Resolver,
    ids: HashSet<RecordId>,
}

impl Log {
    pub fn new(server: ServerId, records: impl IntoIterator<Item = Record>) -> Log {
        let mut log =
            Log { records: Vec::new(), resolver: Resolver::new(server), ids: HashSet::new() };
        for record in records {
            log.add(record);
        }
        log
    }

    /// Adds a record; false if already held.
    pub fn add(&mut self, record: Record) -> bool {
        if !self.ids.insert(record.id()) {
            return false;
        }
        // Pending (deps missing) and evicted records stay in `records` and
        // are re-offered to a fresh resolver on the next load.
        let _ = self.resolver.insert(record.clone());
        self.records.push(record);
        true
    }

    pub fn accepted(&self, id: &RecordId) -> bool {
        self.resolver.state().accepted.contains(id)
    }
}

pub(crate) struct MemberInfo {
    pub key: PublicKey,
    pub role: Role,
    pub display_name: String,
}

/// One server, as its [`Log`] resolves it.
pub(crate) struct ServerView<'a> {
    pub id: ServerId,
    pub genesis: GenesisBody,
    pub owner: PublicKey,
    pub state: &'a State,
    pub records: &'a [Record],
}

impl<'a> ServerView<'a> {
    /// `None` without an accepted genesis for `id`. (`records` refuses a
    /// genesis whose author and nonce don't give its server ID.)
    pub fn build(id: ServerId, log: &'a Log) -> Option<ServerView<'a>> {
        let state = log.resolver.state();
        let genesis = log.records.iter().find(|r| {
            r.kind() == Kind::Genesis && r.scope() == Scope::Control && r.server() == id
        })?;
        let metadata = genesis.body().get(records::GENESIS_NONCE_LEN..)?;
        let body: GenesisBody = serde_json::from_slice(metadata).ok()?;
        let owner = genesis.author();
        (body.v == 1 && state.owner == Some(owner)).then_some(ServerView {
            id,
            genesis: body,
            owner,
            state,
            records: &log.records,
        })
    }

    pub fn is_kicked(&self, key: &PublicKey) -> bool {
        self.state.kicked.contains(key)
    }

    /// Owner, anyone holding a role, and anyone whose join post was
    /// accepted; minus the kicked. Ordered by role, then key.
    pub fn members(&self) -> Vec<MemberInfo> {
        let by_id: HashMap<RecordId, &Record> = self.records.iter().map(|r| (r.id(), r)).collect();
        let mut names: BTreeMap<PublicKey, String> = BTreeMap::new();
        for id in &self.state.posts {
            let Some(record) = by_id.get(id) else { continue };
            if let Some(name) = parse_join(record) {
                // Later joins (a new display name) win.
                names.insert(record.author(), name);
            }
        }
        let mut keys: BTreeSet<PublicKey> = names.keys().copied().collect();
        keys.insert(self.owner);
        keys.extend(self.state.roles.keys().copied());
        let mut members: Vec<MemberInfo> = keys
            .into_iter()
            .filter(|key| !self.is_kicked(key))
            .map(|key| MemberInfo {
                key,
                role: self.state.role(key).into(),
                display_name: names.get(&key).cloned().unwrap_or_default(),
            })
            .collect();
        members.sort_by_key(|m| (std::cmp::Reverse(m.role.to_authority()), m.key));
        members
    }

    /// Control records no other control record depends on.
    pub fn heads(&self) -> Vec<RecordId> {
        let control: Vec<&Record> =
            self.records.iter().filter(|r| r.scope() == Scope::Control).collect();
        let referenced: BTreeSet<RecordId> =
            control.iter().flat_map(|r| r.deps().iter().copied()).collect();
        control.iter().map(|r| r.id()).filter(|id| !referenced.contains(id)).collect()
    }

    pub fn next_seq(&self, author: &PublicKey) -> u64 {
        self.records
            .iter()
            .filter(|r| r.author() == *author)
            .map(|r| r.seq().saturating_add(1))
            .max()
            .unwrap_or(0)
    }
}

/// Whole years between a `YYYY-MM-DD` birth date and `now_ms`.
pub(crate) fn age_on(birth_date: &str, now_ms: u64) -> Option<u32> {
    let mut parts = birth_date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    let (ny, nm, nd) = civil_from_days((now_ms / 86_400_000) as i64);
    let mut age = ny - y;
    if (nm, nd) < (m, d) {
        age -= 1;
    }
    u32::try_from(age).ok()
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_760), (2026, 11, 3));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn ages() {
        let nov_3_2026 = 20_760 * 86_400_000;
        assert_eq!(age_on("2008-11-03", nov_3_2026), Some(18));
        assert_eq!(age_on("2008-11-04", nov_3_2026), Some(17));
        assert_eq!(age_on("bad", nov_3_2026), None);
        assert_eq!(age_on("2030-01-01", nov_3_2026), None);
    }

    #[test]
    fn short_codes_look_like_the_mock() {
        let code = short_code(&PublicKey::from_bytes([7; 32]));
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(code.bytes().all(|b| b == b'-' || CODE_ALPHABET.contains(&b)));
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("קהילת הקוד"), "קה");
        assert_eq!(initials("Rust"), "Ru");
        assert_eq!(initials("  "), "?");
    }
}
