//! Voice calls (voice v1, piece B; voice rooms v2): who is in which room,
//! and the WebRTC signalling between members. The media never passes
//! through the core; the UI's WebRTC engine (piece D) does that, peer to
//! peer.
//!
//! - Every core-to-core message is an envelope signed by the sender's
//!   identity key under [`SIGNAL_PREFIX`]. The signature covers the whole
//!   body, the SDP and its `a=fingerprint` included, so a verified offer or
//!   answer binds the DTLS fingerprint to the sender's MVS identity.
//! - It fails closed: an envelope that doesn't verify, isn't version 2, is
//!   outside the clock window, is replayed, is bound to a room ID that
//!   doesn't derive from its server, creator and `createdAt`, comes from
//!   someone who couldn't call in that server, or is an offer or answer
//!   without a fingerprint is dropped (logged at debug). `call.signal` only
//!   ever carries verified signalling.
//!
//! Voice rooms v2 ("join to create", the tech lead's design of 2026-11-11):
//! - Rooms are soft state in hellos: no records, nothing stored or synced.
//!   A room lives while a verified participant's hellos keep arriving
//!   (each every [`HELLO_EVERY_MS`], dropped [`HELLO_TIMEOUT_MS`] after we
//!   received the last), and is gone with its last participant.
//! - Its ID is derived ([`room_id`]) from the server, the creator and the
//!   creation time, and every receiver recomputes it: it can't be forged
//!   for someone else, or collide with a channel or a v1 room.
//! - Each participant claims `joinedAt`; on the first hello of a stay it
//!   must be no earlier than the room's `createdAt` and within
//!   [`CLOCK_WINDOW_MS`] of the hello's time, and it is frozen after that.
//!   The master is the earliest `joinedAt` (ties: the smaller key).
//!   Accepted residual: someone joining within 2 minutes of another can
//!   claim to be earlier; the master can only rename and set the limit,
//!   and the server owner can always override.
//! - Name and limit come only from the master's hellos. The server owner
//!   edits through an `edit` sent to the master.
//! - Caps: [`MAX_ROOMS`] live rooms per server (the first to arrive), at
//!   most the room's limit (≤ [`MAX_PARTICIPANTS`]) per room, one room per
//!   identity (a hello for another room ends the earlier stay).
//! - Version 1 envelopes (0.4.x and earlier, the one room per server) are
//!   dropped: rooms need everyone on 0.4.3 or later.
//!
//! The blobs travel over sync's voice transport (piece A,
//! `Node::voice_send`), behind the [`VoiceNet`] port. They aren't stored,
//! synced or retried.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use ed25519_dalek::{Signature, Signer, VerifyingKey};
use rand_core::{OsRng, RngCore};
use records::{ChannelId, PublicKey, ServerId, SigningKey};
use serde::{Deserialize, Serialize};

use crate::model;
use crate::types::*;

/// What every voice envelope's signature covers, before its body.
pub const SIGNAL_PREFIX: &[u8] = b"mvs/voice-signal/v1";
/// The mesh cap, and the largest room limit.
pub const MAX_PARTICIPANTS: usize = 5;
/// The smallest room limit.
pub const MIN_LIMIT: u8 = 2;
/// Live rooms per server.
pub const MAX_ROOMS: usize = 10;
/// A room name's longest length, in characters.
pub const MAX_ROOM_NAME: usize = 32;
/// The largest `sendSignal` payload, so an envelope fits [`MAX_BLOB`].
pub const MAX_PAYLOAD: usize = 12 * 1024;
/// The largest blob sent or accepted (piece A's limit).
pub const MAX_BLOB: usize = 16 * 1024;
/// How far an envelope's time may be from ours, either way (home PCs'
/// clocks drift).
pub const CLOCK_WINDOW_MS: u64 = 120_000;
pub const HELLO_EVERY_MS: u64 = 10_000;
pub const HELLO_TIMEOUT_MS: u64 = 30_000;
/// Nonces remembered for replay checks, within the clock window. When full,
/// new envelopes are dropped (fail closed) rather than evicting a live
/// nonce, which would let a replay through. Piece A's per-peer rate cap
/// keeps a flood from filling it for long; keep the two in step.
const MAX_NONCES: usize = 4096;
/// How long after joining a `full` may still turn us away.
const JOINING_MS: u64 = HELLO_EVERY_MS;
/// The envelope version: 2 is voice rooms.
const VERSION: u8 = 2;
/// The voice room ID's `derive_key` context. Frozen.
const ROOM_CTX: &str = "mvs/voice-room/v2";

/// The ID of the room `creator` created in `server` at `created_at` (ms).
/// Every receiver derives it again, so it can't be claimed for another
/// creator or time, and can't be a record-made channel's or a v1 room's.
pub fn room_id(server: ServerId, creator: PublicKey, created_at: u64) -> ChannelId {
    let mut input = Vec::with_capacity(72);
    input.extend_from_slice(server.as_bytes());
    input.extend_from_slice(creator.as_bytes());
    input.extend_from_slice(&created_at.to_le_bytes());
    ChannelId::from_bytes(blake3::derive_key(ROOM_CTX, &input))
}

/// Sync's voice transport (piece A). Received blobs come back through
/// `Core::voice_received`.
pub trait VoiceNet: Send + Sync {
    /// Relay-only is on: calls are off (their media would go direct).
    fn relay_only(&self) -> bool {
        false
    }

    /// Sends one blob (at most [`MAX_BLOB`]) to that identity's peer, best
    /// effort.
    fn send(&self, to: PublicKey, blob: Vec<u8>);
}

/// Who may be in which call, and whom to tell: the core's member state.
pub(crate) trait Members {
    /// `server` is one of mine with an accepted genesis, and `who` is a
    /// member, not kicked. (That a room belongs to `server` is checked when
    /// its envelope opens: the ID derives from it.)
    fn may_call(&mut self, who: PublicKey, server: ServerId) -> bool;
    /// `who` is `server`'s owner, by the loaded log.
    fn is_owner(&mut self, who: PublicKey, server: ServerId) -> bool;
    /// The server's members we can reach now, us excluded.
    fn audience(&mut self, server: ServerId) -> Vec<PublicKey>;
}

/// What a call operation needs from the core.
pub(crate) struct Ctx<'a> {
    pub key: &'a SigningKey,
    pub now: u64,
    pub events: &'a mut Vec<Event>,
}

impl Ctx<'_> {
    fn me(&self) -> PublicKey {
        PublicKey::from_bytes(self.key.verifying_key().to_bytes())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireKind {
    Hello,
    Bye,
    Full,
    Offer,
    Answer,
    Ice,
    /// The server owner's rename or limit, to the master.
    Edit,
}

impl From<SignalKind> for WireKind {
    fn from(kind: SignalKind) -> WireKind {
        match kind {
            SignalKind::Offer => WireKind::Offer,
            SignalKind::Answer => WireKind::Answer,
            SignalKind::Ice => WireKind::Ice,
        }
    }
}

/// Which room an envelope is about: its ID derives from these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRoom {
    creator: String,
    #[serde(rename = "createdAt")]
    created_at: u64,
}

/// A room's name and limit, in the master's hellos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMeta {
    #[serde(default)]
    name: Option<String>,
    limit: u8,
}

/// An `edit`'s payload: what changes. `name: ""` restores the default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<u8>,
}

/// The signed part of an envelope. IDs are lower-case hex.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    v: u8,
    kind: WireKind,
    server: String,
    channel: String,
    from: String,
    /// `None`: everyone the hello or bye was sent to.
    to: Option<String>,
    ts: u64,
    nonce: String,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    payload: String,
    /// v2: required for every kind.
    #[serde(default)]
    room: Option<WireRoom>,
    /// v2 hellos: when the sender joined this room (its claim).
    #[serde(default, rename = "joinedAt")]
    joined_at: u64,
    /// v2 hellos from the master only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meta: Option<WireMeta>,
}

/// On the wire: the body as sent, and the signature over
/// `SIGNAL_PREFIX ‖ body`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    body: String,
    sig: String,
}

/// A verified envelope, its room ID checked against its server, creator
/// and `createdAt`.
#[derive(Debug)]
struct Opened {
    kind: WireKind,
    server: ServerId,
    channel: ChannelId,
    creator: PublicKey,
    created_at: u64,
    from: PublicKey,
    to: Option<PublicKey>,
    ts: u64,
    nonce: [u8; 16],
    muted: bool,
    payload: String,
    joined_at: u64,
    meta: Option<WireMeta>,
}

fn seal(key: &SigningKey, body: &Body) -> Vec<u8> {
    let body = serde_json::to_string(body).expect("a body serializes");
    let sig = key.sign(&[SIGNAL_PREFIX, body.as_bytes()].concat());
    let envelope = Envelope { body, sig: hex::encode(sig.to_bytes()) };
    serde_json::to_vec(&envelope).expect("an envelope serializes")
}

