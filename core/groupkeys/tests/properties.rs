mod common;

use std::collections::BTreeSet;

use common::*;
use groupkeys::{open, seal, Applied, EpochKey, MemberSecret, Rotation};
use proptest::prelude::*;
use records::{PublicKey, SigningKey};

fn identity(seed: [u8; 32]) -> (SigningKey, PublicKey) {
    let key = SigningKey::from_bytes(&seed);
    let public = PublicKey::from_bytes(key.verifying_key().to_bytes());
    (key, public)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Every recipient unwraps the same key; anyone else gets nothing.
    #[test]
    fn wrap_unwrap_round_trip(
        seeds in prop::collection::btree_set(any::<[u8; 32]>(), 1..12),
        outsider in any::<[u8; 32]>(),
        rng_seed in any::<u64>(),
    ) {
        prop_assume!(!seeds.contains(&outsider));
        let people: Vec<(SigningKey, PublicKey)> = seeds.iter().map(|s| identity(*s)).collect();
        let recipients: BTreeSet<PublicKey> = people.iter().map(|(_, p)| *p).collect();
        let (body, key) =
            Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &recipients, &mut rng(rng_seed)).unwrap();
        prop_assert_eq!(body.len(), groupkeys::ROTATION_HEADER_LEN - 32 + groupkeys::WRAP_LEN * recipients.len());
        let signed = sign(1, body);
        let rotation = Rotation::decode(signed.id, SERVER, signed.author, &signed.body).unwrap();
        prop_assert_eq!(rotation.recipients().copied().collect::<BTreeSet<_>>(), recipients);
        for (secret, _) in &people {
            prop_assert_eq!(*rotation.unwrap_key(&MemberSecret::new(secret)).unwrap().as_bytes(), *key.as_bytes());
        }
        let (outsider, _) = identity(outsider);
        prop_assert!(rotation.unwrap_key(&MemberSecret::new(&outsider)).is_err());
    }

    /// A rotation body with any one byte changed never hands anyone a
    /// different key: it fails to decode, or the affected wraps fail.
    #[test]
    fn tampered_rotation_never_yields_another_key(
        seeds in prop::collection::btree_set(any::<[u8; 32]>(), 1..6),
        at in any::<prop::sample::Index>(),
        flip in 1..=255u8,
        rng_seed in any::<u64>(),
    ) {
        let people: Vec<(SigningKey, PublicKey)> = seeds.iter().map(|s| identity(*s)).collect();
        let recipients: BTreeSet<PublicKey> = people.iter().map(|(_, p)| *p).collect();
        let (mut body, key) =
            Rotation::create(SERVER, pk(1), CHANNEL, 0, None, &recipients, &mut rng(rng_seed)).unwrap();
        let at = at.index(body.len());
        body[at] ^= flip;
        if let Ok(rotation) = Rotation::decode(groupkeys_id(&body), SERVER, pk(1), &body) {
            let mut opened = 0;
            for (secret, _) in &people {
                if let Ok(got) = rotation.unwrap_key(&MemberSecret::new(secret)) {
                    prop_assert_eq!(*got.as_bytes(), *key.as_bytes());
                    opened += 1;
                }
            }
            prop_assert!(opened < people.len(), "byte {} changed and every wrap still opened", at);
        }
    }

    /// Through a channel view: whoever is a recipient applies and reads.
    #[test]
    fn channel_round_trip(
        member_count in 1u8..8,
        sender in 0u8..8,
        text in prop::collection::vec(any::<u8>(), 0..512),
        rng_seed in any::<u64>(),
    ) {
        let sender = 1 + sender % member_count;
        let everyone = members(&(1..=member_count).collect::<Vec<_>>());
        let first = rotate(&peer(1), 1, &everyone, rng_seed);
        let mut views: Vec<_> = (1..=member_count).map(peer).collect();
        for view in &mut views {
            prop_assert_eq!(apply(view, &first), Applied::Key);
        }
        let ciphertext =
            views[usize::from(sender - 1)].encrypt(&everyone, &text, &mut rng(rng_seed ^ 1)).unwrap();
        for view in &views {
            prop_assert_eq!(view.decrypt(pk(sender), &ciphertext).unwrap(), text.clone());
        }
        let mut outsider = peer(200);
        prop_assert_eq!(apply(&mut outsider, &first), Applied::NotRecipient);
        prop_assert!(outsider.decrypt(pk(sender), &ciphertext).is_err());
    }

    /// seal/open round-trips, and any change to the ciphertext is caught.
    #[test]
    fn seal_open_rejects_any_change(
        key_bytes in any::<[u8; 32]>(),
        text in prop::collection::vec(any::<u8>(), 0..256),
        at in any::<prop::sample::Index>(),
        flip in 1..=255u8,
        rng_seed in any::<u64>(),
    ) {
        let key = EpochKey::from_bytes(key_bytes);
        let first = sign(1, vec![1]);
        let ctx = groupkeys::MessageContext {
            server: SERVER,
            channel: CHANNEL,
            epoch: 3,
            rotation: first.id,
            author: pk(2),
        };
        let mut ciphertext = seal(&key, &ctx, &text, &mut rng(rng_seed)).unwrap();
        prop_assert_eq!(open(&key, &ctx, &ciphertext).unwrap(), text);
        let at = at.index(ciphertext.len());
        ciphertext[at] ^= flip;
        prop_assert!(open(&key, &ctx, &ciphertext).is_err());
    }
}

fn groupkeys_id(body: &[u8]) -> records::RecordId {
    records::RecordId::from_bytes(*blake3::hash(body).as_bytes())
}
