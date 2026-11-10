//! Voice calls (voice v1, piece B): who is in a call, and the WebRTC
//! signalling between members. The media never passes through the core;
//! the UI's WebRTC engine (piece D) does that, peer to peer.
//!
//! - Every core-to-core message is an envelope signed by the sender's
//!   identity key under [`SIGNAL_PREFIX`]. The signature covers the whole
//!   body, the SDP and its `a=fingerprint` included, so a verified offer or
//!   answer binds the DTLS fingerprint to the sender's MVS identity.
//! - It fails closed: an envelope that doesn't verify, is outside the clock
//!   window, is replayed, comes from someone who couldn't join that call, or
//!   is an offer or answer without a fingerprint is dropped (logged at
//!   debug). `call.signal` only ever carries verified signalling.
//! - Presence: a participant says `hello` to the server's reachable members
//!   every [`HELLO_EVERY_MS`] and `bye` on leaving; one whose hellos stop is
//!   dropped after [`HELLO_TIMEOUT_MS`]. At most [`MAX_PARTICIPANTS`]: a
//!   full call answers a newcomer's hello with `full`.
//!
//! - Rooms (the voice-room amendment): calls happen only in each server's
//!   one voice room, whose ID is derived ([`room_id`]), never in a text
//!   channel. Every member sees who is in it (`voice.room`), built only
//!   from verified hellos: identity and muted, at most
//!   [`MAX_PARTICIPANTS`] (the first to arrive), expiring
//!   [`HELLO_TIMEOUT_MS`] after we received the last one, and sent at most
//!   once per room per tick.
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
/// The mesh cap: a 6th gets `call-full`.
pub const MAX_PARTICIPANTS: usize = 5;
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
/// The voice room's `derive_key` context. Frozen.
const ROOM_CTX: &str = "mvs/voice-room/v1";

/// `server`'s one voice room. Every member derives the same ID, and it
/// can't be a record-made channel's.
pub fn room_id(server: ServerId) -> ChannelId {
    ChannelId::from_bytes(blake3::derive_key(ROOM_CTX, server.as_bytes()))
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
    /// `channel` is `server`'s voice room ([`room_id`]), `server` is one of
    /// mine with an accepted genesis, and `who` is a member, not kicked.
    fn may_call(&mut self, who: PublicKey, server: ServerId, channel: ChannelId) -> bool;
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
}

/// On the wire: the body as sent, and the signature over
/// `SIGNAL_PREFIX ‖ body`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    body: String,
    sig: String,
}

/// A verified envelope.
#[derive(Debug)]
struct Opened {
    kind: WireKind,
    server: ServerId,
    channel: ChannelId,
    from: PublicKey,
    to: Option<PublicKey>,
    nonce: [u8; 16],
    muted: bool,
    payload: String,
}

fn seal(key: &SigningKey, body: &Body) -> Vec<u8> {
    let body = serde_json::to_string(body).expect("a body serializes");
    let sig = key.sign(&[SIGNAL_PREFIX, body.as_bytes()].concat());
    let envelope = Envelope { body, sig: hex::encode(sig.to_bytes()) };
    serde_json::to_vec(&envelope).expect("an envelope serializes")
}