/// `None` unless the signature verifies, the version is 2, the time is
/// within [`CLOCK_WINDOW_MS`] of `now`, and the room ID derives from the
/// server, creator and `createdAt` it names.
fn open(blob: &[u8], now: u64) -> Option<Opened> {
    if blob.len() > MAX_BLOB {
        return None;
    }
    let envelope: Envelope = serde_json::from_slice(blob).ok()?;
    let sig: [u8; 64] = hex::decode(&envelope.sig).ok()?.try_into().ok()?;
    let body: Body = serde_json::from_str(&envelope.body).ok()?;
    let from = model::parse_hex32(&body.from)?;
    VerifyingKey::from_bytes(&from)
        .ok()?
        .verify_strict(
            &[SIGNAL_PREFIX, envelope.body.as_bytes()].concat(),
            &Signature::from_bytes(&sig),
        )
        .ok()?;
    if body.v != VERSION || body.ts.abs_diff(now) > CLOCK_WINDOW_MS {
        return None;
    }
    let to = match &body.to {
        Some(to) => Some(PublicKey::from_bytes(model::parse_hex32(to)?)),
        None => None,
    };
    let room = body.room?;
    let server = ServerId::from_bytes(model::parse_hex32(&body.server)?);
    let channel = ChannelId::from_bytes(model::parse_hex32(&body.channel)?);
    let creator = PublicKey::from_bytes(model::parse_hex32(&room.creator)?);
    if room_id(server, creator, room.created_at) != channel {
        return None;
    }
    Some(Opened {
        kind: body.kind,
        server,
        channel,
        creator,
        created_at: room.created_at,
        from: PublicKey::from_bytes(from),
        to,
        ts: body.ts,
        nonce: hex::decode(&body.nonce).ok()?.try_into().ok()?,
        muted: body.muted,
        payload: body.payload,
        joined_at: body.joined_at,
        meta: body.meta,
    })
}

/// The SDP's `a=fingerprint:` lines, in order; empty without one.
pub(crate) fn fingerprints(sdp: &str) -> Vec<&str> {
    sdp.lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix("a=fingerprint:"))
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .collect()
}

/// A room name as shown: trimmed, 1–[`MAX_ROOM_NAME`] characters, no bidi,
/// control or zero-width characters (the display-name rule). `None` if not.
fn room_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_ROOM_NAME
        && authority::display_text_ok(name);
    ok.then(|| name.to_owned())
}

fn limit_ok(limit: u8) -> bool {
    (MIN_LIMIT..=MAX_PARTICIPANTS as u8).contains(&limit)
}

/// A room's name and limit. `name: None`: the default (the UI shows the
/// master's name).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Meta {
    name: Option<String>,
    limit: u8,
}

impl Default for Meta {
    fn default() -> Meta {
        Meta { name: None, limit: MAX_PARTICIPANTS as u8 }
    }
}

impl Meta {
    /// A master's meta, if valid; an invalid one is ignored.
    fn from_wire(wire: &WireMeta) -> Option<Meta> {
        let name = match &wire.name {
            Some(raw) => Some(room_name(raw)?),
            None => None,
        };
        limit_ok(wire.limit).then_some(Meta { name, limit: wire.limit })
    }

    fn wire(&self) -> WireMeta {
        WireMeta { name: self.name.clone(), limit: self.limit }
    }

    /// `edit` applied: `None` if invalid (a bad name or limit, or a limit
    /// below `count`, who is in the room now).
    fn edited(&self, edit: &WireEdit, count: usize) -> Option<Meta> {
        let mut meta = self.clone();
        if let Some(raw) = &edit.name {
            meta.name = if raw.trim().is_empty() { None } else { Some(room_name(raw)?) };
        }
        if let Some(limit) = edit.limit {
            if !limit_ok(limit) || usize::from(limit) < count {
                return None;
            }
            meta.limit = limit;
        }
        Some(meta)
    }
}

/// My call.
struct Mine {
    server: ServerId,
    channel: ChannelId,
    muted: bool,
    joined_at: u64,
    /// When I joined, by my clock (for `full`).
    since: u64,
}

#[derive(Clone, Copy)]
struct Peer {
    muted: bool,
    /// By our clock: when we received its last hello.
    seen: u64,
    /// Its claim, checked and frozen on the first hello of its stay.
    joined_at: u64,
}

/// A live room, from verified hellos (rooms I'm not in too: every member
/// sees them). At most its limit of peers, the first to arrive.
struct Room {
    server: ServerId,
    creator: PublicKey,
    created_at: u64,
    peers: BTreeMap<PublicKey, Peer>,
    /// The last valid meta from its master.
    meta: Meta,
    /// By our clock: when we first learned of it (see `on_hello`'s
    /// `joinedAt` rule).
    first_seen: u64,
}

#[derive(Default)]
pub(crate) struct Calls {
    net: Option<Arc<dyn VoiceNet>>,
    mine: Option<Mine>,
    rooms: HashMap<ChannelId, Room>,
    nonces: VecDeque<(u64, [u8; 16])>,
    seen: HashSet<[u8; 16]>,
    last_hello: u64,
    /// Servers whose rooms changed since the last tick: one `voice.rooms`
    /// each, at the next tick (coalesced).
    dirty: BTreeSet<ServerId>,
    /// The user's relay-only setting (`setRelayOnly`), kept by the core.
    relay_only: bool,
    /// The room my own identity is in from another computer, and when its
    /// last hello reached us. `createVoiceRoom` and `joinCall` here answer
    /// `in-call-elsewhere` (one room per identity).
    elsewhere: HashMap<ChannelId, (ServerId, u64)>,
}

impl Calls {
    pub fn set_net(&mut self, net: Arc<dyn VoiceNet>) {
        self.net = Some(net);
    }

    /// `setRelayOnly`: calls are off while it's on. A call in progress ends
    /// at the next tick (or right away, see `Core::set_relay_only`).
    pub fn set_relay_only(&mut self, on: bool) {
        self.relay_only = on;
    }

    /// Relay-only is on: the setting, or the transport says so.
    pub fn relay_only(&self) -> bool {
        self.relay_only || self.net.as_ref().is_some_and(|net| net.relay_only())
    }

    /// One `voice.rooms` for `server` at the next tick (its `relayOnly`
    /// changed, say).
    pub fn mark_dirty(&mut self, server: ServerId) {
        self.dirty.insert(server);
    }

    pub fn in_call(&self) -> bool {
        self.mine.is_some()
    }

    /// In a call in `server`.
    pub fn in_server(&self, server: ServerId) -> bool {
        self.mine.as_ref().is_some_and(|m| m.server == server)
    }

    /// The server of the live room `channel`, if any.
    pub fn room_server(&self, channel: ChannelId) -> Option<ServerId> {
        self.rooms.get(&channel).map(|room| room.server)
    }

    /// Server lifecycle: forgets who is in `server`'s calls (after leaving
    /// mine there, see `Core`'s purge).
    pub fn forget_server(&mut self, server: ServerId) {
        if self.in_server(server) {
            self.mine = None;
        }
        self.rooms.retain(|_, room| room.server != server);
        self.elsewhere.retain(|_, (s, _)| *s != server);
        // No `voice.rooms` for a server we're no longer in.
        self.dirty.remove(&server);
    }

    /// Remembers the nonce of an envelope I send to everyone, so an echo of
    /// my own envelope is a replay and never counts as my other computer.
    fn own(&mut self, body: &Body, now: u64) {
        if let Ok(Ok(nonce)) = hex::decode(&body.nonce).map(<[u8; 16]>::try_from) {
            let _ = self.fresh(nonce, now);
        }
    }

    /// My identity is in a room from another computer: its hello came
    /// within the last [`HELLO_TIMEOUT_MS`] and no bye since.
    fn in_call_elsewhere(&mut self, now: u64) -> bool {
        self.elsewhere.retain(|_, (_, seen)| now.saturating_sub(*seen) < HELLO_TIMEOUT_MS);
        !self.elsewhere.is_empty()
    }

    /// A hello or bye signed by my own identity: only from my other
    /// computer, which sync bound to my identity (`from`), to everyone (no
    /// `to`), in a server I may call in. It's remembered, never added to a
    /// room's roster. Anything else signed by me is dropped.
    fn on_my_other_computer(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        from: Option<PublicKey>,
        signal: &Opened,
    ) {
        let me = ctx.me();
        let wanted = from == Some(me)
            && signal.to.is_none()
            && matches!(signal.kind, WireKind::Hello | WireKind::Bye);
        if !wanted {
            return tracing::debug!("voice: dropped an envelope signed by me");
        }
        if !self.fresh(signal.nonce, ctx.now) {
            return tracing::debug!("voice: dropped a replayed envelope");
        }
        if !members.may_call(me, signal.server) {
            return tracing::debug!("voice: dropped my other computer's envelope for a server I can't call in");
        }
        if signal.kind == WireKind::Hello {
            // One room at a time there too.
            self.elsewhere.clear();
            self.elsewhere.insert(signal.channel, (signal.server, ctx.now));
        } else {
            self.elsewhere.remove(&signal.channel);
        }
    }

    fn net(&self) -> CoreResult<&Arc<dyn VoiceNet>> {
        self.net.as_ref().ok_or_else(|| ErrorCode::NotImplemented.into())
    }

    /// Who is in `channel` now, me included.
    fn count(&self, channel: ChannelId) -> usize {
        let mine = self.mine.as_ref().is_some_and(|m| m.channel == channel);
        self.rooms.get(&channel).map_or(0, |room| room.peers.len()) + usize::from(mine)
    }

    fn live_rooms(&self, server: ServerId) -> usize {
        self.rooms.values().filter(|room| room.server == server).count()
    }

    /// `channel`'s members as (joinedAt, key, muted), earliest first; the
    /// master is the first.
    fn roster(&self, me: PublicKey, channel: ChannelId) -> Vec<(u64, PublicKey, bool)> {
        let mut roster: Vec<(u64, PublicKey, bool)> = self
            .rooms
            .get(&channel)
            .map(|room| room.peers.iter().map(|(k, p)| (p.joined_at, *k, p.muted)).collect())
            .unwrap_or_default();
        if let Some(mine) = self.mine.as_ref().filter(|m| m.channel == channel) {
            roster.push((mine.joined_at, me, mine.muted));
        }
        roster.sort_by(|a, b| (a.0, a.1.as_bytes()).cmp(&(b.0, b.1.as_bytes())));
        roster
    }

