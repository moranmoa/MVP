//! [`GroupCipher`] over `groupkeys`: one `ChannelKeys` per channel, all
//! sharing one `MemberSecret`.
//!
//! Key material stays here; the identity signing key and record signing stay
//! in service. Rotating returns unsigned `KeyRotation` part bodies: service
//! signs every one and publishes all of them or none (a partial rotation is
//! never used, and re-signing a part gets the rotation refused).
//!
//! **The member secret.** `MemberSecret::new(signing_key)` is the identity's
//! Ed25519 secret scalar itself (`SigningKey::to_scalar_bytes`: the clamped
//! first half of SHA-512 of the seed), used as an X25519 secret. There is no
//! context string. It is one-way from the seed, but it can sign for the
//! identity, so it is as sensitive as the seed. The security lead accepts
//! this reuse until device keys arrive, under four conditions:
//! 1. DH only through `groupkeys`: small-order keys and an all-zero output
//!    are refused, and the output is only used through BLAKE3 `derive_key`,
//!    bound to the ephemeral key, the recipient, the server, the rotator and
//!    the rotation header.
//! 2. It never leaves the core process, is never persisted, never appears in
//!    `Debug` or logs, and is zeroized on [`GroupKeysCipher::lock`] and on
//!    drop (tested below).
//! 3. The scalar serves this KEM only; libp2p keeps its own `node.key`.
//! 4. TODO(GAP-10): switch to a separate, published X25519 key derived with
//!    its own context string when device keys arrive.

pub mod invite;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use groupkeys::{ChannelKeys, Error, MemberSecret};
use rand_core::OsRng;
use records::{ChannelId, Kind, PublicKey, Record, RecordId, Scope, ServerId};

use crate::ports::{CipherError, CipherOutcome, GroupCipher, Opened};

/// The real group cipher. Starts locked: everything but bookkeeping returns
/// `Unavailable` until [`Self::unlock`].
pub struct GroupKeysCipher {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    secret: Option<Arc<MemberSecret>>,
    channels: HashMap<(ServerId, ChannelId), ChannelKeys>,
    /// Who may read each server's channels now, from authority.
    members: HashMap<ServerId, BTreeSet<PublicKey>>,
    /// Each server's accepted `KeyRotation` records, from authority.
    accepted: HashMap<ServerId, BTreeSet<RecordId>>,
    /// Each server's channels, from authority. Keys exist only for these:
    /// anyone can name a channel in a record, and each `ChannelKeys` may
    /// hold up to 2 MiB on probation (GAP-8).
    known: HashMap<ServerId, BTreeSet<ChannelId>>,
}

impl State {
    /// The keys of a channel authority knows; `Rejected` for any other,
    /// without creating anything.
    fn channel(
        &mut self,
        server: ServerId,
        channel: ChannelId,
    ) -> Result<&mut ChannelKeys, CipherError> {
        let secret = self.secret.clone().ok_or(CipherError::Unavailable)?;
        if !self.known.get(&server).is_some_and(|channels| channels.contains(&channel)) {
            return Err(CipherError::Rejected);
        }
        let accepted = self.accepted.get(&server);
        Ok(self.channels.entry((server, channel)).or_insert_with(|| {
            let mut keys = ChannelKeys::new(server, channel, secret);
            if let Some(accepted) = accepted {
                keys.set_accepted(accepted.iter().copied());
            }
            keys
        }))
    }
}

impl GroupKeysCipher {
    /// Locked until [`Self::unlock`]; what mvs-core plugs into `Ports`.
    pub fn locked() -> GroupKeysCipher {
        GroupKeysCipher { state: Mutex::new(State::default()) }
    }

    /// Unlocked with `secret` (`MemberSecret::new(signing_key)`).
    pub fn new(secret: Arc<MemberSecret>) -> GroupKeysCipher {
        let cipher = GroupKeysCipher::locked();
        cipher.unlock(secret);
        cipher
    }

