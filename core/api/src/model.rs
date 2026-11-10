//! A server as the records describe it: derived IDs, the genesis and join
//! bodies, and members from `authority`'s resolved state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use authority::{Action, Inserted, Resolver, State};
use records::{ChannelId, Kind, PublicKey, Record, RecordId, Scope, ServerId, Stub, MAX_DEPS};
use serde::{Deserialize, Serialize};

use crate::types::{AgeRestriction, ErrorCode, ProfileInput, Role};

// BLAKE3 derive_key contexts. Frozen for the alpha.
const DEFAULT_CHANNEL_CTX: &str = "mvs 2026-11 default channel id v1";
const SHORT_CODE_CTX: &str = "mvs 2026-11 short code v1";

/// Profile posts carry this tag, so ordinary posts are never read as one.
const PROFILE_TAG: &str = "mvs/profile/v1";
pub(crate) const MAX_NAME_CHARS: usize = 100;

const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub(crate) const COLORS: [&str; 8] =
    ["#5865f2", "#2f9e44", "#e8590c", "#d6336c", "#1098ad", "#7048e8", "#f08c00", "#0ca678"];

/// #general's name for a server from before #113, which has no record of it.
pub(crate) const GENERAL: &str = "general";

/// #general's name as created, in the creator's language. Stored in the
/// record, so it doesn't change with anyone's UI language later (UX-04).
pub(crate) fn general_name(locale: Option<crate::types::Locale>) -> &'static str {
    match locale {
        Some(crate::types::Locale::He) => "כללי",
        Some(crate::types::Locale::En) | None => GENERAL,
    }
}

/// A channel name as stored (B11): spaces become `-`, runs of `-` collapse,
/// at most 32 code points; `None` unless authority would accept it
/// (`channel_name_ok`: no bidi, control or zero-width characters).
pub(crate) fn channel_name(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = if c.is_whitespace() { '-' } else { c };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    let cut: String = out.chars().take(authority::MAX_CHANNEL_NAME).collect();
    let out = cut.trim_matches('-').to_owned();
    authority::channel_name_ok(&out).then_some(out)
}

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

// Hard caps on what the UI may hand in. The UI cleans and clips for display
// (ui/src/core/sanitize.ts: 32 graphemes, 300-char bio); these only stop
// oversize or unsafe input from being stored and sent.
const PROFILE_NAME_MAX_CHARS: usize = 128;
const BIO_MAX_CHARS: usize = 1500;
const BIO_MAX_LINES: usize = 8;
/// Same as the UI's `AVATAR_DATA_URL_MAX`: about 512 KB of image.
const AVATAR_DATA_URL_MAX: usize = 700_000;
const AVATAR_TYPES: [&str; 4] = ["png", "jpeg", "webp", "gif"];

/// `bad-request` unless the profile fits the caps above, and the avatar is
/// a base64 `data:` URL of an allow-listed raster type (never SVG).
pub(crate) fn check_profile(input: &ProfileInput) -> Result<(), ErrorCode> {
    let name = input.display_name.trim();
    let bio_ok = input.bio.as_deref().is_none_or(|bio| {
        bio.chars().count() <= BIO_MAX_CHARS && bio.lines().count() <= BIO_MAX_LINES
    });
    let avatar_ok = input.avatar_data_url.as_deref().is_none_or(is_safe_avatar);
    // B23: no bidi, control or zero-width characters in a name others see.
    let ok = !name.is_empty()
        && name.chars().count() <= PROFILE_NAME_MAX_CHARS
        && authority::display_text_ok(name)
        && bio_ok
        && avatar_ok;
    ok.then_some(()).ok_or(ErrorCode::BadRequest)
}

