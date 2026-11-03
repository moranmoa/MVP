//! One member's view of one channel's epochs.
//!
//! Rules, all decided by the set of rotations held and the set `authority`
//! accepts, never by the order either arrived in:
//! - Every well-formed rotation of the channel is stored, whether or not its
//!   author may rotate. Only `authority` decides who may rotate, and it can
//!   change its mind (a revoke voids a moderator's concurrent records), so the
//!   caller passes the rotations it currently **accepts** with
//!   [`ChannelKeys::set_accepted`] whenever that set changes. Deny by
//!   default: a rotation authority hasn't ruled on yet (not processed,
//!   pending on missing records, or a kind it doesn't know) is never sent
//!   with.
//! - A rotation is *linked* when it is epoch 0, or its `prev` is linked and
//!   one epoch below it. A rotation whose `prev` hasn't arrived yet waits.
//!   Linking only checks the epoch counter, so a rotation that isn't accepted
//!   still links the rotations built on it: their keys are fresh and owe
//!   nothing to it.
//! - The head (the key to send with) is the linked, accepted rotation with
//!   the highest epoch; between rotations of that epoch the lowest record ID
//!   wins, the same tie-break `authority` uses.
//! - Keys of every rotation we were a recipient of are kept, accepted or not,
//!   so messages from any epoch we belonged to stay readable. [`Decrypted`]
//!   says which rotation opened a message and whether it is accepted; the
//!   caller must label a message opened under a non-accepted rotation as
//!   untrusted. A member who was offline through several rotations just
//!   applies them all.
//! - If the head's recipients include someone no longer in the channel (a
//!   kick, or a revoke that took away access), sending is refused until a
//!   moderator rotates.
//! - Memory is bounded. Anyone can sign rotation records, so a rotation that
//!   `authority` has never accepted and that no linked rotation builds on is
//!   *on probation*: when more than the limit ([`MAX_UNACCEPTED`] by
//!   default) are, the oldest by arrival is dropped, key and all, and its ID
//!   returned so sync can fetch it again if it turns out to matter. Every
//!   rotation authority ever accepted, and every ancestor of a linked one, is
//!   kept.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use rand_core::{CryptoRng, RngCore};
use records::{ChannelId, PublicKey, RecordId, ServerId};

use crate::{open, seal, EpochKey, Error, MemberSecret, MessageHeader, Rotation};

/// How many rotations a channel holds on probation (never accepted, nothing
/// linked builds on them) before the oldest is dropped. At the 16 KiB record
/// limit that is at most 1 MiB per channel.
pub const MAX_UNACCEPTED: usize = 64;

/// What [`ChannelKeys::apply`] did with a rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Stored, and we now hold its key.
    Key,
    /// Stored; we aren't a recipient.
    NotRecipient,
    /// Stored, but the wrap addressed to us doesn't open. Other members can
    /// still use the rotation, so it counts like any other; we hold no key.
    WrapInvalid,
    /// Already held.
    Duplicate,
}

/// Whether the channel needs a new rotation for the given member set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RotationNeed {
    None,
    /// No usable rotation yet: the channel has no key.
    Initial,
    /// The head key is held by people who were removed. Nobody can send
    /// until a moderator rotates.
    Required {
        removed: Vec<PublicKey>,
    },
    /// Members who can't read the head (joined, got access, or their wrap
    /// didn't open). Sending still works; they need a rotation to read.
    Add {
        missing: Vec<PublicKey>,
    },
}

/// A decrypted message and the rotation whose key opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decrypted {
    pub plaintext: Vec<u8>,
    pub rotation: RecordId,
    pub epoch: u64,
    /// Whether `authority` accepts that rotation. If not, the message was
    /// sealed under a key nobody authorised: show it as untrusted.
    pub accepted: bool,
}