    /// At identity unlock: from now on, encrypt and decrypt work.
    pub fn unlock(&self, secret: Arc<MemberSecret>) {
        self.state().secret = Some(secret);
    }

    /// At app lock and sign-out: drops the secret and every channel's keys,
    /// so the secret is zeroized once the caller's own `Arc` is gone too.
    /// After the next unlock, rotations have to be applied again.
    pub fn lock(&self) {
        let mut state = self.state();
        state.secret = None;
        state.channels.clear();
    }

    /// How many channels hold keys now.
    pub fn channels_held(&self) -> usize {
        self.state().channels.len()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }
}

impl std::fmt::Debug for GroupKeysCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GroupKeysCipher(..)")
    }
}

fn cipher_error(error: Error) -> CipherError {
    match error {
        Error::NoKey => CipherError::NoKey,
        Error::RotationRequired { .. } => CipherError::RotationRequired,
        _ => CipherError::Rejected,
    }
}

impl GroupCipher for GroupKeysCipher {
    /// Records the new member set. It doesn't rotate: service asks
    /// [`GroupCipher::rotate`] when `encrypt` says `RotationRequired`.
    fn membership_changed(&self, server: ServerId, members: &BTreeSet<PublicKey>) -> CipherOutcome {
        self.state().members.insert(server, members.clone());
        CipherOutcome::Unchanged
    }

    fn encrypt(
        &self,
        server: ServerId,
        channel: ChannelId,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        let mut state = self.state();
        let members = state.members.get(&server).cloned().unwrap_or_default();
        // A channel authority doesn't know has no key to send with.
        let keys = state.channel(server, channel).map_err(|error| match error {
            CipherError::Rejected => CipherError::NoKey,
            other => other,
        })?;
        keys.encrypt(&members, plaintext, &mut OsRng).map_err(cipher_error)
    }

    fn decrypt(
        &self,
        server: ServerId,
        channel: ChannelId,
        author: PublicKey,
        body: &[u8],
    ) -> Result<Opened, CipherError> {
        let mut state = self.state();
        let keys = state.channel(server, channel)?;
        let opened = keys.decrypt_with_rotation(author, body).map_err(cipher_error)?;
        Ok(Opened { plaintext: opened.plaintext, accepted: opened.accepted })
    }

    fn apply_rotation(&self, record: &Record) -> Result<Vec<RecordId>, CipherError> {
        let Scope::Channel(channel) = record.scope() else {
            return Err(CipherError::Rejected);
        };
        if record.kind() != Kind::KeyRotation {
            return Err(CipherError::Rejected);
        }
        let mut state = self.state();
        let keys = state.channel(record.server(), channel)?;
        let (_, evicted) =
            keys.apply(record.id(), record.author(), record.body()).map_err(cipher_error)?;
        Ok(evicted)
    }

    fn unlock(&self, secret: Arc<MemberSecret>) {
        GroupKeysCipher::unlock(self, secret);
    }

    fn lock(&self) {
        GroupKeysCipher::lock(self);
    }

    fn set_channels(&self, server: ServerId, channels: &BTreeSet<ChannelId>) {
        let mut state = self.state();
        state.channels.retain(|(at, channel), _| *at != server || channels.contains(channel));
        state.known.insert(server, channels.clone());
    }

    fn set_accepted(&self, server: ServerId, accepted: &BTreeSet<RecordId>) {
        let mut state = self.state();
        state.accepted.insert(server, accepted.clone());
        for ((at, _), keys) in state.channels.iter_mut() {
            if *at == server {
                keys.set_accepted(accepted.iter().copied());
            }
        }
    }

    fn rotate(
        &self,
        server: ServerId,
        channel: ChannelId,
        members: &BTreeSet<PublicKey>,
    ) -> Result<Vec<Vec<u8>>, CipherError> {
        let mut state = self.state();
        let me = state.secret.as_ref().ok_or(CipherError::Unavailable)?.public();
        let keys = state.channel(server, channel)?;
        keys.rotate(me, members, &mut OsRng).map_err(cipher_error)
    }
}

#[cfg(test)]
mod tests;
