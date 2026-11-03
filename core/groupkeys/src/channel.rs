//! One member's view of one channel's epochs.
//!
//! Rules, all decided by the set of parts held and the set `authority`
//! accepts, never by the order either arrived in:
//! - A rotation travels as 1 to 32 parts, each its own signed `KeyRotation`
//!   record. Every well-formed part of the channel is stored, whether or not
//!   its author may rotate. A rotation is *assembled* once every part is
//!   there; parts that contradict each other (two records for one index, a
//!   different part count, parts that don't fit together) get the whole
//!   rotation *refused*: it can never be head.
//! - Only `authority` decides who may rotate, and it can change its mind (a
//!   revoke voids a moderator's concurrent records), so the caller passes the
//!   part records it currently **accepts** with [`ChannelKeys::set_accepted`]
//!   whenever that set changes. A rotation is accepted only when every one of
//!   its parts is. Deny by default: a rotation authority hasn't ruled on yet
//!   (not processed, pending on missing records, or a kind it doesn't know)
//!   is never sent with.
//! - A rotation is *linked* when it is epoch 0, or its `prev` is linked and
//!   one epoch below it. A rotation whose `prev` hasn't arrived yet waits.
//!   Linking only checks the epoch counter, so a rotation that isn't accepted
//!   still links the rotations built on it: their keys are fresh and owe
//!   nothing to it.
//! - The head (the key to send with) is the linked, accepted rotation with
//!   the highest epoch; between rotations of that epoch the lowest rotation
//!   ID wins.
//! - Keys of every rotation we were a recipient of are kept, accepted or not,
//!   so messages from any epoch we belonged to stay readable. [`Decrypted`]
//!   says which rotation opened a message and whether it is accepted; the
//!   caller must label a message opened under a non-accepted rotation as
//!   untrusted. A member who was offline through several rotations just
//!   applies them all.
//! - If the head's recipients include someone no longer in the channel (a
//!   kick, or a revoke that took away access), sending is refused until a
//!   moderator rotates.
//! - Memory is bounded. Anyone can sign rotation records, so only rotations
//!   with a part `authority` has accepted at some point, and their ancestors,
//!   are *protected*. Every other held rotation, complete or not, is *on
//!   probation*. While more than [`MAX_UNACCEPTED`] rotations or more than
//!   [`MAX_UNACCEPTED_BYTES`] of their parts are, the oldest by arrival is
//!   dropped together with everything built on it, parts and keys and all,
//!   and their record IDs returned so sync can fetch them again if they turn
//!   out to matter.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use rand_core::{CryptoRng, RngCore};
use records::{ChannelId, PublicKey, RecordId, ServerId};

use crate::{
    open, seal, EpochKey, Error, MemberSecret, MessageHeader, Part, Rotation, MAX_ROTATION_BYTES,
};

/// How many rotations a channel holds on probation (not accepted, not an
/// ancestor of an accepted one) before the oldest is dropped.
pub const MAX_UNACCEPTED: usize = 64;

/// How many bytes of parts a channel holds on probation before the oldest
/// rotation is dropped: room for four complete rotations of 32 parts.
pub const MAX_UNACCEPTED_BYTES: usize = 2 * 1024 * 1024;

/// What [`ChannelKeys::apply`] did with a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Stored; the rotation still misses parts.
    Incomplete,
    /// Stored, the rotation is complete, and we now hold its key.
    Key,
    /// Stored and complete; we aren't a recipient.
    NotRecipient,
    /// Stored and complete, but the wrap addressed to us doesn't open. Other
    /// members can still use the rotation, so it counts like any other; we
    /// hold no key.
    WrapInvalid,
    /// The rotation's parts contradict each other. It is refused and never
    /// becomes head; this part isn't stored.
    Refused,
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

/// The parts of one rotation we hold.
struct Held {
    prev: Option<RecordId>,
    /// The record carrying each part, by index.
    records: Vec<Option<RecordId>>,
    /// Parts not assembled yet; emptied once the rotation is.
    parts: Vec<Option<Part>>,
    present: usize,
    /// Body bytes of the parts held.
    bytes: usize,
    arrival: u64,
}