pub struct ChannelKeys {
    server: ServerId,
    channel: ChannelId,
    me: Arc<MemberSecret>,
    rotations: HashMap<RecordId, Rotation>,
    keys: HashMap<RecordId, EpochKey>,
    linked: BTreeSet<RecordId>,
    /// Rotations whose `prev` isn't linked yet, by that `prev`.
    waiting: HashMap<RecordId, Vec<RecordId>>,
    /// Rotations `authority` accepts. Only these can be head.
    accepted: HashSet<RecordId>,
    /// Rotations `authority` has accepted at some point. Never on probation.
    ever_accepted: HashSet<RecordId>,
    /// How many linked rotations build on each rotation.
    children: HashMap<RecordId, usize>,
    /// Arrival number of each held rotation.
    arrival: HashMap<RecordId, u64>,
    next_arrival: u64,
    /// Rotations on probation, oldest first.
    probation: BTreeSet<(u64, RecordId)>,
    probation_limit: usize,
    head: Option<RecordId>,
}

impl ChannelKeys {
    /// `me` is shared by every channel of the client; see [`MemberSecret`].
    pub fn new(server: ServerId, channel: ChannelId, me: Arc<MemberSecret>) -> ChannelKeys {
        Self::with_limit(server, channel, me, MAX_UNACCEPTED)
    }

    /// Like [`Self::new`], with at most `limit` rotations on probation.
    pub fn with_limit(
        server: ServerId,
        channel: ChannelId,
        me: Arc<MemberSecret>,
        limit: usize,
    ) -> ChannelKeys {
        ChannelKeys {
            server,
            channel,
            me,
            rotations: HashMap::new(),
            keys: HashMap::new(),
            linked: BTreeSet::new(),
            waiting: HashMap::new(),
            accepted: HashSet::new(),
            ever_accepted: HashSet::new(),
            children: HashMap::new(),
            arrival: HashMap::new(),
            next_arrival: 0,
            probation: BTreeSet::new(),
            probation_limit: limit.max(1),
            head: None,
        }
    }

    fn my_key(&self) -> PublicKey {
        self.me.public()
    }

    /// Takes in a rotation from a signed record: its ID, author and body.
    /// Apply every rotation record of the channel, authorized or not; it can
    /// only become head once [`Self::set_accepted`] or [`Self::accept`] says
    /// `authority` accepts it. Idempotent, and order doesn't matter as long
    /// as nothing is evicted.
    ///
    /// Also returns the rotations dropped from probation to make room (see
    /// the module docs). The new rotation is never one of them. If a dropped
    /// one is needed later (a rotation built on it waits, or authority
    /// accepts it), sync should deliver it again.
    pub fn apply(
        &mut self,
        id: RecordId,
        rotator: PublicKey,
        body: &[u8],
    ) -> Result<(Applied, Vec<RecordId>), Error> {
        if self.rotations.contains_key(&id) {
            return Ok((Applied::Duplicate, Vec::new()));
        }
        let rotation = Rotation::decode(id, self.server, rotator, body)?;
        if rotation.channel() != self.channel {
            return Err(Error::WrongChannel);
        }
        let applied = match rotation.unwrap_key(&self.me) {
            Ok(key) => {
                self.keys.insert(id, key);
                Applied::Key
            }
            Err(Error::NoKey) => Applied::NotRecipient,
            Err(_) => Applied::WrapInvalid,
        };
        self.rotations.insert(id, rotation);
        self.arrival.insert(id, self.next_arrival);
        self.next_arrival += 1;
        self.update_probation(id);
        self.link(id);
        let mut evicted = Vec::new();
        while self.probation.len() > self.probation_limit {
            let (_, oldest) = self.probation.pop_first().expect("over the limit");
            self.evict(oldest);
            evicted.push(oldest);
        }
        Ok((applied, evicted))
    }

    /// How many rotations are on probation now.
    pub fn on_probation(&self) -> usize {
        self.probation.len()
    }

    /// How many rotations are held, linked or not.
    pub fn held(&self) -> usize {
        self.rotations.len()
    }