    fn master(&self, me: PublicKey, channel: ChannelId) -> Option<PublicKey> {
        self.roster(me, channel).first().map(|(_, key, _)| *key)
    }

    /// Before creating or joining: relay-only, a transport, a member, and
    /// not in a room from my other computer.
    fn may_enter(
        &mut self,
        ctx: &Ctx,
        members: &mut dyn Members,
        server: ServerId,
        channel: Option<ChannelId>,
    ) -> CoreResult<()> {
        if self.relay_only() {
            return Err(ErrorCode::RelayOnly.into());
        }
        self.net()?;
        if !members.may_call(ctx.me(), server) {
            return Err(ErrorCode::NotAllowed.into());
        }
        // One computer per call, one room per identity: my identity twice
        // would get half the signals on each (sync routes them to one peer).
        let here = channel.is_some() && self.mine.as_ref().map(|m| m.channel) == channel;
        if !here && self.in_call_elsewhere(ctx.now) {
            return Err(ErrorCode::InCallElsewhere.into());
        }
        Ok(())
    }

    /// `createVoiceRoom`: a new room in `server` with me as its master.
    /// Leaves my current call first.
    pub fn create(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        server: ServerId,
    ) -> CoreResult<CallState> {
        self.may_enter(ctx, members, server, None)?;
        self.expire(ctx.now, members);
        let leaving_here = self.mine.as_ref().is_some_and(|m| {
            m.server == server && self.rooms.get(&m.channel).is_some_and(|r| r.peers.is_empty())
        });
        // My own room, empty but for me, goes when I leave it: it doesn't count.
        if self.live_rooms(server) - usize::from(leaving_here) >= MAX_ROOMS {
            return Err(ErrorCode::RoomCap.into());
        }
        self.leave(ctx, members);
        let (me, created_at) = (ctx.me(), ctx.now);
        let channel = room_id(server, me, created_at);
        self.rooms.insert(
            channel,
            Room {
                server,
                creator: me,
                created_at,
                peers: BTreeMap::new(),
                meta: Meta::default(),
                first_seen: created_at,
            },
        );
        self.enter(ctx, members, server, channel, created_at)
    }

    /// `joinCall(roomId)`: a live room of `server`. Leaves my current call
    /// first.
    pub fn join(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        server: ServerId,
        channel: ChannelId,
    ) -> CoreResult<CallState> {
        self.may_enter(ctx, members, server, Some(channel))?;
        if self.mine.as_ref().is_some_and(|m| m.channel == channel) {
            return Ok(self.state(ctx.me()).expect("in a call"));
        }
        self.expire(ctx.now, members);
        let Some(room) = self.rooms.get(&channel).filter(|room| room.server == server) else {
            return Err(ErrorCode::NotFound.into());
        };
        if room.peers.len() >= usize::from(room.meta.limit) {
            return Err(ErrorCode::CallFull.into());
        }
        // Never before the room's creation, even if its creator's clock is ahead.
        let joined_at = ctx.now.max(room.created_at);
        self.leave(ctx, members);
        self.enter(ctx, members, server, channel, joined_at)
    }

    fn enter(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        server: ServerId,
        channel: ChannelId,
        joined_at: u64,
    ) -> CoreResult<CallState> {
        self.mine = Some(Mine { server, channel, muted: false, joined_at, since: ctx.now });
        self.dirty.insert(server);
        self.hello(ctx, members, None);
        let state = self.state(ctx.me()).expect("in a call");
        ctx.events.push(Event::CallState(state.clone()));
        Ok(state)
    }

    pub fn leave(&mut self, ctx: &mut Ctx, members: &mut dyn Members) {
        let Some(mine) = self.mine.take() else { return };
        self.dirty.insert(mine.server);
        if let Some(body) = self.body(ctx, WireKind::Bye, mine.channel, None) {
            self.own(&body, ctx.now);
            self.broadcast(ctx, members, mine.server, body);
        }
        if self.rooms.get(&mine.channel).is_some_and(|room| room.peers.is_empty()) {
            self.rooms.remove(&mine.channel);
        }
        ctx.events.push(Event::CallState(CallState {
            channel_id: model::hex_id(mine.channel.as_bytes()),
            participants: Vec::new(),
            full: false,
            relay_only: self.relay_only(),
        }));
    }

    pub fn set_muted(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        muted: bool,
    ) -> CoreResult<()> {
        let mine = self.mine.as_mut().ok_or(ErrorCode::NotAllowed)?;
        if mine.muted != muted {
            mine.muted = muted;
            self.dirty.insert(mine.server);
            self.hello(ctx, members, None);
            ctx.events.push(Event::CallState(self.state(ctx.me()).expect("in a call")));
        }
        Ok(())
    }

    /// `editVoiceRoom`: the master applies it at once (its next hello, sent
    /// now, carries it); the server owner sends it to the master. Anyone
    /// else: `not-allowed`.
    pub fn edit(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        channel: ChannelId,
        name: Option<String>,
        limit: Option<u8>,
    ) -> CoreResult<()> {
        let me = ctx.me();
        self.expire(ctx.now, members);
        let Some(room) = self.rooms.get(&channel) else {
            return Err(ErrorCode::NotFound.into());
        };
        let server = room.server;
        if !members.may_call(me, server) {
            return Err(ErrorCode::NotAllowed.into());
        }
        let edit = WireEdit { name, limit };
        let meta = room.meta.edited(&edit, self.count(channel)).ok_or(ErrorCode::BadRequest)?;
        let master = self.master(me, channel);
        if master == Some(me) {
            self.set_meta(channel, meta);
            self.hello(ctx, members, None);
            return Ok(());
        }
        if !members.is_owner(me, server) {
            return Err(ErrorCode::NotAllowed.into());
        }
        let master = master.ok_or(ErrorCode::NotFound)?;
        if !members.audience(server).contains(&master) {
            return Err(ErrorCode::NotFound.into());
        }
        let mut body = self.body(ctx, WireKind::Edit, channel, Some(master)).ok_or(ErrorCode::NotFound)?;
        body.payload = serde_json::to_string(&edit).expect("an edit serializes");
        self.send(ctx, master, &body);
        Ok(())
    }

    fn set_meta(&mut self, channel: ChannelId, meta: Meta) {
        if let Some(room) = self.rooms.get_mut(&channel) {
            if room.meta != meta {
                room.meta = meta;
                self.dirty.insert(room.server);
            }
        }
    }

    pub fn send_signal(
        &mut self,
        ctx: &mut Ctx,
        to: PublicKey,
        kind: SignalKind,
        payload: String,
    ) -> CoreResult<()> {
        let mine = self.mine.as_ref().ok_or(ErrorCode::NotAllowed)?;
        if !self.rooms.get(&mine.channel).is_some_and(|room| room.peers.contains_key(&to)) {
            return Err(ErrorCode::NotAllowed.into());
        }
        let needs_fingerprint = matches!(kind, SignalKind::Offer | SignalKind::Answer);
        if payload.len() > MAX_PAYLOAD || (needs_fingerprint && fingerprints(&payload).is_empty()) {
            return Err(ErrorCode::BadRequest.into());
        }
        let channel = mine.channel;
        let mut body = self.body(ctx, kind.into(), channel, Some(to)).ok_or(ErrorCode::NotAllowed)?;
        body.payload = payload;
        self.send(ctx, to, &body);
        Ok(())
    }

    /// Every [`crate::SYNC_EVERY`]: drops peers gone quiet or no longer
    /// allowed (a kick within one tick), says hello again when due, then
    /// sends one `voice.rooms` per server whose rooms changed, only for
    /// servers I'm still a member of.
    pub fn tick(&mut self, ctx: &mut Ctx, members: &mut dyn Members) {
        self.tick_calls(ctx, members);
        let me = ctx.me();
        for server in std::mem::take(&mut self.dirty) {
            if members.may_call(me, server) {
                let rooms = self.rooms_of(me, server);
                ctx.events.push(Event::VoiceRooms {
                    server_id: model::hex_id(server.as_bytes()),
                    rooms,
                });
            }
        }
    }

    fn tick_calls(&mut self, ctx: &mut Ctx, members: &mut dyn Members) {
        if let Some(server) = self.mine.as_ref().map(|m| m.server) {
            // Kicked, or relay-only turned on mid-call (the media would go
            // direct): leave, with a bye.
            if self.relay_only() || !members.may_call(ctx.me(), server) {
                self.leave(ctx, members);
            }
        }
        if self.expire(ctx.now, members) {
            if let Some(state) = self.state(ctx.me()) {
                ctx.events.push(Event::CallState(state));
            }
        }
        if self.mine.is_some() && ctx.now.saturating_sub(self.last_hello) >= HELLO_EVERY_MS {
            self.hello(ctx, members, None);
        }
    }