pub struct ChannelKeys {
    server: ServerId,
    channel: ChannelId,
    me: Arc<MemberSecret>,
    /// Every rotation we hold parts of, by rotation ID.
    held: HashMap<RecordId, Held>,
    /// Part record → rotation ID.
    part_of: HashMap<RecordId, RecordId>,
    /// Assembled rotations.
    rotations: HashMap<RecordId, Rotation>,
    keys: HashMap<RecordId, EpochKey>,
    linked: BTreeSet<RecordId>,
    /// Rotations whose `prev` isn't linked yet, by that `prev`.
    waiting: HashMap<RecordId, Vec<RecordId>>,
    /// Part records `authority` accepts.
    accepted: HashSet<RecordId>,
    /// Part records `authority` has accepted at some point.
    ever_accepted: HashSet<RecordId>,
    /// Rotations with an ever-accepted part, and their ancestors, held or
    /// not yet. Never on probation.
    protected: HashSet<RecordId>,
    /// Rotations whose parts contradict each other.
    refused: HashSet<RecordId>,
    /// Held rotations by their `prev`, complete or not.
    children: HashMap<RecordId, Vec<RecordId>>,
    next_arrival: u64,
    /// Rotations on probation, oldest first, and the bytes of their parts.
    probation: BTreeSet<(u64, RecordId)>,
    probation_bytes: usize,
    limit: usize,
    byte_limit: usize,
    head: Option<RecordId>,
}

impl ChannelKeys {
    /// `me` is shared by every channel of the client; see [`MemberSecret`].
    pub fn new(server: ServerId, channel: ChannelId, me: Arc<MemberSecret>) -> ChannelKeys {
        Self::with_limits(server, channel, me, MAX_UNACCEPTED, MAX_UNACCEPTED_BYTES)
    }

    /// Like [`Self::new`], with at most `rotations` rotations and `bytes`
    /// bytes on probation. `bytes` is raised to fit one complete rotation.
    pub fn with_limits(
        server: ServerId,
        channel: ChannelId,
        me: Arc<MemberSecret>,
        rotations: usize,
        bytes: usize,
    ) -> ChannelKeys {
        ChannelKeys {
            server,
            channel,
            me,
            held: HashMap::new(),
            part_of: HashMap::new(),
            rotations: HashMap::new(),
            keys: HashMap::new(),
            linked: BTreeSet::new(),
            waiting: HashMap::new(),
            accepted: HashSet::new(),
            ever_accepted: HashSet::new(),
            protected: HashSet::new(),
            refused: HashSet::new(),
            children: HashMap::new(),
            next_arrival: 0,
            probation: BTreeSet::new(),
            probation_bytes: 0,
            limit: rotations.max(1),
            byte_limit: bytes.max(MAX_ROTATION_BYTES),
            head: None,
        }
    }

    fn my_key(&self) -> PublicKey {
        self.me.public()
    }

    /// Takes in one rotation part from a signed `KeyRotation` record: its
    /// ID, author and body. Apply every part of the channel, authorized or
    /// not; a rotation can only become head once [`Self::set_accepted`] or
    /// [`Self::accept`] says `authority` accepts every one of its parts.
    /// Idempotent, and order doesn't matter as long as nothing is evicted.
    ///
    /// Also returns the part records dropped to make room (see the module
    /// docs), which can include this one if its rotation builds on the
    /// oldest. If a dropped rotation is needed later (a rotation built on it
    /// waits, or authority accepts it), sync should deliver it again.
    pub fn apply(
        &mut self,
        record: RecordId,
        rotator: PublicKey,
        body: &[u8],
    ) -> Result<(Applied, Vec<RecordId>), Error> {
        if self.part_of.contains_key(&record) {
            return Ok((Applied::Duplicate, Vec::new()));
        }
        let part = Part::decode(rotator, body)?;
        if part.channel() != self.channel {
            return Err(Error::WrongChannel);
        }
        let id = part.rotation();
        if self.refused.contains(&id) {
            return Ok((Applied::Refused, Vec::new()));
        }
        if !self.held.contains_key(&id) {
            self.hold(id, &part);
        }
        let held = self.held.get_mut(&id).expect("held above");
        let index = usize::from(part.index());
        if held.records.len() != usize::from(part.parts()) || held.records[index].is_some() {
            // The rotator signed parts that contradict each other.
            self.refuse(id);
            return Ok((Applied::Refused, Vec::new()));
        }
        held.records[index] = Some(record);
        held.parts[index] = Some(part);
        held.present += 1;
        held.bytes += body.len();
        let complete = held.present == held.records.len();
        if self.probation.contains(&(held.arrival, id)) {
            self.probation_bytes += body.len();
        }
        self.part_of.insert(record, id);
        if self.ever_accepted.contains(&record) {
            self.protect(id);
        }
        let applied = if complete { self.assemble(id) } else { Applied::Incomplete };
        let mut evicted = Vec::new();
        while self.probation.len() > self.limit || self.probation_bytes > self.byte_limit {
            // Eviction takes it off probation and subtracts its bytes.
            let (_, oldest) = *self.probation.first().expect("over a limit");
            self.evict_with_descendants(oldest, &mut evicted);
        }
        Ok((applied, evicted))
    }