    /// Puts `id` on probation or takes it off, by the current state.
    fn update_probation(&mut self, id: RecordId) {
        let Some(arrival) = self.arrival.get(&id).copied() else {
            return;
        };
        let has_children = self.children.get(&id).is_some_and(|count| *count > 0);
        if self.ever_accepted.contains(&id) || has_children {
            self.probation.remove(&(arrival, id));
        } else {
            self.probation.insert((arrival, id));
        }
    }

    /// Drops a rotation on probation. Nothing linked builds on it and it was
    /// never accepted, so it isn't head and the head doesn't change.
    fn evict(&mut self, id: RecordId) {
        let rotation = self.rotations.remove(&id).expect("evicting a held rotation");
        self.keys.remove(&id);
        self.arrival.remove(&id);
        self.children.remove(&id);
        let Some(prev) = rotation.prev() else {
            self.linked.remove(&id);
            return;
        };
        if self.linked.remove(&id) {
            let count = self.children.get_mut(&prev).expect("a linked rotation's prev counts it");
            *count -= 1;
            self.update_probation(prev);
        } else if let Some(waiters) = self.waiting.get_mut(&prev) {
            waiters.retain(|waiter| *waiter != id);
            if waiters.is_empty() {
                self.waiting.remove(&prev);
            }
        }
    }

    /// Replaces the set of rotations `authority` accepts (for the integration:
    /// `authority::State::accepted` intersected with this channel's rotation
    /// records). Call it whenever that set changes, including when authority
    /// voids a rotation it accepted before. Only accepted rotations can be
    /// head; the others keep their keys for reading. IDs we don't hold yet are
    /// remembered, so the order of `apply` and `set_accepted` doesn't matter.
    pub fn set_accepted(&mut self, accepted: impl IntoIterator<Item = RecordId>) {
        self.accepted = accepted.into_iter().collect();
        for id in &self.accepted {
            if self.ever_accepted.insert(*id) {
                if let Some(arrival) = self.arrival.get(id) {
                    self.probation.remove(&(*arrival, *id));
                }
            }
        }
        self.head = None;
        let linked: Vec<RecordId> = self.linked.iter().copied().collect();
        for id in linked {
            self.consider_head(id);
        }
    }

    /// Adds one rotation to the accepted set, for a caller that learns
    /// acceptances one at a time. Withdrawing one needs [`Self::set_accepted`].
    pub fn accept(&mut self, id: RecordId) {
        if self.ever_accepted.insert(id) {
            self.update_probation(id);
        }
        if self.accepted.insert(id) && self.linked.contains(&id) {
            self.consider_head(id);
        }
    }

    pub fn is_accepted(&self, id: &RecordId) -> bool {
        self.accepted.contains(id)
    }

    fn consider_head(&mut self, id: RecordId) {
        if !self.accepted.contains(&id) {
            return;
        }
        let rotation = &self.rotations[&id];
        let better = match self.head.map(|head| &self.rotations[&head]) {
            None => true,
            Some(head) => (rotation.epoch(), Reverse(id)) > (head.epoch(), Reverse(head.id())),
        };
        if better {
            self.head = Some(id);
        }
    }