fn is_safe_avatar(url: &str) -> bool {
    if url.len() > AVATAR_DATA_URL_MAX {
        return false;
    }
    let Some((kind, data)) =
        url.strip_prefix("data:image/").and_then(|rest| rest.split_once(";base64,"))
    else {
        return false;
    };
    let body = data.trim_end_matches('=');
    AVATAR_TYPES.contains(&kind)
        && !body.is_empty()
        && data.len() - body.len() <= 2
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
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

// ---- Invites and profiles ---------------------------------------------------
//
// Membership is `authority`'s: a `Join` naming an invite (B4, #46). The
// display name goes in a control-log `Post` tagged `mvs/profile/v1`, signed
// right after the join (posts need a member).
// TODO(groupkeys): the display name is readable by anyone holding the
// records (relays too) until control-log content is encrypted.
// `updateProfile` with a new name re-posts it to every server I'm in.

/// A v0 link, `mvs://join/<server>/<invite>/<secret>`, all 64 hex
/// (`crate::invite_link`, the one parser). The secret (GAP-4) is a bearer
/// credential: never logged, only copied by the user.
pub(crate) fn invite_link(server: ServerId, invite: RecordId, secret: &[u8; 32]) -> String {
    crate::invite_link::encode_v0(&server, &invite, secret).as_str().to_owned()
}

/// The first invite a new server gets: no expiry, no use limit.
/// TODO(ui): invites with expiry and use limits, and revoking them.
pub(crate) const FIRST_INVITE: Action = Action::Invite { expires_ms: 0, max_uses: 0 };

/// Server lifecycle (#229): what a profile post says this version can read.
/// Old versions ignore the field (`ProfileBody` takes unknown fields).
pub(crate) const SERVER_LIFECYCLE_CAP: &str = "server-lifecycle-v1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBody {
    mvs: String,
    display_name: String,
    /// Absent before server lifecycle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    caps: Vec<String>,
}

/// The post a member signs to tell the server their display name, and
/// what this version can read.
pub(crate) fn profile_action(display_name: &str) -> Action {
    let body = ProfileBody {
        mvs: PROFILE_TAG.to_owned(),
        display_name: display_name.to_owned(),
        caps: vec![SERVER_LIFECYCLE_CAP.to_owned()],
    };
    Action::Post { text: serde_json::to_string(&body).expect("plain struct") }
}

/// The profile, if `record` is a profile post.
fn parse_profile(record: &Record) -> Option<ProfileBody> {
    let Some(Action::Post { text }) = Action::from_record(record) else { return None };
    let body: ProfileBody = serde_json::from_str(&text).ok()?;
    (body.mvs == PROFILE_TAG).then_some(body)
}

/// One server's records as held locally, with its resolver kept up to date
/// (one `insert` per record, never a full re-resolve per action).
///
/// `records` holds only what joined the resolver's log, in join order: only
/// those are stored, offered as heads or read for members. Records waiting
/// on deps are mirrored in `pending`, which the resolver bounds: what it
/// evicts is dropped here too (sync keeps its own pending buffer and
/// redelivers).
pub(crate) struct Log {
    pub records: Vec<Record>,
    pub resolver: Resolver,
    joined: HashSet<RecordId>,
    pending: HashMap<RecordId, (Record, usize)>,
    /// Missing dep -> pending records waiting on it.
    waiting: HashMap<RecordId, Vec<RecordId>>,
    /// This server's channel records, in the order they arrived. TODO:
    /// bound them (per-channel windows, as sync pages).
    pub channel: Vec<Record>,
    /// Channel records pruned to their stubs (B6-p): no body kept.
    pub stubs: HashMap<RecordId, Stub>,
    server: ServerId,
    channel_ids: HashSet<RecordId>,
}

impl Log {
    pub fn new(server: ServerId, records: impl IntoIterator<Item = Record>) -> Log {
        Log::with_resolver(server, Resolver::new(server), records)
    }

    fn with_resolver(
        server: ServerId,
        resolver: Resolver,
        records: impl IntoIterator<Item = Record>,
    ) -> Log {
        let mut log = Log {
            records: Vec::new(),
            resolver,
            joined: HashSet::new(),
            pending: HashMap::new(),
            waiting: HashMap::new(),
            channel: Vec::new(),
            stubs: HashMap::new(),
            server,
            channel_ids: HashSet::new(),
        };
        for record in records {
            log.add(record);
        }
        log
    }