/// `None` unless the signature verifies, the version is 1 and the time is
/// within [`CLOCK_WINDOW_MS`] of `now`.
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
    if body.v != 1 || body.ts.abs_diff(now) > CLOCK_WINDOW_MS {
        return None;
    }
    let to = match &body.to {
        Some(to) => Some(PublicKey::from_bytes(model::parse_hex32(to)?)),
        None => None,
    };
    Some(Opened {
        kind: body.kind,
        server: ServerId::from_bytes(model::parse_hex32(&body.server)?),
        channel: ChannelId::from_bytes(model::parse_hex32(&body.channel)?),
        from: PublicKey::from_bytes(from),
        to,
        nonce: hex::decode(&body.nonce).ok()?.try_into().ok()?,
        muted: body.muted,
        payload: body.payload,
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

/// My call.
struct Mine {
    server: ServerId,
    channel: ChannelId,
    muted: bool,
    joined_at: u64,
}

#[derive(Clone, Copy)]
struct Peer {
    muted: bool,
    seen: u64,
}

/// Who else is in a room, from their verified hellos (rooms I'm not in
/// too: every member sees the roster, and a 6th learns it's full before
/// joining). At most [`MAX_PARTICIPANTS`] peers, the first to arrive.
struct Room {
    server: ServerId,
    peers: BTreeMap<PublicKey, Peer>,
}

#[derive(Default)]
pub(crate) struct Calls {
    net: Option<Arc<dyn VoiceNet>>,
    mine: Option<Mine>,
    rooms: HashMap<ChannelId, Room>,
    nonces: VecDeque<(u64, [u8; 16])>,
    seen: HashSet<[u8; 16]>,
    last_hello: u64,
    /// Servers whose room roster changed since the last tick: one
    /// `voice.room` each, at the next tick (coalesced).
    dirty: BTreeSet<ServerId>,
}

impl Calls {
    pub fn set_net(&mut self, net: Arc<dyn VoiceNet>) {
        self.net = Some(net);
    }

    pub fn in_call(&self) -> bool {
        self.mine.is_some()
    }

    fn net(&self) -> CoreResult<&Arc<dyn VoiceNet>> {
        self.net.as_ref().ok_or_else(|| ErrorCode::NotImplemented.into())
    }

    fn others(&self, channel: ChannelId) -> usize {
        self.rooms.get(&channel).map_or(0, |room| room.peers.len())
    }

    pub fn join(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        server: ServerId,
        channel: ChannelId,
    ) -> CoreResult<CallState> {
        if self.net()?.relay_only() {
            return Err(ErrorCode::RelayOnly.into());
        }
        if !members.may_call(ctx.me(), server, channel) {
            return Err(ErrorCode::NotAllowed.into());
        }
        match self.mine.as_ref().map(|m| m.channel) {
            Some(current) if current == channel => {
                return Ok(self.state(ctx.me()).expect("in a call"));
            }
            Some(_) => self.leave(ctx, members),
            None => {}
        }
        self.expire(ctx.now, members);
        if self.others(channel) >= MAX_PARTICIPANTS {
            return Err(ErrorCode::CallFull.into());
        }
        self.mine = Some(Mine { server, channel, muted: false, joined_at: ctx.now });
        self.dirty.insert(server);
        self.hello(ctx, members, None);
        let state = self.state(ctx.me()).expect("in a call");
        ctx.events.push(Event::CallState(state.clone()));
        Ok(state)
    }

    pub fn leave(&mut self, ctx: &mut Ctx, members: &mut dyn Members) {
        let Some(mine) = self.mine.take() else { return };
        self.dirty.insert(mine.server);
        let body = self.body(ctx, WireKind::Bye, mine.server, mine.channel, None);
        self.broadcast(ctx, members, mine.server, mine.channel, body);
        ctx.events.push(Event::CallState(CallState {
            channel_id: model::hex_id(mine.channel.as_bytes()),
            participants: Vec::new(),
            full: false,
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
        let (server, channel) = (mine.server, mine.channel);
        let mut body = self.body(ctx, kind.into(), server, channel, Some(to));
        body.payload = payload;
        self.send(ctx, to, &body);
        Ok(())
    }

    /// Every [`crate::SYNC_EVERY`]: drops peers gone quiet or no longer
    /// allowed (a kick within one tick), says hello again when due, then
    /// sends one `voice.room` per room whose roster changed, only for
    /// servers I'm still a member of.
    pub fn tick(&mut self, ctx: &mut Ctx, members: &mut dyn Members) {
        self.tick_calls(ctx, members);
        let me = ctx.me();
        for server in std::mem::take(&mut self.dirty) {
            if members.may_call(me, server, room_id(server)) {
                ctx.events.push(Event::VoiceRoom(self.room(me, server)));
            }
        }
    }

    fn tick_calls(&mut self, ctx: &mut Ctx, members: &mut dyn Members) {
        if let Some((server, channel)) = self.mine.as_ref().map(|m| (m.server, m.channel)) {
            // Kicked, or relay-only turned on mid-call (the media would go
            // direct): leave, with a bye.
            let relay_only = self.net.as_ref().is_some_and(|net| net.relay_only());
            if relay_only || !members.may_call(ctx.me(), server, channel) {
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
        let Some(signal) = open(blob, ctx.now) else {
            return tracing::debug!("voice: dropped an envelope that doesn't verify");
        };
        let me = ctx.me();
        if from.is_some_and(|from| from != signal.from)
            || signal.from == me
            || signal.to.is_some_and(|to| to != me)
        {
            return tracing::debug!("voice: dropped a misaddressed envelope");
        }
        // Bound to its server's room: any other channel ID is dropped.
        if signal.channel != room_id(signal.server) {
            return tracing::debug!("voice: dropped an envelope for no room");
        }
        if !self.fresh(signal.nonce, ctx.now) {
            return tracing::debug!("voice: dropped a replayed envelope");
        }
        if !members.may_call(signal.from, signal.server, signal.channel) {
            return tracing::debug!("voice: dropped an envelope from someone not allowed");
        }
        let in_this_call = self
            .mine
            .as_ref()
            .is_some_and(|m| (m.server, m.channel) == (signal.server, signal.channel));
        match signal.kind {
            WireKind::Hello => self.on_hello(ctx, members, &signal, in_this_call),
            WireKind::Bye => {
                let gone = self
                    .rooms
                    .get_mut(&signal.channel)
                    .is_some_and(|room| room.peers.remove(&signal.from).is_some());
                if gone {
                    self.dirty.insert(signal.server);
                }
                if gone && in_this_call {
                    ctx.events.push(Event::CallState(self.state(me).expect("in a call")));
                }
            }
            WireKind::Full => {
                // A full call refused us: only believed from someone already
                // in that call, and only while we are still joining.
                let joining = self.mine.as_ref().is_some_and(|m| {
                    in_this_call && ctx.now.saturating_sub(m.joined_at) <= JOINING_MS
                });
                let known = self
                    .rooms
                    .get(&signal.channel)
                    .is_some_and(|room| room.peers.contains_key(&signal.from));
                if !joining || !known || signal.to != Some(me) {
                    return tracing::debug!("voice: ignored a full from outside the call");
                }
                self.mine = None;
                self.dirty.insert(signal.server);
                ctx.events.push(Event::CallState(CallState {
                    channel_id: model::hex_id(signal.channel.as_bytes()),
                    participants: Vec::new(),
                    full: true,
                }));
            }
            WireKind::Offer | WireKind::Answer | WireKind::Ice => {
                let known = self
                    .rooms
                    .get(&signal.channel)
                    .is_some_and(|room| room.peers.contains_key(&signal.from));
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

    fn on_hello(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        signal: &Opened,
        in_this_call: bool,
    ) {
        let me = ctx.me();
        let room = self
            .rooms
            .entry(signal.channel)
            .or_insert_with(|| Room { server: signal.server, peers: BTreeMap::new() });
        let before = room.peers.get(&signal.from).copied();
        // A room I'm not in keeps the first 5 to arrive; later ones are
        // ignored until a slot frees up.
        if before.is_none() && !in_this_call && room.peers.len() >= MAX_PARTICIPANTS {
            return tracing::debug!("voice: ignored a hello for a full room");
        }
        // Me and 4 others: full. (Calls I'm not in only count.)
        let present = room.peers.len() + usize::from(in_this_call);
        if before.is_none() && in_this_call && present >= MAX_PARTICIPANTS {
            let body =
                self.body(ctx, WireKind::Full, signal.server, signal.channel, Some(signal.from));
            return self.send(ctx, signal.from, &body);
        }
        let new = before.is_none();
        let changed = before.is_none_or(|p| p.muted != signal.muted);
        // Expiry runs from when we received it, never the sender's `ts`.
        room.peers.insert(signal.from, Peer { muted: signal.muted, seen: ctx.now });
        if changed {
            self.dirty.insert(signal.server);
        }
        if in_this_call {
            if new {
                // So the newcomer learns of us now, not at our next hello.
                self.hello(ctx, members, Some(signal.from));
            }
            if changed {
                ctx.events.push(Event::CallState(self.state(me).expect("in a call")));
            }
        }
    }

    /// Hello to `to`, or to the whole audience (then it counts as our
    /// periodic one).
    fn hello(&mut self, ctx: &mut Ctx, members: &mut dyn Members, to: Option<PublicKey>) {
        let Some(mine) = &self.mine else { return };
        let (server, channel, muted) = (mine.server, mine.channel, mine.muted);
        let mut body = self.body(ctx, WireKind::Hello, server, channel, to);
        body.muted = muted;
        match to {
            Some(to) => self.send(ctx, to, &body),
            None => {
                self.last_hello = ctx.now;
                self.broadcast(ctx, members, server, channel, body);
            }
        }
    }

    fn body(
        &self,
        ctx: &Ctx,
        kind: WireKind,
        server: ServerId,
        channel: ChannelId,
        to: Option<PublicKey>,
    ) -> Body {
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        Body {
            v: 1,
            kind,
            server: model::hex_id(server.as_bytes()),
            channel: model::hex_id(channel.as_bytes()),
            from: model::hex_id(ctx.me().as_bytes()),
            to: to.map(|to| model::hex_id(to.as_bytes())),
            ts: ctx.now,
            nonce: hex::encode(nonce),
            muted: false,
            payload: String::new(),
        }
    }

    fn send(&self, ctx: &Ctx, to: PublicKey, body: &Body) {
        if let Some(net) = &self.net {
            net.send(to, seal(ctx.key, body));
        }
    }

    /// To the server's reachable members who could join this call
    /// themselves: a call in an 18+ channel isn't shown to anyone else.
    fn broadcast(
        &self,
        ctx: &Ctx,
        members: &mut dyn Members,
        server: ServerId,
        channel: ChannelId,
        body: Body,
    ) {
        let Some(net) = &self.net else { return };
        let blob = seal(ctx.key, &body);
        for to in members.audience(server) {
            if members.may_call(to, server, channel) {
                net.send(to, blob.clone());
            }
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

    /// Drops peers gone quiet or no longer allowed; true if my call changed.
    fn expire(&mut self, now: u64, members: &mut dyn Members) -> bool {
        let mine = self.mine.as_ref().map(|m| m.channel);
        let mut changed = false;
        for (channel, room) in &mut self.rooms {
            let before = room.peers.len();
            let server = room.server;
            room.peers.retain(|key, peer| {
                now.saturating_sub(peer.seen) <= HELLO_TIMEOUT_MS
                    && members.may_call(*key, server, *channel)
            });
            if room.peers.len() != before {
                self.dirty.insert(server);
            }
            changed |= Some(*channel) == mine && room.peers.len() != before;
        }
        self.rooms.retain(|_, room| !room.peers.is_empty());
        changed
    }

    /// My call: me first, then the others by key.
    fn state(&self, me: PublicKey) -> Option<CallState> {
        let mine = self.mine.as_ref()?;
        let mut participants =
            vec![CallParticipant { identity: model::hex_id(me.as_bytes()), muted: mine.muted }];
        if let Some(room) = self.rooms.get(&mine.channel) {
            participants.extend(room.peers.iter().map(|(key, peer)| CallParticipant {
                identity: model::hex_id(key.as_bytes()),
                muted: peer.muted,
            }));
        }
        let full = participants.len() >= MAX_PARTICIPANTS;
        Some(CallState { channel_id: model::hex_id(mine.channel.as_bytes()), participants, full })
    }

    /// `getVoiceRoom`: drops who went quiet or was kicked, then the roster
    /// (whether I may see it is the caller's check).
    pub fn voice_room(
        &mut self,
        ctx: &mut Ctx,
        members: &mut dyn Members,
        server: ServerId,
    ) -> VoiceRoom {
        if self.expire(ctx.now, members) {
            if let Some(state) = self.state(ctx.me()) {
                ctx.events.push(Event::CallState(state));
            }
        }
        self.room(ctx.me(), server)
    }

    /// `server`'s room: me first when I'm in it, then the others by key; at
    /// most [`MAX_PARTICIPANTS`]. Identity and muted only.
    fn room(&self, me: PublicKey, server: ServerId) -> VoiceRoom {
        let room = room_id(server);
        let mut participants = Vec::new();
        if let Some(mine) = self.mine.as_ref().filter(|m| m.server == server) {
            participants.push(VoiceParticipant {
                user_id: model::hex_id(me.as_bytes()),
                muted: mine.muted,
            });
        }
        if let Some(peers) = self.rooms.get(&room) {
            participants.extend(peers.peers.iter().map(|(key, peer)| VoiceParticipant {
                user_id: model::hex_id(key.as_bytes()),
                muted: peer.muted,
            }));
        }
        participants.truncate(MAX_PARTICIPANTS);
        VoiceRoom {
            server_id: model::hex_id(server.as_bytes()),
            room_id: model::hex_id(room.as_bytes()),
            participants,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn body(from: &SigningKey, ts: u64) -> Body {
        Body {
            v: 1,
            kind: WireKind::Offer,
            server: hex::encode([1; 32]),
            channel: hex::encode(room_id(SERVER).as_bytes()),
            from: hex::encode(from.verifying_key().to_bytes()),
            to: Some(hex::encode([3; 32])),
            ts,
            nonce: hex::encode([4; 16]),
            muted: false,
            payload: "v=0\r\na=fingerprint:sha-256 AB:CD\r\n".into(),
        }
    }

    #[test]
    fn a_sealed_envelope_opens() {
        let opened = open(&seal(&key(9), &body(&key(9), NOW)), NOW + 1000).unwrap();
        assert_eq!(opened.kind, WireKind::Offer);
        assert_eq!(opened.from.as_bytes(), &key(9).verifying_key().to_bytes());
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

    #[test]
    fn only_version_1_inside_the_clock_window_opens() {
        let mut v2 = body(&key(9), NOW);
        v2.v = 2;
        assert!(open(&seal(&key(9), &v2), NOW).is_none());
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

    /// Everyone may call except `barred`.
    struct Club {
        all: Vec<PublicKey>,
        barred: Vec<PublicKey>,
    }

    impl Members for Club {
        fn may_call(&mut self, who: PublicKey, _: ServerId, _: ChannelId) -> bool {
            !self.barred.contains(&who)
        }

        fn audience(&mut self, _: ServerId) -> Vec<PublicKey> {
            self.all.clone()
        }
    }

    fn pk(n: u8) -> PublicKey {
        PublicKey::from_bytes(key(n).verifying_key().to_bytes())
    }

    fn signal(from: u8, kind: WireKind, to: Option<u8>, ts: u64, nonce: u8) -> Vec<u8> {
        let mut body = body(&key(from), ts);
        body.kind = kind;
        body.to = to.map(|to| hex::encode(pk(to).as_bytes()));
        body.nonce = hex::encode([nonce; 16]);
        body.payload.clear();
        seal(&key(from), &body)
    }

    const SERVER: ServerId = ServerId::from_bytes([1; 32]);
    fn room() -> ChannelId {
        room_id(SERVER)
    }

    #[test]
    fn hello_and_bye_reach_only_those_who_may_call() {
        let net = Arc::new(Net::default());
        let mut calls = Calls::default();
        calls.set_net(net.clone());
        let mut club = Club { all: vec![pk(2), pk(3)], barred: vec![pk(3)] };
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        calls.join(&mut ctx, &mut club, SERVER, room()).unwrap();
        calls.leave(&mut ctx, &mut club);
        assert_eq!(*net.0.lock().unwrap(), [pk(2), pk(2)], "an 18+ call shown to a minor");
    }

    #[test]
    fn full_counts_only_from_the_call_and_only_while_joining() {
        let joined = || {
            let mut calls = Calls::default();
            calls.set_net(Arc::new(Net::default()));
            let mut club = Club { all: vec![pk(2), pk(4)], barred: vec![] };
            let mut events = Vec::new();
            let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
            // Peer 2 is in the call; peer 4 isn't.
            calls.received(&mut ctx, &mut club, None, &signal(2, WireKind::Hello, None, NOW, 1));
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

    fn rooms(events: &[Event]) -> Vec<VoiceRoom> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::VoiceRoom(room) => Some(room.clone()),
                _ => None,
            })
            .collect()
    }

    fn user(n: u8) -> String {
        hex::encode(pk(n).as_bytes())
    }

    fn users(room: &VoiceRoom) -> Vec<String> {
        room.participants.iter().map(|p| p.user_id.clone()).collect()
    }

    #[test]
    fn the_room_id_is_derived_per_server() {
        assert_eq!(room_id(SERVER), room_id(ServerId::from_bytes([1; 32])));
        assert_ne!(room_id(SERVER), room_id(ServerId::from_bytes([9; 32])));
        assert_ne!(room_id(SERVER), model::default_channel(SERVER));
    }

    #[test]
    fn a_hello_bound_to_another_channel_is_dropped() {
        let mut calls = Calls::default();
        calls.set_net(Arc::new(Net::default()));
        let mut club = Club { all: vec![], barred: vec![] };
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        let mut hello = body(&key(2), NOW);
        (hello.kind, hello.to, hello.channel) = (WireKind::Hello, None, hex::encode([2; 32]));
        hello.payload.clear();
        calls.received(&mut ctx, &mut club, None, &seal(&key(2), &hello));
        calls.tick(&mut ctx, &mut club);
        assert!(calls.room(pk(1), SERVER).participants.is_empty());
        assert!(rooms(&events).is_empty());
    }

    #[test]
    fn the_roster_keeps_the_first_five_and_expires_from_receipt() {
        let mut calls = Calls::default();
        calls.set_net(Arc::new(Net::default()));
        let mut club = Club { all: vec![], barred: vec![] };
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        // Six hellos, 7 last. Their `ts` is 100 s old, inside the window.
        for n in 2..8 {
            let hello = signal(n, WireKind::Hello, None, NOW - 100_000, n);
            calls.received(&mut ctx, &mut club, None, &hello);
        }
        let first_five: Vec<String> = (2..7).map(user).collect();
        let roster = |calls: &Calls| {
            let mut ids = users(&calls.room(pk(1), SERVER));
            ids.sort();
            ids
        };
        let mut want = first_five.clone();
        want.sort();
        assert_eq!(roster(&calls), want, "the 6th to arrive was kept");
        // Expiry runs from receipt: 30 s after it, not after the old `ts`.
        ctx.now = NOW + HELLO_TIMEOUT_MS;
        calls.tick(&mut ctx, &mut club);
        assert_eq!(roster(&calls).len(), MAX_PARTICIPANTS);
        // 2 leaves: a slot frees up, and 7 gets in.
        let at = ctx.now;
        calls.received(&mut ctx, &mut club, None, &signal(2, WireKind::Bye, None, at, 20));
        calls.received(&mut ctx, &mut club, None, &signal(7, WireKind::Hello, None, at, 21));
        assert!(roster(&calls).contains(&user(7)));
        assert!(!roster(&calls).contains(&user(2)));
        // The others' hellos were received at NOW: gone a tick later.
        ctx.now = NOW + HELLO_TIMEOUT_MS + 1;
        calls.tick(&mut ctx, &mut club);
        assert_eq!(roster(&calls), [user(7)]);
    }

    #[test]
    fn one_room_event_per_tick_and_only_while_a_member() {
        let mut calls = Calls::default();
        calls.set_net(Arc::new(Net::default()));
        let mut club = Club { all: vec![pk(2)], barred: vec![] };
        let mut events = Vec::new();
        let mut ctx = Ctx { key: &key(1), now: NOW, events: &mut events };
        calls.join(&mut ctx, &mut club, SERVER, room()).unwrap();
        // Flipping mute and a peer arriving: one event, at the tick.
        for n in 0..10 {
            calls.set_muted(&mut ctx, &mut club, n % 2 == 0).unwrap();
        }
        calls.received(&mut ctx, &mut club, None, &signal(2, WireKind::Hello, None, NOW, 1));
        assert!(rooms(ctx.events).is_empty(), "sent before the tick");
        calls.tick(&mut ctx, &mut club);
        calls.tick(&mut ctx, &mut club);
        let seen = rooms(ctx.events);
        assert_eq!(seen.len(), 1);
        assert_eq!(users(&seen[0]), [user(1), user(2)], "me first");
        assert_eq!(seen[0].room_id, hex::encode(room().as_bytes()));
        assert!(!seen[0].participants[0].muted);
        // 2 is kicked: gone at the next tick, not after the timeout.
        ctx.events.clear();
        club.barred.push(pk(2));
        calls.tick(&mut ctx, &mut club);
        assert_eq!(users(&rooms(ctx.events)[0]), [user(1)]);
        // I'm kicked: I leave, and no roster is sent to me.
        ctx.events.clear();
        club.barred.push(pk(1));
        calls.tick(&mut ctx, &mut club);
        assert!(!calls.in_call());
        assert!(rooms(ctx.events).is_empty());
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
