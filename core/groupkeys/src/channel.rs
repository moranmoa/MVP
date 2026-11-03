//! One member's view of one channel's epochs.
//!
//! Rules, all decided by the set of rotations held and the set `authority`
//! voids, never by the order either arrived in:
//! - Every well-formed rotation of the channel is stored, whether or not its
//!   author may rotate. `authority` has the final say, and it can change its
//!   mind (a revoke voids a moderator's concurrent records), so the caller
//!   reports the rotations it doesn't accept with [`ChannelKeys::set_voided`]
//!   whenever that set changes.
//! - A rotation is *linked* when it is epoch 0, or its `prev` is linked and
//!   one epoch below it. A rotation whose `prev` hasn't arrived yet waits.
//!   Linking only checks the epoch counter, so a voided rotation still links
//!   the rotations built on it: their keys are fresh and owe nothing to it.
//! - The head (the key to send with) is the linked, non-voided rotation with
//!   the highest epoch; between rotations of that epoch the lowest record ID
//!   wins, the same tie-break `authority` uses.
//! - Keys of every rotation we were a recipient of are kept, voided or not,
//!   so messages from any epoch we belonged to stay readable. [`Decrypted`]
//!   says which rotation opened a message, for policy above this crate. A
//!   member who was offline through several rotations just applies them all.
//! - If the head's recipients include someone no longer in the channel (a
//!   kick, or a revoke that took away access), sending is refused until a
//!   moderator rotates.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};

use ed25519_dalek::SigningKey;
use rand_core::{CryptoRng, RngCore};
use records::{ChannelId, PublicKey, RecordId, ServerId};

use crate::{open, seal, EpochKey, Error, MessageHeader, Rotation};

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
}

pub struct ChannelKeys {
    server: ServerId,
    channel: ChannelId,
    me: SigningKey,
    rotations: HashMap<RecordId, Rotation>,
    keys: HashMap<RecordId, EpochKey>,
    linked: BTreeSet<RecordId>,
    /// Rotations whose `prev` isn't linked yet, by that `prev`.
    waiting: HashMap<RecordId, Vec<RecordId>>,
    /// Rotations `authority` doesn't accept.
    voided: HashSet<RecordId>,
    head: Option<RecordId>,
}

impl ChannelKeys {
    pub fn new(server: ServerId, channel: ChannelId, me: SigningKey) -> ChannelKeys {
        ChannelKeys {
            server,
            channel,
            me,
            rotations: HashMap::new(),
            keys: HashMap::new(),
            linked: BTreeSet::new(),
            waiting: HashMap::new(),
            voided: HashSet::new(),
            head: None,
        }
    }

    fn my_key(&self) -> PublicKey {
        PublicKey::from_bytes(self.me.verifying_key().to_bytes())
    }

    /// Takes in a rotation from a signed record: its ID, author and body.
    /// Apply every rotation record of the channel, authorized or not, and
    /// report the ones `authority` rejects with [`Self::set_voided`].
    /// Idempotent, and order doesn't matter.
    pub fn apply(
        &mut self,
        id: RecordId,
        rotator: PublicKey,
        body: &[u8],
    ) -> Result<Applied, Error> {
        if self.rotations.contains_key(&id) {
            return Ok(Applied::Duplicate);
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
        self.link(id);
        Ok(applied)
    }

    /// Replaces the set of rotations `authority` doesn't accept: never
    /// permitted, or voided later (say the rotator was revoked concurrently).
    /// Call it whenever that set changes. A voided rotation can't be the head
    /// but keeps its key for reading. IDs we don't hold are remembered too, so
    /// the order of `apply` and `set_voided` doesn't matter.
    pub fn set_voided(&mut self, voided: impl IntoIterator<Item = RecordId>) {
        self.voided = voided.into_iter().collect();
        self.head = None;
        let linked: Vec<RecordId> = self.linked.iter().copied().collect();
        for id in linked {
            self.consider_head(id);
        }
    }

    pub fn is_voided(&self, id: &RecordId) -> bool {
        self.voided.contains(id)
    }

    fn consider_head(&mut self, id: RecordId) {
        if self.voided.contains(&id) {
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
    /// so the caller can flag a message sent under a superseded or voided key.
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
                });
            }
        }
        Err(error)
    }
}