    /// Offers a record; returns the records that joined because of it (it,
    /// and any pending ones it unblocked), for the caller to store.
    pub fn add(&mut self, record: Record) -> Vec<Record> {
        // TODO(sync adapter, #46): rate-limit incoming joins per invite and
        // author before they get here. A join refused on a limited invite
        // makes authority re-resolve in full (a CPU lever).
        let id = record.id();
        match self.resolver.insert(record.clone()) {
            // Not a control record of ours: keep this server's channel
            // records (authority judges them on read, see `chat`).
            Inserted::Ignored => {
                let ours = record.server() == self.server
                    && matches!(record.scope(), Scope::Channel(_))
                    && self.channel_ids.insert(id);
                if ours {
                    self.channel.push(record.clone());
                    vec![record]
                } else {
                    Vec::new()
                }
            }
            Inserted::Pending { evicted } => {
                if let Some(old) = evicted {
                    self.forget(&old.id());
                }
                let missing: Vec<RecordId> =
                    record.deps().iter().filter(|d| !self.joined.contains(*d)).copied().collect();
                for dep in &missing {
                    self.waiting.entry(*dep).or_default().push(id);
                }
                self.pending.insert(id, (record, missing.len()));
                Vec::new()
            }
            Inserted::Joined => {
                let mut out = Vec::new();
                let mut ready = vec![record];
                while let Some(record) = ready.pop() {
                    let id = record.id();
                    self.joined.insert(id);
                    for waiter in self.waiting.remove(&id).unwrap_or_default() {
                        let Some(entry) = self.pending.get_mut(&waiter) else { continue };
                        entry.1 -= 1;
                        if entry.1 == 0 {
                            ready.push(self.pending.remove(&waiter).expect("just seen").0);
                        }
                    }
                    self.records.push(record.clone());
                    out.push(record);
                }
                out
            }
        }
    }

    /// Drops a pending record the resolver evicted.
    fn forget(&mut self, id: &RecordId) {
        let Some((record, _)) = self.pending.remove(id) else { return };
        for dep in record.deps() {
            if let Some(waiters) = self.waiting.get_mut(dep) {
                waiters.retain(|w| w != id);
                if waiters.is_empty() {
                    self.waiting.remove(dep);
                }
            }
        }
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.pending.len()
    }

    /// A stub from the store (a record pruned earlier).
    pub fn add_stub(&mut self, stub: Stub) {
        if stub.server() == self.server && self.channel_ids.insert(stub.id()) {
            self.stubs.insert(stub.id(), stub);
        }
    }