    /// A blob from piece A. `from`: the identity sync bound the sending peer
    /// to, when it knows; it must be the envelope's signer.
    pub fn received(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        from: Option<PublicKey>,
        blob: &[u8],
    ) {
        // Version 2 only, its room ID derived from its server, creator and
        // `createdAt`: anything else is dropped here.
        let Some(signal) = open(blob, ctx.now) else {
            return tracing::debug!("voice: dropped an envelope that doesn't verify");
        };
        let me = ctx.me();
        if signal.from == me {
            return self.on_my_other_computer(ctx, members, from, &signal);
        }
        if from.is_some_and(|from| from != signal.from) || signal.to.is_some_and(|to| to != me) {
            return tracing::debug!("voice: dropped a misaddressed envelope");
        }
        if !self.fresh(signal.nonce, ctx.now) {
            return tracing::debug!("voice: dropped a replayed envelope");
        }
        if !members.may_call(signal.from, signal.server) {
            return tracing::debug!("voice: dropped an envelope from someone not allowed");
        }
        let in_this_call = self
            .mine
            .as_ref()
            .is_some_and(|m| (m.server, m.channel) == (signal.server, signal.channel));
        let known = self
            .rooms
            .get(&signal.channel)
            .is_some_and(|room| room.peers.contains_key(&signal.from));
        match signal.kind {
            WireKind::Hello => self.on_hello(ctx, members, &signal, in_this_call),
            WireKind::Bye => {
                if self.remove_peer(signal.channel, signal.from) && in_this_call {
                    ctx.events.push(Event::CallState(self.state(me).expect("in a call")));
                }
            }
            WireKind::Full => {
                // A full call refused us: only believed from someone already
                // in that call, and only while we are still joining.
                let joining = self.mine.as_ref().is_some_and(|m| {
                    in_this_call && ctx.now.saturating_sub(m.since) <= JOINING_MS
                });
                if !joining || !known || signal.to != Some(me) {
                    return tracing::debug!("voice: ignored a full from outside the call");
                }
                let channel = signal.channel;
                self.mine = None;
                self.dirty.insert(signal.server);
                ctx.events.push(Event::CallState(CallState {
                    channel_id: model::hex_id(channel.as_bytes()),
                    participants: Vec::new(),
                    full: true,
                    relay_only: self.relay_only(),
                }));
            }
            WireKind::Edit => {
                // Only the server owner's, only to me as the room's master.
                let master = self.master(me, signal.channel) == Some(me);
                let edit: Option<WireEdit> = serde_json::from_str(&signal.payload).ok();
                if !in_this_call || !master || signal.to != Some(me) || !members.is_owner(signal.from, signal.server) {
                    return tracing::debug!("voice: dropped an edit from someone who may not");
                }
                let count = self.count(signal.channel);
                let meta = self
                    .rooms
                    .get(&signal.channel)
                    .and_then(|room| room.meta.edited(&edit?, count));
                let Some(meta) = meta else {
                    return tracing::debug!("voice: dropped an invalid edit");
                };
                self.set_meta(signal.channel, meta);
                self.hello(ctx, members, None);
            }
            WireKind::Offer | WireKind::Answer | WireKind::Ice => {
                let kind = match signal.kind {
                    WireKind::Offer => SignalKind::Offer,
                    WireKind::Answer => SignalKind::Answer,
                    _ => SignalKind::Ice,
                };
                if !in_this_call || !known || signal.to != Some(me) {
                    return tracing::debug!("voice: dropped a signal outside my call");
                }
                if kind != SignalKind::Ice && fingerprints(&signal.payload).is_empty() {
                    return tracing::debug!(
                        "voice: dropped an offer or answer without a fingerprint"
                    );
                }
                ctx.events.push(Event::CallSignal {
                    from: model::hex_id(signal.from.as_bytes()),
                    kind,
                    payload: signal.payload,
                    fingerprint_verified: true,
                });
            }
        }
    }

    /// `who` left `channel` (bye, or a hello for another room); true if it
    /// was there. An emptied room goes, unless I'm in it.
    fn remove_peer(&mut self, channel: ChannelId, who: PublicKey) -> bool {
        let mine = self.mine.as_ref().is_some_and(|m| m.channel == channel);
        let Some(room) = self.rooms.get_mut(&channel) else { return false };
        let gone = room.peers.remove(&who).is_some();
        if gone {
            self.dirty.insert(room.server);
        }
        if room.peers.is_empty() && !mine {
            self.rooms.remove(&channel);
        }
        gone
    }

    fn on_hello(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        signal: &Opened,
        in_this_call: bool,
    ) {
        let me = ctx.me();
        let (server, channel) = (signal.server, signal.channel);
        if !self.rooms.contains_key(&channel) {
            // A room new to us: by a member, not from the future, and under
            // the server's cap (the first rooms to arrive are kept). It may
            // be old: we may have come online long after it started.
            let ahead = signal.created_at > ctx.now.saturating_add(CLOCK_WINDOW_MS);
            if ahead || !members.may_call(signal.creator, server) {
                return tracing::debug!("voice: dropped a hello for a future or foreign room");
            }
            if self.live_rooms(server) >= MAX_ROOMS {
                return tracing::debug!("voice: dropped a hello for a room over the server's cap");
            }
        }
        let before = self.rooms.get(&channel).and_then(|room| room.peers.get(&signal.from)).copied();
        let first_seen = self.rooms.get(&channel).map_or(ctx.now, |room| room.first_seen);
        let joined_at = match before {
            // Frozen for the stay.
            Some(peer) => peer.joined_at,
            // Learning the room (we just came online, or it just started):
            // every member already in it says hello within one timeout, so
            // their claims are taken as they are, if no earlier than the
            // room and not ahead of the hello. The cores that saw the room
            // start agree on these, whatever order the hellos arrive in.
            None if ctx.now.saturating_sub(first_seen) <= HELLO_TIMEOUT_MS => {
                let ok = signal.joined_at >= signal.created_at
                    && signal.joined_at <= signal.ts.saturating_add(CLOCK_WINDOW_MS);
                if !ok {
                    return tracing::debug!("voice: dropped a hello with a joinedAt outside the window");
                }
                signal.joined_at
            }
            // A newcomer to a room we've known for a while: it can't have
            // joined long before we heard of it, so its claim is clamped to
            // the clock window (never dropped).
            None => signal
                .joined_at
                .max(ctx.now.saturating_sub(CLOCK_WINDOW_MS))
                .max(signal.created_at),
        };
        let room = self.rooms.entry(channel).or_insert_with(|| Room {
            server,
            creator: signal.creator,
            created_at: signal.created_at,
            peers: BTreeMap::new(),
            meta: Meta::default(),
            first_seen: ctx.now,
        });
        let limit = usize::from(room.meta.limit);
        if before.is_none() {
            // A room I'm not in keeps the first to arrive, up to its limit.
            if !in_this_call && room.peers.len() >= limit {
                if room.peers.is_empty() {
                    self.rooms.remove(&channel);
                }
                return tracing::debug!("voice: ignored a hello for a full room");
            }
            // Me and the others at the limit: full.
            if in_this_call && room.peers.len() + 1 >= limit {
                if let Some(body) = self.body(ctx, WireKind::Full, channel, Some(signal.from)) {
                    self.send(ctx, signal.from, &body);
                }
                return;
            }
        }
        // One room per identity: a hello here ends its stay elsewhere.
        let elsewhere: Vec<ChannelId> = self
            .rooms
            .iter()
            .filter(|(id, room)| **id != channel && room.peers.contains_key(&signal.from))
            .map(|(id, _)| *id)
            .collect();
        let mine = self.mine.as_ref().map(|m| m.channel);
        for old in elsewhere {
            if self.remove_peer(old, signal.from) && Some(old) == mine {
                ctx.events.push(Event::CallState(self.state(me).expect("in a call")));
            }
        }
        let room = self.rooms.get_mut(&channel).expect("just made");
        let changed = before.is_none_or(|p| p.muted != signal.muted);
        // Expiry runs from when we received it, never the sender's `ts`.
        room.peers.insert(signal.from, Peer { muted: signal.muted, seen: ctx.now, joined_at });
        if changed {
            self.dirty.insert(server);
        }
        // Name and limit: only from the room's master, and only when valid.
        if self.master(me, channel) == Some(signal.from) {
            if let Some(meta) = signal.meta.as_ref().and_then(Meta::from_wire) {
                self.set_meta(channel, meta);
            }
        }
        if in_this_call {
            if before.is_none() {
                // So the newcomer learns of us now, not at our next hello.
                self.hello(ctx, members, Some(signal.from));
            }
            if changed {
                ctx.events.push(Event::CallState(self.state(me).expect("in a call")));
            }
        }
    }

    /// Hello to `to`, or to the whole audience (then it counts as our
    /// periodic one). The master's carries the room's name and limit.
    fn hello(&mut self, ctx: &mut Ctx, members: &mut dyn Members, to: Option<PublicKey>) {
        let Some(mine) = &self.mine else { return };
        let (server, channel, muted, joined_at) = (mine.server, mine.channel, mine.muted, mine.joined_at);
        let Some(mut body) = self.body(ctx, WireKind::Hello, channel, to) else { return };
        self.own(&body, ctx.now);
        body.muted = muted;
        body.joined_at = joined_at;
        if self.master(ctx.me(), channel) == Some(ctx.me()) {
            body.meta = self.rooms.get(&channel).map(|room| room.meta.wire());
        }
        match to {
            Some(to) => self.send(ctx, to, &body),
            None => {
                self.last_hello = ctx.now;
                self.broadcast(ctx, members, server, body);
            }
        }
    }

    /// A v2 body about the live room `channel`; `None` if it isn't known.
    fn body(&self, ctx: &Ctx, kind: WireKind, channel: ChannelId, to: Option<PublicKey>) -> Option<Body> {
        let room = self.rooms.get(&channel)?;
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        Some(Body {
            v: VERSION,
            kind,
            server: model::hex_id(room.server.as_bytes()),
            channel: model::hex_id(channel.as_bytes()),
            from: model::hex_id(ctx.me().as_bytes()),
            to: to.map(|to| model::hex_id(to.as_bytes())),
            ts: ctx.now,
            nonce: hex::encode(nonce),
            muted: false,
            payload: String::new(),
            room: Some(WireRoom {
                creator: model::hex_id(room.creator.as_bytes()),
                created_at: room.created_at,
            }),
            joined_at: 0,
            meta: None,
        })
    }

    fn send(&self, ctx: &Ctx, to: PublicKey, body: &Body) {
        if let Some(net) = &self.net {
            net.send(to, seal(ctx.key, body));
        }
    }