    /// Links `id` if it can be, then everything that was waiting on it.
    fn link(&mut self, id: RecordId) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let rotation = &self.rotations[&id];
            let prev = match rotation.prev() {
                None => None,
                Some(prev) if !self.linked.contains(&prev) => {
                    self.waiting.entry(prev).or_default().push(id);
                    continue;
                }
                Some(prev) => Some(&self.rotations[&prev]),
            };
            // Decode guarantees epoch 0 <=> no prev.
            if prev.is_some_and(|prev| prev.epoch().checked_add(1) != Some(rotation.epoch())) {
                continue;
            }
            self.linked.insert(id);
            if let Some(prev) = self.rotations[&id].prev() {
                *self.children.entry(prev).or_default() += 1;
                self.update_probation(prev);
            }
            self.consider_head(id);
            stack.extend(self.waiting.remove(&id).unwrap_or_default());
        }
    }

    /// The rotation whose key new messages use.
    pub fn head(&self) -> Option<&Rotation> {
        self.head.map(|id| &self.rotations[&id])
    }

    pub fn rotation(&self, id: &RecordId) -> Option<&Rotation> {
        self.rotations.get(id)
    }

    /// Whether we hold the key of rotation `id`.
    pub fn has_key(&self, id: &RecordId) -> bool {
        self.keys.contains_key(id)
    }

    /// Compares the head's recipients with who should read the channel now
    /// (from `authority`: not kicked, and with access to the channel).
    pub fn rotation_need(&self, members: &BTreeSet<PublicKey>) -> RotationNeed {
        let Some(head) = self.head() else {
            return RotationNeed::Initial;
        };
        let removed: Vec<PublicKey> =
            head.recipients().filter(|recipient| !members.contains(recipient)).copied().collect();
        if !removed.is_empty() {
            return RotationNeed::Required { removed };
        }
        let me = self.my_key();
        let missing: Vec<PublicKey> = members
            .iter()
            .filter(|member| {
                !head.is_recipient(member) || (**member == me && !self.has_key(&head.id()))
            })
            .copied()
            .collect();
        if !missing.is_empty() {
            return RotationNeed::Add { missing };
        }
        RotationNeed::None
    }

    /// A rotation body that replaces the head with a fresh key for
    /// `members`. `rotator` must be the key that signs the record.
    pub fn rotate(
        &self,
        rotator: PublicKey,
        members: &BTreeSet<PublicKey>,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<Vec<u8>, Error> {
        let (epoch, prev) = match self.head() {
            None => (0, None),
            Some(head) => {
                (head.epoch().checked_add(1).ok_or(Error::EpochOverflow)?, Some(head.id()))
            }
        };
        let (body, _key) =
            Rotation::create(self.server, rotator, self.channel, epoch, prev, members, rng)?;
        Ok(body)
    }

    /// Encrypts a message from us under the head key. Refuses while the
    /// head is still readable by someone outside `members`.
    pub fn encrypt(
        &self,
        members: &BTreeSet<PublicKey>,
        plaintext: &[u8],
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<Vec<u8>, Error> {
        let head = self.head().ok_or(Error::NoKey)?;
        match self.rotation_need(members) {
            RotationNeed::Required { removed } => return Err(Error::RotationRequired { removed }),
            RotationNeed::Initial => return Err(Error::NoKey),
            RotationNeed::None | RotationNeed::Add { .. } => {}
        }
        let key = self.keys.get(&head.id()).ok_or(Error::NoKey)?;
        seal(key, &head.message_context(self.my_key()), plaintext, rng)
    }

    /// Decrypts a message body whose record was signed by `author`.
    pub fn decrypt(&self, author: PublicKey, ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
        self.decrypt_with_rotation(author, ciphertext).map(|decrypted| decrypted.plaintext)
    }

    /// Like [`Self::decrypt`], and also says which rotation's key opened it,
    /// and whether `authority` accepts that rotation, so the caller can flag
    /// a message sent under a superseded or non-accepted key.
    pub fn decrypt_with_rotation(
        &self,
        author: PublicKey,
        ciphertext: &[u8],
    ) -> Result<Decrypted, Error> {
        let header = MessageHeader::parse(ciphertext)?;
        let mut candidates = self
            .keys
            .iter()
            .map(|(id, key)| (&self.rotations[id], key))
            .filter(|(rotation, _)| {
                rotation.epoch() == header.epoch && header.matches(&rotation.id())
            })
            .peekable();
        if candidates.peek().is_none() {
            return Err(Error::NoKey);
        }
        let mut error = Error::DecryptFailed;
        for (rotation, key) in candidates {
            if !rotation.is_recipient(&author) {
                error = Error::AuthorNotRecipient;
                continue;
            }
            if let Ok(plaintext) = open(key, &rotation.message_context(author), ciphertext) {
                return Ok(Decrypted {
                    plaintext,
                    rotation: rotation.id(),
                    epoch: rotation.epoch(),
                    accepted: self.accepted.contains(&rotation.id()),
                });
            }
        }
        Err(error)
    }
}