    /// Replaces a held full record by its stub (B6-p); the stub, for the
    /// store and the network, or `None` if not held or not prunable.
    pub fn prune(&mut self, id: RecordId) -> Option<Stub> {
        let at = self.channel.iter().position(|r| r.id() == id)?;
        let stub = self.channel[at].prune()?;
        self.channel.remove(at);
        self.stubs.insert(id, stub.clone());
        Some(stub)
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

    /// `authority`'s member set, with names from profile posts. Ordered by
    /// role, then key.
    /// Each author's latest display name from their profile posts. A name
    /// with bidi, control or zero-width characters (B23) isn't shown.
    pub fn display_names(&self) -> BTreeMap<PublicKey, String> {
        let by_id: HashMap<RecordId, &Record> = self.records.iter().map(|r| (r.id(), r)).collect();
        let mut names: BTreeMap<PublicKey, String> = BTreeMap::new();
        for id in &self.state.posts {
            let Some(record) = by_id.get(id) else { continue };
            let name = parse_profile(record).map(|p| p.display_name);
            if let Some(name) = name.filter(|n| authority::display_text_ok(n)) {
                // Later profile posts (a new display name) win.
                names.insert(record.author(), name);
            }
        }
        names
    }

    /// Server lifecycle's rollout gate (#229): whether each author's latest
    /// profile post carries [`SERVER_LIFECYCLE_CAP`]. Members with no
    /// profile post are absent (they count as needing an update).
    pub fn lifecycle_caps(&self) -> BTreeMap<PublicKey, bool> {
        let by_id: HashMap<RecordId, &Record> = self.records.iter().map(|r| (r.id(), r)).collect();
        let mut caps = BTreeMap::new();
        for id in &self.state.posts {
            let Some(record) = by_id.get(id) else { continue };
            if let Some(profile) = parse_profile(record) {
                let ready = profile.caps.iter().any(|c| c == SERVER_LIFECYCLE_CAP);
                caps.insert(record.author(), ready);
            }
        }
        caps
    }

    pub fn members(&self) -> Vec<MemberInfo> {
        let names = self.display_names();
        let mut members: Vec<MemberInfo> = self
            .state
            .members
            .iter()
            .copied()
            .map(|key| MemberInfo {
                key,
                role: self.state.role(key).into(),
                display_name: names.get(&key).cloned().unwrap_or_default(),
            })
            .collect();
        members.sort_by_key(|m| (std::cmp::Reverse(m.role.to_authority()), m.key));
        members
    }

    /// Deps for a record signed now: joined control records no other joined
    /// one depends on (the latest in join order), plus `extra` (a join's or
    /// revoke-invite's invite). At most [`MAX_DEPS`], sorted as `records`
    /// requires, so the record joins at once.
    pub fn deps(&self, extra: Option<RecordId>) -> Vec<RecordId> {
        let control: Vec<&Record> =
            self.records.iter().filter(|r| r.scope() == Scope::Control).collect();
        let referenced: BTreeSet<RecordId> =
            control.iter().flat_map(|r| r.deps().iter().copied()).collect();
        let heads: Vec<RecordId> = control
            .iter()
            .map(|r| r.id())
            .filter(|id| !referenced.contains(id) && Some(*id) != extra)
            .collect();
        let room = MAX_DEPS - usize::from(extra.is_some());
        let mut deps: Vec<RecordId> = heads[heads.len().saturating_sub(room)..].to_vec();
        deps.extend(extra);
        deps.sort();
        deps
    }

    /// T8-02 (C1, S1): the author of invite `id` if a join through it would
    /// be accepted at `now_ms` (authority's rule: not revoked, author not
    /// kicked, uses left, not expired); `None` once it is dead.
    pub fn live_invite_author(&self, id: RecordId, now_ms: u64) -> Option<PublicKey> {
        let invite = self.state.invites.get(&id)?;
        let live = !invite.revoked
            && !self.is_kicked(&invite.author)
            && (invite.max_uses == 0 || invite.uses < invite.max_uses)
            && (invite.expires_ms == 0 || now_ms <= invite.expires_ms);
        live.then_some(invite.author)
    }

    /// An invite to put in this server's link: one that still lets people
    /// join, mine first, then the oldest.
    pub fn invite(&self, me: Option<PublicKey>) -> Option<RecordId> {
        let usable: Vec<(RecordId, PublicKey)> = self
            .records
            .iter()
            .filter(|r| r.kind() == Kind::Invite)
            .filter_map(|r| {
                let invite = self.state.invites.get(&r.id())?;
                let open = !invite.revoked
                    && !self.is_kicked(&invite.author)
                    && (invite.max_uses == 0 || invite.uses < invite.max_uses);
                // TODO(ui): compare `expires_ms` with now once invites expire.
                (open && invite.expires_ms == 0).then_some((r.id(), invite.author))
            })
            .collect();
        usable.iter().find(|(_, author)| Some(*author) == me).or(usable.first()).map(|(id, _)| *id)
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

// ---- G-11: channel order ---------------------------------------------------
//
// Tech Lead's design (side notes, g11-channel-order-design.md): a control-log
// `Post` tagged `mvs/channel-order/v1`, like profile posts, so every version
// accepts it and older ones ignore it. Body:
// `{"mvs":"mvs/channel-order/v1","v":1,"pos":{"<channel hex>":"<key>"}}`.
// Keys are fractional indexes: base-62 digits (`0-9A-Za-z`, ASCII order),
// compared as strings, 1..=32 long, never ending in `0`.

const CHANNEL_ORDER_TAG: &str = "mvs/channel-order/v1";
/// The longest position key; a longer one means a rebalance.
pub(crate) const POSITION_MAX: usize = 32;
const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

#[derive(Serialize, Deserialize)]
struct ChannelOrderBody {
    mvs: String,
    v: u32,
    pos: BTreeMap<String, String>,
}

pub(crate) fn channel_order_action(pos: &BTreeMap<ChannelId, String>) -> Action {
    let body = ChannelOrderBody {
        mvs: CHANNEL_ORDER_TAG.to_owned(),
        v: 1,
        pos: pos.iter().map(|(id, key)| (hex_id(id.as_bytes()), key.clone())).collect(),
    };
    Action::Post { text: serde_json::to_string(&body).expect("plain struct") }
}

/// A well-formed position key.
pub(crate) fn valid_position(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= POSITION_MAX
        && key.bytes().all(|b| DIGITS.contains(&b))
        && !key.ends_with('0')
}

fn digit(b: u8) -> u32 {
    DIGITS.iter().position(|d| *d == b).expect("checked digit") as u32
}

/// A key strictly between `lo` and `hi` (`None`: no bound), both valid and
/// `lo < hi`. Never ends in `0`, so there's always room before it.
pub(crate) fn key_between(lo: Option<&str>, hi: Option<&str>) -> String {
    let lo = lo.unwrap_or("").as_bytes();
    let mut hi = hi.map(str::as_bytes);
    let mut out = Vec::new();
    for i in 0.. {
        let l = lo.get(i).map_or(0, |b| digit(*b));
        let h = hi.map_or(62, |hi| hi.get(i).map_or(0, |b| digit(*b)));
        if h > l + 1 {
            out.push(DIGITS[((l + h) / 2) as usize]);
            break;
        }
        out.push(DIGITS[l as usize]);
        if h > l {
            // Below `hi` from here on: any larger tail fits.
            hi = None;
        }
    }
    String::from_utf8(out).expect("ascii")
}

/// `n` evenly spread keys, short (a rebalance).
pub(crate) fn spread_keys(n: usize) -> Vec<String> {
    let mut width = 1u32;
    while 62u128.pow(width) <= (n as u128 + 1) * 4 {
        width += 1;
    }
    let span = 62u128.pow(width);
    (1..=n as u128)
        .map(|i| {
            let mut value = span * i / (n as u128 + 1);
            let mut digits = vec![b'0'; width as usize];
            for slot in digits.iter_mut().rev() {
                *slot = DIGITS[(value % 62) as usize];
                value /= 62;
            }
            let key = String::from_utf8(digits).expect("ascii");
            key.trim_end_matches('0').to_owned()
        })
        .collect()
}

impl ServerView<'_> {
    /// G-11: each live channel's position key. Channel-order posts count in
    /// authority's linear order, each only if its author was owner or admin
    /// at that record: in the state of the post's causal past (its deps and
    /// theirs). Per channel, the last counting post wins. Deleted and
    /// unknown channels, and bad keys, are ignored.
    ///
    /// Cheap against spam (Security Lead on #323): a post whose author was
    /// never owner or admin in this server is skipped before anything is
    /// parsed or resolved, and each resolved post's answer is remembered.
    pub fn channel_positions(&self) -> BTreeMap<ChannelId, String> {
        let ever_admin = self.ever_admin();
        let by_id: HashMap<RecordId, &Record> = self.records.iter().map(|r| (r.id(), r)).collect();
        let mut out = BTreeMap::new();
        for id in &self.state.posts {
            let Some(record) = by_id.get(id) else { continue };
            if !ever_admin.contains(&record.author()) {
                continue;
            }
            let Some(Action::Post { text }) = Action::from_record(record) else { continue };
            let Ok(body) = serde_json::from_str::<ChannelOrderBody>(&text) else { continue };
            if body.mvs != CHANNEL_ORDER_TAG || body.v != 1 {
                continue;
            }
            if !self.admin_at(record, &by_id) {
                continue;
            }
            for (channel, key) in body.pos {
                let Some(channel) = parse_hex32(&channel).map(ChannelId::from_bytes) else { continue };
                if valid_position(&key) && self.state.channels.contains_key(&channel) {
                    out.insert(channel, key);
                }
            }
        }
        out
    }

    /// Everyone who was ever owner or admin here: the owner, and the target
    /// of any accepted grant of admin or owner. One pass, no resolving.
    fn ever_admin(&self) -> HashSet<PublicKey> {
        let mut out = HashSet::from([self.owner]);
        for record in self.records.iter().filter(|r| r.kind() == Kind::Grant) {
            if let Some(Action::Grant { target, role }) = Action::from_record(record) {
                if role >= authority::Role::Admin {
                    out.insert(target);
                }
            }
        }
        out
    }

    /// `record`'s author is owner or admin in the state of its causal past.
    /// The answer depends only on the record (its ID fixes its deps, and so
    /// its past), so it's remembered by ID.
    fn admin_at(&self, record: &Record, by_id: &HashMap<RecordId, &Record>) -> bool {
        if let Some(known) = admin_cache().lock().expect("not poisoned").get(&record.id()) {
            return *known;
        }
        let mut past: Vec<Record> = Vec::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<RecordId> = record.deps().to_vec();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(r) = by_id.get(&id) {
                stack.extend(r.deps().iter().copied());
                past.push((*r).clone());
            }
        }
        let state = authority::resolve(self.id, &past);
        let admin = state.role(record.author()) >= authority::Role::Admin;
        let mut cache = admin_cache().lock().expect("not poisoned");
        if cache.len() >= ADMIN_CACHE_MAX {
            cache.clear();
        }
        cache.insert(record.id(), admin);
        admin
    }
}

/// `admin_at` answers by record ID, at most this many (then it starts over).
const ADMIN_CACHE_MAX: usize = 4096;

fn admin_cache() -> &'static std::sync::Mutex<HashMap<RecordId, bool>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<RecordId, bool>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// G-11: 1,000 moves into the same gap keep every key valid and at most
    /// 32 characters (a rebalance when the next one would pass it), and the
    /// order always holds.
    #[test]
    fn position_keys_stay_short_and_ordered() {
        let mut keys = spread_keys(4);
        assert!(keys.windows(2).all(|w| w[0] < w[1]) && keys.iter().all(|k| valid_position(k)));
        let mut rebalances = 0;
        for _ in 0..1_000 {
            // Right after the first key, before the second, again and again.
            let key = key_between(Some(&keys[0]), Some(&keys[1]));
            if key.len() > POSITION_MAX {
                keys = spread_keys(keys.len());
                rebalances += 1;
            } else {
                assert!(keys[0] < key && key < keys[1], "{} < {key} < {}", keys[0], keys[1]);
                keys.insert(1, key);
                keys.truncate(4);
            }
            assert!(keys.iter().all(|k| valid_position(k)), "{keys:?}");
            assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
        }
        assert!(rebalances > 0, "never needed a rebalance");
        // Before the first and after the last, too.
        assert!(key_between(None, Some("1")).as_str() < "1");
        assert!("z" < key_between(Some("z"), None).as_str());
        assert!(!valid_position("") && !valid_position("a0") && !valid_position("a-b"));
        assert!(!valid_position(&"a".repeat(33)));
    }

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
    fn log_keeps_only_joined_records_and_bounds_pending() {
        let owner = records::SigningKey::from_bytes(&[1; 32]);
        let genesis = authority::sign_genesis(&owner, [0; 16], 1, b"{}").unwrap();
        let server = genesis.server();
        let post = |seq: u64, deps: Vec<RecordId>| {
            authority::sign_control(
                &owner,
                server,
                seq,
                deps,
                1,
                &Action::Post { text: "x".into() },
            )
            .unwrap()
        };
        let mut log =
            Log::with_resolver(server, Resolver::with_pending_limit(server, 2), [genesis.clone()]);

        // Three orphans, room for two: the oldest is dropped.
        let orphans: Vec<Record> =
            (1..=3).map(|i| post(i, vec![RecordId::from_bytes([i as u8; 32])])).collect();
        for orphan in &orphans {
            assert!(log.add(orphan.clone()).is_empty());
        }
        assert_eq!(log.pending(), 2);
        assert_eq!(log.records.len(), 1);

        // A record waiting on a pending one joins once that one does.
        let parent = post(10, vec![genesis.id()]);
        let child = post(11, vec![parent.id()]);
        assert!(log.add(child.clone()).is_empty());
        let joined: Vec<RecordId> = log.add(parent.clone()).iter().map(Record::id).collect();
        assert_eq!(joined, [parent.id(), child.id()]);
        assert_eq!(log.records.len(), 3);
        assert!(log.add(child).is_empty(), "duplicate");
    }