    /// Starts holding rotation `id`, of which `part` is the first part seen.
    fn hold(&mut self, id: RecordId, part: &Part) {
        let parts = usize::from(part.parts());
        let arrival = self.next_arrival;
        self.next_arrival += 1;
        self.held.insert(
            id,
            Held {
                prev: part.prev(),
                records: vec![None; parts],
                parts: vec![None; parts],
                present: 0,
                bytes: 0,
                arrival,
            },
        );
        if let Some(prev) = part.prev() {
            self.children.entry(prev).or_default().push(id);
        }
        if self.protected.contains(&id) {
            // An ancestor of an accepted rotation before it arrived: its own
            // ancestors are protected too.
            if let Some(prev) = part.prev() {
                self.protect(prev);
            }
        } else {
            self.probation.insert((arrival, id));
        }
    }

    /// Every part of `id` is there: put it together and link it.
    fn assemble(&mut self, id: RecordId) -> Applied {
        let held = self.held.get_mut(&id).expect("assembling a held rotation");
        let parts: Vec<Part> = held.parts.iter_mut().filter_map(Option::take).collect();
        let rotation = match Rotation::assemble(self.server, parts) {
            Ok(rotation) => rotation,
            Err(_) => {
                self.refuse(id);
                return Applied::Refused;
            }
        };
        let applied = match rotation.unwrap_key(&self.me) {
            Ok(key) => {
                self.keys.insert(id, key);
                Applied::Key
            }
            Err(Error::NoKey) => Applied::NotRecipient,
            Err(_) => Applied::WrapInvalid,
        };
        self.rotations.insert(id, rotation);
        self.link(id);
        applied
    }

    /// Never lets `id` be head again. Its parts stay held (and on probation
    /// unless protected), so a later conflicting part is spotted too.
    fn refuse(&mut self, id: RecordId) {
        if self.refused.insert(id) && self.head == Some(id) {
            self.recompute_head();
        }
    }

    /// How many rotations are on probation now.
    pub fn on_probation(&self) -> usize {
        self.probation.len()
    }

    /// Bytes of the parts of rotations on probation.
    pub fn probation_bytes(&self) -> usize {
        self.probation_bytes
    }

    /// How many rotations we hold parts of, complete or not.
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// Protects rotation `id` and its ancestors, held or not yet, from
    /// eviction.
    fn protect(&mut self, id: RecordId) {
        let mut next = Some(id);
        while let Some(id) = next.take() {
            if !self.protected.insert(id) {
                // Already protected, and so are its held ancestors.
                break;
            }
            if let Some(held) = self.held.get(&id) {
                if self.probation.remove(&(held.arrival, id)) {
                    self.probation_bytes -= held.bytes;
                }
                next = held.prev;
            }
        }
    }

