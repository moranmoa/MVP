//! [`GroupCipher`] over `groupkeys`: one `ChannelKeys` per channel, all
//! sharing one `MemberSecret`.
//!
//! Key material stays here; the identity signing key and record signing stay
//! in service. Rotating returns unsigned `KeyRotation` part bodies: service
//! signs every one and publishes all of them or none (a partial rotation is
//! never used, and re-signing a part gets the rotation refused).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use groupkeys::{ChannelKeys, Error, MemberSecret};
use rand_core::OsRng;
use records::{ChannelId, Kind, PublicKey, Record, RecordId, Scope, ServerId};

use crate::ports::{CipherError, CipherOutcome, GroupCipher, Opened};

/// The real group cipher.
pub struct GroupKeysCipher {
    secret: Arc<MemberSecret>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    channels: HashMap<(ServerId, ChannelId), ChannelKeys>,
    /// Who may read each server's channels now, from authority.
    members: HashMap<ServerId, BTreeSet<PublicKey>>,
    /// Each server's accepted `KeyRotation` records, from authority.
    accepted: HashMap<ServerId, BTreeSet<RecordId>>,
}

impl State {
    fn channel(
        &mut self,
        secret: &Arc<MemberSecret>,
        server: ServerId,
        channel: ChannelId,
    ) -> &mut ChannelKeys {
        let accepted = self.accepted.get(&server);
        self.channels.entry((server, channel)).or_insert_with(|| {
            let mut keys = ChannelKeys::new(server, channel, Arc::clone(secret));
            if let Some(accepted) = accepted {
                keys.set_accepted(accepted.iter().copied());
            }
            keys
        })
    }
}

impl GroupKeysCipher {
    /// `secret` is built once at unlock (`MemberSecret::new(signing_key)`).
    pub fn new(secret: Arc<MemberSecret>) -> GroupKeysCipher {
        GroupKeysCipher { secret, state: Mutex::new(State::default()) }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
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
        let keys = state.channel(&self.secret, server, channel);
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
        let keys = state.channel(&self.secret, server, channel);
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
        let keys = state.channel(&self.secret, record.server(), channel);
        let (_, evicted) =
            keys.apply(record.id(), record.author(), record.body()).map_err(cipher_error)?;
        Ok(evicted)
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
        let me = self.secret.public();
        let mut state = self.state();
        let keys = state.channel(&self.secret, server, channel);
        keys.rotate(me, members, &mut OsRng).map_err(cipher_error)
    }
}

#[cfg(test)]
mod tests;