    #[test]
    fn heads_skip_pending_and_are_capped() {
        let owner = records::SigningKey::from_bytes(&[2; 32]);
        let genesis = authority::sign_genesis(&owner, [0; 16], 1, b"{}").unwrap();
        let server = genesis.server();
        let mut log = Log::new(server, [genesis.clone()]);
        for seq in 1..=(MAX_DEPS as u64 + 5) {
            let deps = vec![genesis.id()];
            let record = authority::sign_control(
                &owner,
                server,
                seq,
                deps,
                1,
                &Action::Post { text: "x".into() },
            )
            .unwrap();
            log.add(record);
        }
        let orphan = authority::sign_control(
            &owner,
            server,
            999,
            vec![RecordId::from_bytes([9; 32])],
            1,
            &Action::Post { text: "x".into() },
        )
        .unwrap();
        log.add(orphan.clone());
        let view = ServerView {
            id: server,
            genesis: GenesisBody { v: 1, name: "S".into(), age_restriction: AgeRestriction::None },
            owner: genesis.author(),
            state: log.resolver.state(),
            records: &log.records,
        };
        let heads = view.deps(None);
        assert_eq!(heads.len(), MAX_DEPS);
        assert!(!heads.contains(&orphan.id()));
        assert!(heads.contains(&log.records.last().unwrap().id()));
        assert!(heads.windows(2).all(|w| w[0] < w[1]), "sorted, as records requires");
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("קהילת הקוד"), "קה");
        assert_eq!(initials("Rust"), "Ru");
        assert_eq!(initials("  "), "?");
    }
}
