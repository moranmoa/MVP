//! The 4,000-code-point message limit (B6, QA-17) against the 16 KiB record
//! body limit: the longest text, sealed by `groupkeys` and signed with every
//! dep slot used, must still make a valid record.

use std::collections::BTreeSet;
use std::sync::Arc;

use authority::{message_text_ok, MAX_MESSAGE_CODE_POINTS};
use groupkeys::{ChannelKeys, MemberSecret, MESSAGE_OVERHEAD};
use rand_core::OsRng;
use records::{
    ChannelId, Draft, Kind, PublicKey, Record, RecordId, Scope, ServerId, SigningKey,
    MAX_AUTH_DEPS, MAX_DEPS, MAX_RECORD_BYTES,
};

#[test]
fn the_longest_message_fits_in_a_record_when_sealed() {
    // 4,000 four-byte characters: the most UTF-8 bytes the limit allows.
    let text = "😀".repeat(MAX_MESSAGE_CODE_POINTS);
    assert!(message_text_ok(&text));
    assert_eq!(text.len(), 16_000);

    let me = SigningKey::from_bytes(&[1; 32]);
    let my_key = PublicKey::from_bytes(me.verifying_key().to_bytes());
    let server = ServerId::from_bytes([2; 32]);
    let channel = ChannelId::from_bytes([3; 32]);
    let mut keys = ChannelKeys::new(server, channel, Arc::new(MemberSecret::new(&me)));
    let members = BTreeSet::from([my_key]);
    let parts = keys.rotate(my_key, &members, &mut OsRng).expect("rotation");
    let ids: Vec<RecordId> =
        (0..parts.len()).map(|i| RecordId::from_bytes([10 + i as u8; 32])).collect();
    for (id, part) in ids.iter().zip(&parts) {
        keys.apply(*id, my_key, part).expect("own rotation applies");
    }
    keys.set_accepted(ids);
    let body = keys.encrypt(&members, text.as_bytes(), &mut OsRng).expect("sealed");
    assert_eq!(body.len(), text.len() + MESSAGE_OVERHEAD);
    assert!(body.len() <= Kind::Message.max_body(), "{} bytes sealed", body.len());

    let record = Draft {
        server,
        scope: Scope::Channel(channel),
        seq: u64::MAX,
        deps: (0..MAX_DEPS as u8).map(|n| RecordId::from_bytes([n; 32])).collect(),
        auth_deps: (0..MAX_AUTH_DEPS as u8).map(|n| RecordId::from_bytes([100 + n; 32])).collect(),
        ts: u64::MAX,
        kind: Kind::Message,
        body,
    }
    .sign(&me)
    .expect("the longest sealed message is a valid record");
    let bytes = record.encode();
    assert!(bytes.len() <= MAX_RECORD_BYTES);
    assert_eq!(Record::decode(&bytes).unwrap(), record);
    println!(
        "longest message: {} B text, {} B sealed of {} B allowed, {} B record",
        16_000,
        record.body().len(),
        Kind::Message.max_body(),
        bytes.len()
    );
}