    /// To the server's reachable members who may call.
    fn broadcast(&self, ctx: &Ctx, members: &mut dyn Members, server: ServerId, body: Body) {
        let Some(net) = &self.net else { return };
        let kind = body.kind;
        let blob = seal(ctx.key, &body);
        for to in members.audience(server) {
            if members.may_call(to, server) {
                net.send(to, blob.clone());
            }
        }
        // My other computers (same identity): they learn I'm in this room,
        // so they don't join a room too. Sync delivers it only to peers
        // bound to my identity; none is dropped quietly.
        if matches!(kind, WireKind::Hello | WireKind::Bye) {
            net.send(ctx.me(), blob);
        }
    }

    /// Not seen within the clock window: remembers it.
    /// Full of live nonces: false (fail closed).
    fn fresh(&mut self, nonce: [u8; 16], now: u64) -> bool {
        while let Some((at, old)) = self.nonces.front().copied() {
            if now.saturating_sub(at) <= 2 * CLOCK_WINDOW_MS {
                break;
            }
            self.nonces.pop_front();
            self.seen.remove(&old);
        }
        if self.nonces.len() >= MAX_NONCES || !self.seen.insert(nonce) {
            return false;
        }
        self.nonces.push_back((now, nonce));
        true
    }

    /// Drops peers gone quiet or no longer allowed, and rooms left empty
    /// (mine stays while I'm in it); true if my call changed.
    fn expire(&mut self, now: u64, members: &mut dyn Members) -> bool {
        let mine = self.mine.as_ref().map(|m| m.channel);
        let mut changed = false;
        for (channel, room) in &mut self.rooms {
            let before = room.peers.len();
            let server = room.server;
            room.peers.retain(|key, peer| {
                now.saturating_sub(peer.seen) <= HELLO_TIMEOUT_MS && members.may_call(*key, server)
            });
            if room.peers.len() != before {
                self.dirty.insert(server);
            }
            changed |= Some(*channel) == mine && room.peers.len() != before;
        }
        self.rooms.retain(|channel, room| !room.peers.is_empty() || Some(*channel) == mine);
        changed
    }

    /// My call: me first, then the others by key.
    fn state(&self, me: PublicKey) -> Option<CallState> {
        let mine = self.mine.as_ref()?;
        let mut participants =
            vec![CallParticipant { identity: model::hex_id(me.as_bytes()), muted: mine.muted }];
        let mut limit = MAX_PARTICIPANTS;
        if let Some(room) = self.rooms.get(&mine.channel) {
            limit = usize::from(room.meta.limit);
            participants.extend(room.peers.iter().map(|(key, peer)| CallParticipant {
                identity: model::hex_id(key.as_bytes()),
                muted: peer.muted,
            }));
        }
        let full = participants.len() >= limit;
        Some(CallState {
            channel_id: model::hex_id(mine.channel.as_bytes()),
            participants,
            full,
            relay_only: self.relay_only(),
        })
    }

    /// `listVoiceRooms`: drops who went quiet or was kicked, then the
    /// server's rooms (whether I may see them is the caller's check).
    pub fn list(&mut self, ctx: &mut Ctx, members: &mut dyn Members, server: ServerId) -> Vec<VoiceRoom> {
        if self.expire(ctx.now, members) {
            if let Some(state) = self.state(ctx.me()) {
                ctx.events.push(Event::CallState(state));
            }
        }
        self.rooms_of(ctx.me(), server)
    }

