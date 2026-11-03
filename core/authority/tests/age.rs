//! Age claims (B7): a member signs only "18+ or not", never a birth date,
//! and their latest claim in linear order counts. Every test runs the
//! reference `resolve` and the incremental `Resolver` over many delivery
//! orders and requires them to agree.

mod common;
mod rng;

use authority::{resolve, Action, Record, Resolver, State};
use common::{actor, founded, key, rec, server};
use records::{Draft, Kind, Scope};
use rng::{SliceRandom, StdRng};

const ORDERS: u64 = 200;

fn all_orders(records: &[Record]) -> State {
    let expected = resolve(server(), records);
    let mut rng = StdRng::seed_from_u64(17);
    let mut arriving = records.to_vec();
    for round in 0..=ORDERS {
        if round > 0 {
            arriving.shuffle(&mut rng);
        }
        assert_eq!(resolve(server(), &arriving), expected, "resolve depends on order");
        let mut resolver = Resolver::new(server());
        for record in &arriving {
            resolver.insert(record.clone());
        }
        assert_eq!(resolver.state(), &expected, "incremental disagrees with resolve");
    }
    expected
}

fn claim(author: u8, seq: u64, deps: Vec<authority::RecordId>, adult: bool) -> Record {
    rec(author, seq, deps, Action::AgeClaim { adult })
}

#[test]
fn the_latest_claim_counts() {
    let (mut records, heads) = founded(&[1, 2]);
    // 1 turns 18; 2 corrects a wrong date down to under 18.
    let minor = claim(1, 1, heads.clone(), false);
    let grown = claim(1, 2, vec![minor.id()], true);
    let adult = claim(2, 1, heads.clone(), true);
    let corrected = claim(2, 2, vec![adult.id()], false);
    records.extend([minor, grown, adult, corrected]);
    let state = all_orders(&records);
    assert_eq!(state.adults, [actor(1)].into());
}

/// Two claims that don't see each other: the later one in linear order
/// counts, the same in every delivery order.
#[test]
fn concurrent_claims_are_settled_by_linear_order() {
    let (mut records, heads) = founded(&[1]);
    let yes = claim(1, 1, heads.clone(), true);
    let no = claim(1, 2, heads.clone(), false);
    records.extend([yes.clone(), no.clone()]);
    let state = all_orders(&records);
    assert_eq!(state.adults.contains(&actor(1)), yes.id() > no.id());
}

#[test]
fn only_members_claim_and_a_claim_changes_nothing_else() {
    let (mut records, heads) = founded(&[1]);
    let outsider = claim(9, 0, heads.clone(), true);
    let kick = rec(0, 2, heads.clone(), Action::kick(actor(1)));
    let after_kick = claim(1, 1, vec![kick.id()], true);
    let backdated = claim(1, 2, heads.clone(), true);
    records.extend([outsider.clone(), kick, after_kick.clone(), backdated.clone()]);
    let state = all_orders(&records);
    assert!(state.adults.is_empty());
    for refused in [outsider, after_kick, backdated] {
        assert!(!state.accepted.contains(&refused.id()));
    }

    let (mut records, heads) = founded(&[1]);
    let before = resolve(server(), &records);
    let yes = claim(1, 1, heads, true);
    records.push(yes.clone());
    let state = all_orders(&records);
    assert_eq!(state.members, before.members);
    assert_eq!(state.roles, before.roles);
    assert!(state.accepted.contains(&yes.id()));
}

/// The body is exactly one byte, 0 or 1: there is no room for a birth date.
#[test]
fn a_claim_carries_nothing_but_18_plus() {
    let (mut records, heads) = founded(&[1]);
    let bodies: [&[u8]; 5] = [&[], &[2], &[1, 0], &[0x07, 0xd0, 5, 17], b"2008-05-17"];
    let mut refused = Vec::new();
    for (seq, body) in bodies.into_iter().enumerate() {
        let record = Draft {
            server: server(),
            scope: Scope::Control,
            seq: 10 + seq as u64,
            deps: heads.clone(),
            auth_deps: vec![],
            ts: 0,
            kind: Kind::AgeClaim,
            body: body.to_vec(),
        }
        .sign(&key(1))
        .unwrap();
        assert_eq!(Action::from_record(&record), None);
        refused.push(record);
    }
    assert_eq!(Action::AgeClaim { adult: true }.body(), vec![1]);
    records.extend(refused);
    let state = all_orders(&records);
    assert!(state.adults.is_empty());
}
