//! The storage gate `sync` asks before storing a channel record
//! (`sync::Authorizer`). It must be monotone in the stored control log, so
//! it asks only: is the author the server's owner, or does the control log
//! hold a `Join` by the author whose invite is stored too? Kicks, revokes
//! and voids never enter here: they belong to display, forwarding and keys
//! (`authority::check_channel`, B4-4).
//!
//! `ControlLog::records` decodes the whole control log, so verdicts are
//! cached per server and author (#55):
//! - an accept never changes (monotone), so it is kept for good;
//! - a reject holds only until the next control record of that server is
//!   stored ([`MemberGate::control_changed`]). A reject that is stale
//!   because the event hadn't arrived yet only delays: sync asks again when
//!   the record comes back.

use std::collections::HashSet;
use std::sync::Mutex;

use authority::Action;
use records::{Kind, PublicKey, Record, RecordId, ServerId};
use sync::{Authorizer, ControlLog, Verdict};

/// Rejects remembered across all servers; past this the reject cache is
/// cleared, so a flood of strangers costs memory once, not forever.
const MAX_REJECTS: usize = 4096;

#[derive(Default)]
struct Cache {
    accepted: HashSet<(ServerId, PublicKey)>,
    rejected: HashSet<(ServerId, PublicKey)>,
}

#[derive(Default)]
pub struct MemberGate {
    cache: Mutex<Cache>,
}

impl MemberGate {
    pub fn new() -> MemberGate {
        MemberGate::default()
    }

    /// Call when a control record of `server` is stored: rejects for that
    /// server may now be accepts.
    pub fn control_changed(&self, server: ServerId) {
        self.cache.lock().expect("not poisoned").rejected.retain(|(s, _)| *s != server);
    }

    /// The monotone rule on a decoded control log.
    fn may_write(author: PublicKey, control: &[Record]) -> bool {
        let invites: HashSet<RecordId> =
            control.iter().filter(|r| r.kind() == Kind::Invite).map(Record::id).collect();
        control.iter().any(|r| match r.kind() {
            Kind::Genesis => r.author() == author,
            Kind::Join => {
                r.author() == author
                    && matches!(
                        Action::from_record(r),
                        Some(Action::Join { invite }) if invites.contains(&invite)
                    )
            }
            _ => false,
        })
    }
}

impl Authorizer for MemberGate {
    fn check(&self, record: &Record, control: &ControlLog<'_>) -> Verdict {
        let key = (record.server(), record.author());
        {
            let cache = self.cache.lock().expect("not poisoned");
            if cache.accepted.contains(&key) {
                return Verdict::Accept;
            }
            if cache.rejected.contains(&key) {
                return Verdict::Reject;
            }
        }
        // A store error is a reject: sync asks again later.
        let ok = control.records().is_ok_and(|records| Self::may_write(key.1, &records));
        let mut cache = self.cache.lock().expect("not poisoned");
        if ok {
            cache.accepted.insert(key);
            Verdict::Accept
        } else {
            if cache.rejected.len() >= MAX_REJECTS {
                cache.rejected.clear();
            }
            cache.rejected.insert(key);
            Verdict::Reject
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use records::{ChannelId, Draft, Scope, SigningKey};
    use sync::{Append, Store};

    fn message(server: ServerId, key: &SigningKey, seq: u64) -> Record {
        Draft {
            server,
            scope: Scope::Channel(ChannelId::from_bytes([3; 32])),
            seq,
            deps: vec![],
            auth_deps: vec![],
            ts: 1,
            kind: Kind::Message,
            body: seq.to_le_bytes().to_vec(),
        }
        .sign(key)
        .unwrap()
    }

    #[test]
    fn owner_and_joined_members_may_write_strangers_wait() {
        let owner = SigningKey::from_bytes(&[1; 32]);
        let member = SigningKey::from_bytes(&[2; 32]);
        let stranger = SigningKey::from_bytes(&[3; 32]);
        let genesis = authority::sign_genesis(&owner, [0; 16], 1, b"{}").unwrap();
        let server = genesis.server();
        let invite = authority::sign_control(
            &owner,
            server,
            1,
            vec![genesis.id()],
            1,
            &Action::Invite { expires_ms: 0, max_uses: 0 },
        )
        .unwrap();
        let join = authority::sign_control(
            &member,
            server,
            0,
            vec![invite.id()],
            1,
            &Action::Join { invite: invite.id() },
        )
        .unwrap();
        // A join naming an invite that isn't stored doesn't count.
        let fake = authority::sign_control(
            &stranger,
            server,
            0,
            vec![genesis.id()],
            1,
            &Action::Join { invite: RecordId::from_bytes([9; 32]) },
        )
        .unwrap();

        let gate = MemberGate::new();
        let mut store = Store::in_memory().unwrap();
        for record in [&genesis, &invite, &fake] {
            assert_eq!(store.append(record, &gate).unwrap(), Append::Added);
        }
        assert_eq!(store.append(&message(server, &owner, 0), &gate).unwrap(), Append::Added);
        assert_eq!(
            store.append(&message(server, &member, 0), &gate).unwrap(),
            Append::Unauthorized
        );
        assert_eq!(
            store.append(&message(server, &stranger, 0), &gate).unwrap(),
            Append::Unauthorized
        );

        // The join arrives: still refused until the gate hears of it (the
        // cached reject), then accepted, and the accept sticks.
        assert_eq!(store.append(&join, &gate).unwrap(), Append::Added);
        assert_eq!(
            store.append(&message(server, &member, 1), &gate).unwrap(),
            Append::Unauthorized
        );
        gate.control_changed(server);
        assert_eq!(store.append(&message(server, &member, 2), &gate).unwrap(), Append::Added);
        assert_eq!(
            store.append(&message(server, &stranger, 1), &gate).unwrap(),
            Append::Unauthorized
        );
        assert!(gate.cache.lock().unwrap().accepted.contains(&(server, member_key(&member))));
    }

    fn member_key(key: &SigningKey) -> PublicKey {
        PublicKey::from_bytes(key.verifying_key().to_bytes())
    }
}