    /// `server`'s live rooms, oldest first (then by ID); each one's members
    /// by `joinedAt`, the master first. At most [`MAX_ROOMS`] rooms and
    /// [`MAX_PARTICIPANTS`] members each.
    fn rooms_of(&self, me: PublicKey, server: ServerId) -> Vec<VoiceRoom> {
        let mut rooms: Vec<(&ChannelId, &Room)> =
            self.rooms.iter().filter(|(_, room)| room.server == server).collect();
        rooms.sort_by(|a, b| (a.1.created_at, a.0.as_bytes()).cmp(&(b.1.created_at, b.0.as_bytes())));
        rooms
            .into_iter()
            .take(MAX_ROOMS)
            .filter_map(|(channel, room)| {
                let mut roster = self.roster(me, *channel);
                roster.truncate(MAX_PARTICIPANTS);
                let master = roster.first()?.1;
                Some(VoiceRoom {
                    server_id: model::hex_id(server.as_bytes()),
                    room_id: model::hex_id(channel.as_bytes()),
                    creator_id: model::hex_id(room.creator.as_bytes()),
                    created_at: room.created_at,
                    name: room.meta.name.clone(),
                    limit: room.meta.limit,
                    master_id: model::hex_id(master.as_bytes()),
                    members: roster
                        .into_iter()
                        .map(|(joined_at, key, muted)| VoiceMember {
                            user_id: model::hex_id(key.as_bytes()),
                            muted,
                            joined_at,
                        })
                        .collect(),
                    relay_only: self.relay_only(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    const SERVER: ServerId = ServerId::from_bytes([1; 32]);

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn pk(n: u8) -> PublicKey {
        PublicKey::from_bytes(key(n).verifying_key().to_bytes())
    }

    fn user(n: u8) -> String {
        hex::encode(pk(n).as_bytes())
    }

    /// The room 2 created at `NOW`.
    fn room() -> ChannelId {
        room_id(SERVER, pk(2), NOW)
    }

    /// An offer from `from` in `room()`, joined at `NOW`.
    fn body(from: &SigningKey, ts: u64) -> Body {
        Body {
            v: VERSION,
            kind: WireKind::Offer,
            server: hex::encode(SERVER.as_bytes()),
            channel: hex::encode(room().as_bytes()),
            from: hex::encode(from.verifying_key().to_bytes()),
            to: Some(hex::encode([3; 32])),
            ts,
            nonce: hex::encode([4; 16]),
            muted: false,
            payload: "v=0\r\na=fingerprint:sha-256 AB:CD\r\n".into(),
            room: Some(WireRoom { creator: user(2), created_at: NOW }),
            joined_at: NOW,
            meta: None,
        }
    }

    /// The room `creator` made at `created_at`.
    fn in_room(body: &mut Body, creator: u8, created_at: u64) {
        body.room = Some(WireRoom { creator: user(creator), created_at });
        body.channel = hex::encode(room_id(SERVER, pk(creator), created_at).as_bytes());
    }

    fn signal(from: u8, kind: WireKind, to: Option<u8>, ts: u64, nonce: u8) -> Vec<u8> {
        let mut body = body(&key(from), ts);
        body.kind = kind;
        body.to = to.map(user);
        body.nonce = hex::encode([nonce; 16]);
        body.payload.clear();
        seal(&key(from), &body)
    }

    /// A hello from `from` in `creator`'s room made at `created_at`.
    fn hello_in(from: u8, creator: u8, created_at: u64, joined_at: u64, ts: u64, nonce: u8) -> Vec<u8> {
        let mut body = body(&key(from), ts);
        (body.kind, body.to, body.joined_at) = (WireKind::Hello, None, joined_at);
        body.nonce = hex::encode([nonce; 16]);
        body.payload.clear();
        in_room(&mut body, creator, created_at);
        seal(&key(from), &body)
    }

    #[test]
    fn a_sealed_envelope_opens() {
        let opened = open(&seal(&key(9), &body(&key(9), NOW)), NOW + 1000).unwrap();
        assert_eq!(opened.kind, WireKind::Offer);
        assert_eq!(opened.from.as_bytes(), &key(9).verifying_key().to_bytes());
        assert_eq!((opened.channel, opened.creator, opened.created_at), (room(), pk(2), NOW));
        assert_eq!(fingerprints(&opened.payload), ["sha-256 AB:CD"]);
    }

    #[test]
    fn a_changed_or_mislabelled_envelope_does_not_open() {
        let blob = String::from_utf8(seal(&key(9), &body(&key(9), NOW))).unwrap();
        // The SDP's fingerprint changed in transit.
        assert!(open(blob.replace("AB:CD", "AB:CE").as_bytes(), NOW).is_none());
        // Signed by one key, claiming another.
        let mut claimed = body(&key(8), NOW);
        claimed.payload.clear();
        assert!(open(&seal(&key(9), &claimed), NOW).is_none());
        // Not the voice prefix.
        let raw = serde_json::to_string(&body(&key(9), NOW)).unwrap();
        let sig = hex::encode(key(9).sign(raw.as_bytes()).to_bytes());
        let other = serde_json::to_vec(&Envelope { body: raw, sig }).unwrap();
        assert!(open(&other, NOW).is_none());
    }

    /// v1 (0.4.x's one room per server) and anything without a room are
    /// dropped; so is a room ID that doesn't derive from its server,
    /// creator and `createdAt` (forged, or claimed for someone else).
    #[test]
    fn only_version_2_with_a_derived_room_id_opens() {
        let mut v1 = body(&key(9), NOW);
        v1.v = 1;
        assert!(open(&seal(&key(9), &v1), NOW).is_none());
        let mut roomless = body(&key(9), NOW);
        roomless.room = None;
        assert!(open(&seal(&key(9), &roomless), NOW).is_none());
        for forge in [
            |b: &mut Body| b.channel = hex::encode([2; 32]),
            |b: &mut Body| b.room.as_mut().unwrap().created_at += 1,
            |b: &mut Body| b.room.as_mut().unwrap().creator = user(9),
            |b: &mut Body| b.server = hex::encode([9; 32]),
        ] {
            let mut forged = body(&key(9), NOW);
            forge(&mut forged);
            assert!(open(&seal(&key(9), &forged), NOW).is_none());
        }
        let old = seal(&key(9), &body(&key(9), NOW));
        assert!(open(&old, NOW + CLOCK_WINDOW_MS).is_some());
        assert!(open(&old, NOW + CLOCK_WINDOW_MS + 1).is_none());
        assert!(open(&old, NOW - CLOCK_WINDOW_MS - 1).is_none());
        assert!(open(&vec![b' '; MAX_BLOB + 1], NOW).is_none());
    }

    #[test]
    fn a_nonce_counts_once_and_a_full_cache_fails_closed() {
        let nonce = |n: u32| {
            let mut nonce = [7; 16];
            nonce[..4].copy_from_slice(&n.to_le_bytes());
            nonce
        };
        let mut calls = Calls::default();
        assert!(calls.fresh([1; 16], NOW));
        assert!(!calls.fresh([1; 16], NOW + 1000));
        for n in 1..MAX_NONCES as u32 {
            assert!(calls.fresh(nonce(n), NOW));
        }
        // Full: the first nonce is still remembered, and new ones are dropped.
        assert!(!calls.fresh(nonce(0), NOW + 1));
        assert!(!calls.fresh([1; 16], NOW + 1));
        assert_eq!((calls.nonces.len(), calls.seen.len()), (MAX_NONCES, MAX_NONCES));
        // Once the window passes, room again.
        assert!(calls.fresh(nonce(0), NOW + 2 * CLOCK_WINDOW_MS + 1));
    }

    #[derive(Default)]
    struct Net(std::sync::Mutex<Vec<PublicKey>>);

    impl VoiceNet for Net {
        fn send(&self, to: PublicKey, _: Vec<u8>) {
            self.0.lock().unwrap().push(to);
        }
    }

    /// Everyone may call except `barred`; `owner` owns the server.
    #[derive(Clone)]
    struct Club {
        all: Vec<PublicKey>,
        barred: Vec<PublicKey>,
        owner: Option<PublicKey>,
    }

    fn club(all: &[u8]) -> Club {
        Club { all: all.iter().map(|n| pk(*n)).collect(), barred: vec![], owner: None }
    }

    impl Members for Club {
        fn may_call(&mut self, who: PublicKey, _: ServerId) -> bool {
            !self.barred.contains(&who)
        }

        fn is_owner(&mut self, who: PublicKey, _: ServerId) -> bool {
            self.owner == Some(who)
        }

        fn audience(&mut self, _: ServerId) -> Vec<PublicKey> {
            self.all.clone()
        }
    }

    fn new_calls() -> Calls {
        let mut calls = Calls::default();
        calls.set_net(Arc::new(Net::default()));
        calls
    }

    fn members_of(calls: &Calls, me: u8, room: ChannelId) -> Vec<String> {
        calls.roster(pk(me), room).iter().map(|(_, k, _)| hex::encode(k.as_bytes())).collect()
    }

    #[test]
    fn hello_and_bye_reach_only_those_who_may_call() {
        let net = Arc::new(Net::default());
        let mut calls = Calls::default();
        calls.set_net(net.clone());
        let mut club = club(&[2, 3]);
        club.barred.push(pk(3));
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        calls.create(&mut ctx, &mut club, SERVER).unwrap();
        calls.leave(&mut ctx, &mut club);
        // pk(1) is me: my other computers hear my hello and bye too.
        assert_eq!(*net.0.lock().unwrap(), [pk(2), pk(1), pk(2), pk(1)]);
        // My room, empty but for me, went with me.
        assert!(calls.rooms_of(pk(1), SERVER).is_empty());
    }

    /// Security Lead (restore design): my identity on another computer is
    /// in a room, so creating or joining one here answers
    /// `in-call-elsewhere` (one room per identity), until its bye or until
    /// its hellos stop. Only my other computer counts (sync bound it to
    /// me), only a hello to everyone, and it never shows in a roster.
    #[test]
    fn a_call_on_my_other_computer_blocks_joining_here() {
        let mut calls = new_calls();
        let mut club = club(&[2]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        let elsewhere = ErrorCode::InCallElsewhere;
        let mine = |nonce| hello_in(1, 2, NOW, NOW, NOW, nonce);

        // Not from my other computer: a hello signed by me that sync says
        // came from someone else, or one addressed to someone.
        calls.received(&mut ctx, &mut club, Some(pk(2)), &mine(1));
        calls.received(&mut ctx, &mut club, Some(pk(1)), &signal(1, WireKind::Hello, Some(2), NOW, 2));
        assert!(!calls.in_call_elsewhere(NOW));

        // My other computer says hello: refused here, and I'm in no roster.
        calls.received(&mut ctx, &mut club, Some(pk(1)), &mine(3));
        assert_eq!(calls.join(&mut ctx, &mut club, SERVER, room()).unwrap_err().code, elsewhere);
        assert_eq!(calls.create(&mut ctx, &mut club, SERVER).unwrap_err().code, elsewhere);
        assert!(calls.rooms_of(pk(1), SERVER).is_empty());
        // Replayed: no effect after its bye.
        calls.received(&mut ctx, &mut club, Some(pk(1)), &signal(1, WireKind::Bye, None, NOW, 4));
        calls.received(&mut ctx, &mut club, Some(pk(1)), &mine(3));
        calls.create(&mut ctx, &mut club, SERVER).unwrap();
        calls.leave(&mut ctx, &mut club);

        // Its hellos stop: after the timeout, creating here works.
        calls.received(&mut ctx, &mut club, Some(pk(1)), &mine(5));
        assert_eq!(calls.create(&mut ctx, &mut club, SERVER).unwrap_err().code, elsewhere);
        ctx.now = NOW + HELLO_TIMEOUT_MS;
        calls.create(&mut ctx, &mut club, SERVER).unwrap();
    }

    #[test]
    fn full_counts_only_from_the_call_and_only_while_joining() {
        let joined = || {
            let mut calls = new_calls();
            let mut club = club(&[2, 4]);
            let mut events = Vec::new();
            let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
            // Peer 2 is in the room (its creator); peer 4 isn't.
            calls.received(&mut ctx, &mut club, None, &hello_in(2, 2, NOW, NOW, NOW, 1));
            calls.join(&mut ctx, &mut club, SERVER, room()).unwrap();
            (calls, club)
        };
        let full = |calls: &mut Calls, club: &mut Club, from, at, nonce| {
            let mut events = Vec::new();
            let mut ctx = Ctx { key: &key(1), now: at, events: &mut events };
            calls.received(&mut ctx, club, None, &signal(from, WireKind::Full, Some(1), at, nonce));
            calls.in_call()
        };

        let (mut calls, mut club) = joined();
        assert!(full(&mut calls, &mut club, 4, NOW + 1, 2), "a stranger's full counted");
        assert!(full(&mut calls, &mut club, 2, NOW + JOINING_MS + 1, 3), "a late full counted");
        let (mut calls, mut club) = joined();
        assert!(!full(&mut calls, &mut club, 2, NOW + 1, 4), "a joining full was ignored");
    }

    #[test]
    fn room_ids_derive_from_server_creator_and_time() {
        assert_eq!(room(), room_id(ServerId::from_bytes([1; 32]), pk(2), NOW));
        assert_ne!(room(), room_id(ServerId::from_bytes([9; 32]), pk(2), NOW));
        assert_ne!(room(), room_id(SERVER, pk(3), NOW));
        assert_ne!(room(), room_id(SERVER, pk(2), NOW + 1));
        assert_ne!(room(), model::default_channel(SERVER));
        // Not the v1 room either.
        let v1 = ChannelId::from_bytes(blake3::derive_key("mvs/voice-room/v1", SERVER.as_bytes()));
        assert_ne!(room(), v1);
    }

    #[test]
    fn a_hello_for_a_forged_room_is_dropped() {
        let mut calls = new_calls();
        let mut club = club(&[]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        // 3 claims a room in 2's name at a time 2 never used.
        let mut hello = body(&key(3), NOW);
        (hello.kind, hello.to) = (WireKind::Hello, None);
        hello.room = Some(WireRoom { creator: user(2), created_at: NOW + 5 });
        hello.payload.clear();
        calls.received(&mut ctx, &mut club, None, &seal(&key(3), &hello));
        // A room from the future: created after our clock plus the window.
        let ahead = NOW + CLOCK_WINDOW_MS + 1;
        calls.received(&mut ctx, &mut club, None, &hello_in(3, 3, ahead, ahead, NOW, 2));
        // A room whose creator may not call (kicked).
        club.barred.push(pk(4));
        calls.received(&mut ctx, &mut club, None, &hello_in(3, 4, NOW, NOW, NOW, 3));
        calls.tick(&mut ctx, &mut club);
        assert!(calls.rooms_of(pk(1), SERVER).is_empty());
    }

    /// `joinedAt` while learning a room (its first hello timeout): no
    /// earlier than the room, not ahead of the hello, or dropped. After
    /// that, a newcomer's claim is clamped to the clock window. Frozen for
    /// the stay in every case.
    #[test]
    fn a_joined_at_is_checked_clamped_and_then_frozen() {
        let mut calls = new_calls();
        let mut club = club(&[]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        calls.received(&mut ctx, &mut club, None, &hello_in(2, 2, NOW, NOW + 1000, NOW, 1));
        // Before the room existed, or ahead of its own hello: dropped.
        calls.received(&mut ctx, &mut club, None, &hello_in(3, 2, NOW, NOW - 1, NOW, 2));
        let far = NOW + 1000 + CLOCK_WINDOW_MS + 1;
        calls.received(&mut ctx, &mut club, None, &hello_in(4, 2, NOW, far, NOW + 1000, 3));
        assert_eq!(members_of(&calls, 1, room()), [user(2)]);
        // 5 joins; claiming an earlier time on its next hello changes nothing.
        calls.received(&mut ctx, &mut club, None, &hello_in(5, 2, NOW, NOW + 2000, NOW + 2000, 4));
        calls.received(&mut ctx, &mut club, None, &hello_in(5, 2, NOW, NOW, NOW + 3000, 5));
        assert_eq!(members_of(&calls, 1, room()), [user(2), user(5)]);
        assert_eq!(calls.master(pk(1), room()), Some(pk(2)));
        // 10 minutes on (2 and 5 still saying hello), 6 claims it joined at
        // the start: clamped to 2 minutes ago, so not the master.
        let later = NOW + 600_000;
        ctx.now = later;
        calls.received(&mut ctx, &mut club, None, &hello_in(2, 2, NOW, NOW + 1000, later, 6));
        calls.received(&mut ctx, &mut club, None, &hello_in(5, 2, NOW, NOW + 2000, later, 7));
        calls.received(&mut ctx, &mut club, None, &hello_in(6, 2, NOW, NOW, later, 8));
        assert_eq!(members_of(&calls, 1, room()), [user(2), user(5), user(6)]);
        let six = calls.roster(pk(1), room()).into_iter().find(|(_, k, _)| *k == pk(6)).unwrap();
        assert_eq!(six.0, later - CLOCK_WINDOW_MS);
    }

    #[test]
    fn the_roster_keeps_the_first_five_and_expires_from_receipt() {
        let mut calls = new_calls();
        let mut club = club(&[]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        // Six hellos, 7 last. Their `ts` is 100 s old, inside the window.
        for n in 2..8 {
            let hello = hello_in(n, 2, NOW - 100_000, NOW - 100_000, NOW - 100_000, n);
            calls.received(&mut ctx, &mut club, None, &hello);
        }
        let room = room_id(SERVER, pk(2), NOW - 100_000);
        let roster = |calls: &Calls| {
            let mut ids = members_of(calls, 1, room);
            ids.sort();
            ids
        };
        let mut want: Vec<String> = (2..7).map(user).collect();
        want.sort();
        assert_eq!(roster(&calls), want, "the 6th to arrive was kept");
        // Expiry runs from receipt: 30 s after it, not after the old `ts`.
        ctx.now = NOW + HELLO_TIMEOUT_MS;
        calls.tick(&mut ctx, &mut club);
        assert_eq!(roster(&calls).len(), MAX_PARTICIPANTS);
        // 2 leaves: a slot frees up, and 7 gets in.
        let at = ctx.now;
        let mut bye = body(&key(2), at);
        (bye.kind, bye.to, bye.nonce) = (WireKind::Bye, None, hex::encode([20; 16]));
        in_room(&mut bye, 2, NOW - 100_000);
        calls.received(&mut ctx, &mut club, None, &seal(&key(2), &bye));
        calls.received(&mut ctx, &mut club, None, &hello_in(7, 2, NOW - 100_000, at, at, 21));
        assert!(roster(&calls).contains(&user(7)));
        assert!(!roster(&calls).contains(&user(2)));
        // The others' hellos were received at NOW: gone a tick later.
        ctx.now = NOW + HELLO_TIMEOUT_MS + 1;
        calls.tick(&mut ctx, &mut club);
        assert_eq!(roster(&calls), [user(7)]);
    }

    fn rooms_events(events: &[Event]) -> Vec<(String, Vec<VoiceRoom>)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::VoiceRooms { server_id, rooms } => Some((server_id.clone(), rooms.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_rooms_event_per_tick_and_a_kick_purges() {
        let mut calls = new_calls();
        let mut club = club(&[2]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        let state = calls.create(&mut ctx, &mut club, SERVER).unwrap();
        let mine = room_id(SERVER, pk(1), NOW);
        assert_eq!(state.channel_id, hex::encode(mine.as_bytes()));
        // Flipping mute and a peer arriving: one event, at the tick.
        for n in 0..10 {
            calls.set_muted(&mut ctx, &mut club, n % 2 == 0).unwrap();
        }
        calls.received(&mut ctx, &mut club, None, &hello_in(2, 1, NOW, NOW + 5, NOW + 5, 1));
        assert!(rooms_events(ctx.events).is_empty(), "sent before the tick");
        calls.tick(&mut ctx, &mut club);
        calls.tick(&mut ctx, &mut club);
        let seen = rooms_events(ctx.events);
        assert_eq!(seen.len(), 1);
        let room = &seen[0].1[0];
        let ids: Vec<&str> = room.members.iter().map(|m| m.user_id.as_str()).collect();
        assert_eq!(ids, [user(1), user(2)], "by joinedAt");
        assert_eq!((room.master_id.clone(), room.creator_id.clone()), (user(1), user(1)));
        assert_eq!((room.limit, room.name.clone(), room.created_at), (5, None, NOW));
        // 2 is kicked: gone at the next tick, not after the timeout.
        ctx.events.clear();
        club.barred.push(pk(2));
        calls.tick(&mut ctx, &mut club);
        assert_eq!(rooms_events(ctx.events)[0].1[0].members.len(), 1);
        // I'm kicked: I leave, and no rooms are sent to me.
        ctx.events.clear();
        club.barred.push(pk(1));
        calls.tick(&mut ctx, &mut club);
        assert!(!calls.in_call());
        assert!(rooms_events(ctx.events).is_empty());
    }

    /// At most 10 live rooms per server: the first 10 to arrive. An 11th
    /// room's hellos are dropped, and creating one here says `room-cap`.
    #[test]
    fn the_eleventh_room_is_dropped() {
        let mut calls = new_calls();
        let mut club = club(&[]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        for n in 2..12 {
            calls.received(&mut ctx, &mut club, None, &hello_in(n, n, NOW, NOW, NOW, n));
        }
        assert_eq!(calls.rooms_of(pk(1), SERVER).len(), MAX_ROOMS);
        calls.received(&mut ctx, &mut club, None, &hello_in(12, 12, NOW, NOW, NOW, 12));
        assert_eq!(calls.rooms_of(pk(1), SERVER).len(), MAX_ROOMS);
        assert!(calls.room_server(room_id(SERVER, pk(12), NOW)).is_none());
        assert_eq!(calls.create(&mut ctx, &mut club, SERVER).unwrap_err().code, ErrorCode::RoomCap);
        // Joining one of the 10 still works.
        calls.join(&mut ctx, &mut club, SERVER, room()).unwrap();
    }

    /// One room per identity: a hello for another room ends the earlier stay.
    #[test]
    fn a_member_is_in_one_room_at_a_time() {
        let mut calls = new_calls();
        let mut club = club(&[]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        calls.received(&mut ctx, &mut club, None, &hello_in(2, 2, NOW, NOW, NOW, 1));
        calls.received(&mut ctx, &mut club, None, &hello_in(3, 2, NOW, NOW, NOW, 2));
        calls.received(&mut ctx, &mut club, None, &hello_in(3, 3, NOW, NOW, NOW, 3));
        assert_eq!(members_of(&calls, 1, room()), [user(2)]);
        assert_eq!(members_of(&calls, 1, room_id(SERVER, pk(3), NOW)), [user(3)]);
    }

    /// A room at its limit answers `call-full`; the limit can't go below
    /// who is in it, or outside 2–5, and names follow the display rule.
    #[test]
    fn the_limit_and_name_rules() {
        let mut calls = new_calls();
        let mut club = club(&[2]);
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        calls.create(&mut ctx, &mut club, SERVER).unwrap();
        let mine = room_id(SERVER, pk(1), NOW);
        calls.received(&mut ctx, &mut club, None, &hello_in(2, 1, NOW, NOW + 1, NOW + 1, 1));
        let bad = |calls: &mut Calls, ctx: &mut Ctx, club: &mut Club, name: Option<&str>, limit| {
            calls.edit(ctx, club, mine, name.map(String::from), limit).unwrap_err().code
        };
        for limit in [1, 6] {
            assert_eq!(bad(&mut calls, &mut ctx, &mut club, None, Some(limit)), ErrorCode::BadRequest);
        }
        let long = "x".repeat(MAX_ROOM_NAME + 1);
        for name in [long.as_str(), "a\u{202e}b"] {
            assert_eq!(bad(&mut calls, &mut ctx, &mut club, Some(name), None), ErrorCode::BadRequest);
        }
        calls.edit(&mut ctx, &mut club, mine, Some("  Gaming ".into()), Some(2)).unwrap();
        let r = &calls.rooms_of(pk(1), SERVER)[0];
        assert_eq!((r.name.as_deref(), r.limit), (Some("Gaming"), 2));
        // Full at 2: a third gets `full`, and isn't in the room.
        calls.received(&mut ctx, &mut club, None, &hello_in(3, 1, NOW, NOW + 2, NOW + 2, 2));
        assert_eq!(members_of(&calls, 1, mine), [user(1), user(2)]);
        // "" restores the default name.
        calls.edit(&mut ctx, &mut club, mine, Some(String::new()), None).unwrap();
        assert_eq!(calls.rooms_of(pk(1), SERVER)[0].name, None);
        // Not the master, not the owner: not allowed.
        let mut other = new_calls();
        let mut events2 = Vec::new();
        let mut ctx2 = Ctx { key: &key(4), now: NOW, events: &mut events2 };
        other.received(&mut ctx2, &mut club, None, &hello_in(2, 2, NOW, NOW, NOW, 9));
        let code = other.edit(&mut ctx2, &mut club, room(), Some("x".into()), None).unwrap_err().code;
        assert_eq!(code, ErrorCode::NotAllowed);
        // Full, for a joiner too.
        let mut events3 = Vec::new();
        let mut ctx3 = Ctx { key: &key(5), now: NOW + 10, events: &mut events3 };
        let mut third = new_calls();
        let mut limited = body(&key(1), NOW + 10);
        (limited.kind, limited.to, limited.joined_at) = (WireKind::Hello, None, NOW);
        (limited.payload, limited.nonce) = (String::new(), hex::encode([30; 16]));
        limited.meta = Some(WireMeta { name: None, limit: 2 });
        in_room(&mut limited, 1, NOW);
        third.received(&mut ctx3, &mut club, None, &seal(&key(1), &limited));
        third.received(&mut ctx3, &mut club, None, &hello_in(2, 1, NOW, NOW + 1, NOW + 10, 31));
        let code = third.join(&mut ctx3, &mut club, SERVER, mine).unwrap_err().code;
        assert_eq!(code, ErrorCode::CallFull);
    }

    // ---- two (or three) cores over a hub -----------------------------------

    /// Blobs between cores, delivered by [`Mesh::pump`].
    #[derive(Default)]
    struct Hub(std::sync::Mutex<Vec<(PublicKey, PublicKey, Vec<u8>)>>);

    struct Port(Arc<Hub>, PublicKey);

    impl VoiceNet for Port {
        fn send(&self, to: PublicKey, blob: Vec<u8>) {
            self.0 .0.lock().unwrap().push((self.1, to, blob));
        }
    }

    struct Mesh {
        hub: Arc<Hub>,
        cores: Vec<(u8, Calls)>,
        club: Club,
        /// Cores not running yet: nothing reaches them.
        offline: Vec<u8>,
    }

    impl Mesh {
        fn new(ids: &[u8], owner: u8) -> Mesh {
            let hub = Arc::new(Hub::default());
            let cores = ids
                .iter()
                .map(|n| {
                    let mut calls = Calls::default();
                    calls.set_net(Arc::new(Port(hub.clone(), pk(*n))));
                    (*n, calls)
                })
                .collect();
            let mut club = club(ids);
            club.owner = Some(pk(owner));
            Mesh { hub, cores, club, offline: Vec::new() }
        }

        fn run<T>(&mut self, n: u8, now: u64, f: impl FnOnce(&mut Calls, &mut Ctx, &mut Club) -> T) -> T {
            let mut club = self.club.clone();
            club.all.retain(|k| *k != pk(n));
            let (_, calls) = self.cores.iter_mut().find(|(id, _)| *id == n).unwrap();
            let mut events = Vec::new();
            let key = key(n);
            let mut ctx = Ctx { key: &key, now, events: &mut events };
            f(calls, &mut ctx, &mut club)
        }

        fn pump(&mut self, now: u64) {
            loop {
                let batch = std::mem::take(&mut *self.hub.0.lock().unwrap());
                if batch.is_empty() {
                    return;
                }
                for (from, to, blob) in batch {
                    let up = |n: &u8| !self.offline.contains(n);
                    if let Some(n) = self.cores.iter().map(|(n, _)| *n).filter(up).find(|n| pk(*n) == to && pk(*n) != from) {
                        self.run(n, now, |calls, ctx, club| calls.received(ctx, club, Some(from), &blob));
                    }
                }
            }
        }

        fn view(&mut self, n: u8) -> Vec<VoiceRoom> {
            self.run(n, NOW, |calls, ctx, _| calls.rooms_of(ctx.me(), SERVER))
        }
    }

    /// Two cores agree on the master after a hand-off, and the name the
    /// first master set stays; the limit and name come from the master.
    #[test]
    fn cores_agree_on_the_master_after_a_hand_off() {
        let mut mesh = Mesh::new(&[1, 2, 3], 9);
        let state = mesh.run(1, NOW, |c, ctx, club| c.create(ctx, club, SERVER)).unwrap();
        mesh.pump(NOW);
        let room = model::parse_hex32(&state.channel_id).map(ChannelId::from_bytes).unwrap();
        mesh.run(2, NOW + 1000, |c, ctx, club| c.join(ctx, club, SERVER, room)).unwrap();
        mesh.pump(NOW + 1000);
        mesh.run(3, NOW + 2000, |c, ctx, club| c.join(ctx, club, SERVER, room)).unwrap();
        mesh.pump(NOW + 2000);
        mesh.run(1, NOW + 2500, |c, ctx, club| c.edit(ctx, club, room, Some("Gaming".into()), Some(4)))
            .unwrap();
        mesh.pump(NOW + 2500);
        for n in [1, 2, 3] {
            let rooms = mesh.view(n);
            assert_eq!(rooms.len(), 1, "core {n}");
            assert_eq!((rooms[0].master_id.clone(), rooms[0].members.len()), (user(1), 3), "core {n}");
            assert_eq!((rooms[0].name.as_deref(), rooms[0].limit), (Some("Gaming"), 4), "core {n}");
        }
        // The master leaves: 2 (joined earliest of the rest) on both cores.
        mesh.run(1, NOW + 3000, |c, ctx, club| c.leave(ctx, club));
        mesh.pump(NOW + 3000);
        for n in [2, 3] {
            let rooms = mesh.view(n);
            assert_eq!((rooms[0].master_id.clone(), rooms[0].members.len()), (user(2), 2), "core {n}");
            assert_eq!((rooms[0].name.as_deref(), rooms[0].limit), (Some("Gaming"), 4), "core {n}");
        }
        // The new master's hello carries the name it inherited.
        mesh.run(2, NOW + 15_000, |c, ctx, club| c.tick(ctx, club));
        mesh.pump(NOW + 15_000);
        assert_eq!(mesh.view(3)[0].name.as_deref(), Some("Gaming"));
    }

    /// A core that comes online 5 minutes after a room started lists it and
    /// all its members, agrees on the master whatever order their hellos
    /// arrive in, and can join.
    #[test]
    fn a_core_that_comes_online_later_sees_the_room() {
        let mut mesh = Mesh::new(&[1, 2, 3], 9);
        mesh.offline.push(3);
        let state = mesh.run(1, NOW, |c, ctx, club| c.create(ctx, club, SERVER)).unwrap();
        let room = model::parse_hex32(&state.channel_id).map(ChannelId::from_bytes).unwrap();
        mesh.pump(NOW);
        mesh.run(2, NOW + 1000, |c, ctx, club| c.join(ctx, club, SERVER, room)).unwrap();
        mesh.pump(NOW + 1000);
        // Ticks every 10 s for 5 minutes, 2 first, then 3 comes online.
        let mut at = NOW + 1000;
        while at < NOW + 300_000 {
            at += HELLO_EVERY_MS;
            if at >= NOW + 300_000 {
                mesh.offline.clear();
            }
            for n in [2, 1] {
                mesh.run(n, at, |c, ctx, club| c.tick(ctx, club));
                mesh.pump(at);
            }
        }
        for n in [1, 2, 3] {
            let rooms = mesh.run(n, at, |c, ctx, _| c.rooms_of(ctx.me(), SERVER));
            assert_eq!(rooms.len(), 1, "core {n}");
            let ids: Vec<&str> = rooms[0].members.iter().map(|m| m.user_id.as_str()).collect();
            assert_eq!(ids, [user(1), user(2)], "core {n}");
            assert_eq!(rooms[0].master_id, user(1), "core {n}");
        }
        mesh.run(3, at, |c, ctx, club| c.join(ctx, club, SERVER, room)).unwrap();
        mesh.pump(at);
        for n in [1, 2] {
            assert_eq!(mesh.run(n, at, |c, ctx, _| c.rooms_of(ctx.me(), SERVER))[0].members.len(), 3);
        }
    }

    /// The owner's edit goes to the master, which applies it; a member's
    /// edit is refused locally, and a forged one sent anyway is dropped.
    #[test]
    fn only_the_master_and_the_owner_edit() {
        let mut mesh = Mesh::new(&[1, 2, 9], 9);
        let state = mesh.run(1, NOW, |c, ctx, club| c.create(ctx, club, SERVER)).unwrap();
        let room = model::parse_hex32(&state.channel_id).map(ChannelId::from_bytes).unwrap();
        mesh.pump(NOW);
        mesh.run(2, NOW + 1000, |c, ctx, club| c.join(ctx, club, SERVER, room)).unwrap();
        mesh.pump(NOW + 1000);
        let code = mesh
            .run(2, NOW + 1100, |c, ctx, club| c.edit(ctx, club, room, Some("Mine".into()), None))
            .unwrap_err()
            .code;
        assert_eq!(code, ErrorCode::NotAllowed);
        // 2 sends an edit to the master anyway: dropped.
        let mut forged = body(&key(2), NOW + 1200);
        (forged.kind, forged.to, forged.nonce) = (WireKind::Edit, Some(user(1)), hex::encode([40; 16]));
        forged.payload = r#"{"name":"Mine"}"#.into();
        in_room(&mut forged, 1, NOW);
        let blob = seal(&key(2), &forged);
        mesh.run(1, NOW + 1200, |c, ctx, club| c.received(ctx, club, Some(pk(2)), &blob));
        mesh.pump(NOW + 1200);
        assert_eq!(mesh.view(2)[0].name, None);
        // The owner (9, not in the room): goes through the master.
        mesh.run(9, NOW + 1300, |c, ctx, club| c.edit(ctx, club, room, Some("Owner's".into()), Some(3)))
            .unwrap();
        mesh.pump(NOW + 1300);
        for n in [1, 2, 9] {
            let rooms = mesh.view(n);
            assert_eq!((rooms[0].name.as_deref(), rooms[0].limit), (Some("Owner's"), 3), "core {n}");
        }
    }

    #[test]
    fn fingerprints_are_read_from_the_sdp() {
        assert!(fingerprints("v=0\r\na=setup:actpass\r\n").is_empty());
        assert_eq!(
            fingerprints("a=fingerprint:sha-256 AA\na=fingerprint: sha-1 BB \n"),
            ["sha-256 AA", "sha-1 BB"]
        );
    }
}