    /// Drops an unprotected rotation and every held rotation built on it,
    /// appending their part records to `evicted`. A protected rotation's
    /// ancestors are all protected, so none of these is, and none is head.
    fn evict_with_descendants(&mut self, root: RecordId, evicted: &mut Vec<RecordId>) {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            debug_assert!(!self.protected.contains(&id));
            let Some(held) = self.held.remove(&id) else {
                continue;
            };
            if self.probation.remove(&(held.arrival, id)) {
                self.probation_bytes -= held.bytes;
            }
            for record in held.records.iter().flatten() {
                self.part_of.remove(record);
                evicted.push(*record);
            }
            self.rotations.remove(&id);
            self.keys.remove(&id);
            self.linked.remove(&id);
            self.waiting.remove(&id);
            self.refused.remove(&id);
            if let Some(prev) = held.prev {
                for index in [&mut self.children, &mut self.waiting] {
                    if let Some(ids) = index.get_mut(&prev) {
                        ids.retain(|other| *other != id);
                        if ids.is_empty() {
                            index.remove(&prev);
                        }
                    }
                }
            }
            stack.extend(self.children.remove(&id).unwrap_or_default());
        }
    }

    /// Replaces the set of part records `authority` accepts (for the
    /// integration: `authority::State::accepted` intersected with this
    /// channel's `KeyRotation` records). Call it whenever that set changes,
    /// including when authority voids parts it accepted before. Only
    /// rotations with every part accepted can be head; the others keep their
    /// keys for reading. IDs we don't hold yet are remembered, so the order of
    /// `apply` and `set_accepted` doesn't matter.
    pub fn set_accepted(&mut self, accepted: impl IntoIterator<Item = RecordId>) {
        self.accepted = accepted.into_iter().collect();
        let accepted: Vec<RecordId> = self.accepted.iter().copied().collect();
        for record in accepted {
            self.ever_accepted.insert(record);
            if let Some(id) = self.part_of.get(&record).copied() {
                self.protect(id);
            }
        }
        self.recompute_head();
    }

    /// Adds one part record to the accepted set, for a caller that learns
    /// acceptances one at a time. Withdrawing one needs [`Self::set_accepted`].
    pub fn accept(&mut self, record: RecordId) {
        self.ever_accepted.insert(record);
        if !self.accepted.insert(record) {
            return;
        }
        if let Some(id) = self.part_of.get(&record).copied() {
            self.protect(id);
            if self.linked.contains(&id) {
                self.consider_head(id);
            }
        }
    }

    /// Whether `authority` accepts rotation `id`: it is complete, not
    /// refused, and every part is accepted.
    pub fn is_accepted(&self, id: &RecordId) -> bool {
        !self.refused.contains(id)
            && self.rotations.contains_key(id)
            && self.held.get(id).is_some_and(|held| {
                held.records
                    .iter()
                    .all(|record| record.is_some_and(|record| self.accepted.contains(&record)))
            })
    }

    fn recompute_head(&mut self) {
        self.head = None;
        let linked: Vec<RecordId> = self.linked.iter().copied().collect();
        for id in linked {
            self.consider_head(id);
        }
    }

    fn consider_head(&mut self, id: RecordId) {
        if !self.is_accepted(&id) {
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
            self.consider_head(id);
            stack.extend(self.waiting.remove(&id).unwrap_or_default());
        }
    }

    /// The rotation whose key new messages use.
    pub fn head(&self) -> Option<&Rotation> {
        self.head.map(|id| &self.rotations[&id])
    }

    /// An assembled rotation, by rotation ID.
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

    /// The part bodies of a rotation that replaces the head with a fresh key
    /// for `members`, each to be signed by `rotator` in its own record.
    pub fn rotate(
        &self,
        rotator: PublicKey,
        members: &BTreeSet<PublicKey>,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<Vec<Vec<u8>>, Error> {
        let (epoch, prev) = match self.head() {
            None => (0, None),
            Some(head) => {
                (head.epoch().checked_add(1).ok_or(Error::EpochOverflow)?, Some(head.id()))
            }
        };
        let (parts, _key) =
            Rotation::create(self.server, rotator, self.channel, epoch, prev, members, rng)?;
        Ok(parts)
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
                    accepted: self.is_accepted(&rotation.id()),
                });
            }
        }
        Err(error)
    }
}
