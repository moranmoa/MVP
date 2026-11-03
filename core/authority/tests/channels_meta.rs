//! Channel records in the control log: a new server's first channel named
//! in its creator's language (UX-04), and who may create channels (ש-10).
//! Every test runs the reference `resolve` and the incremental `Resolver`
//! over many delivery orders and requires them to agree.

mod common;

use authority::{
    channel_name_ok, resolve, sign_new_server, Action, Channel, ChannelId, Record, Resolver, Role,
    State, MAX_CHANNEL_NAME,
};
use common::{actor, founded, key, rec, server, NONCE};
mod rng;
use records::{Draft, Kind, Scope};
use rng::{SliceRandom, StdRng};

const ORDERS: u64 = 200;
const GENERAL: ChannelId = ChannelId::from_bytes([5; 32]);
const RANDOM: ChannelId = ChannelId::from_bytes([6; 32]);

fn all_orders(records: &[Record]) -> State {
    let expected = resolve(server(), records);
    let mut rng = StdRng::seed_from_u64(13);
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

fn create(
    author: u8,
    seq: u64,
    deps: Vec<authority::RecordId>,
    channel: ChannelId,
    name: &str,
) -> Record {
    rec(author, seq, deps, Action::ChannelCreate { channel, adult: false, name: name.into() })
}

#[test]
fn a_new_server_starts_with_its_first_channel_in_the_creators_language() {
    let [genesis, general] =
        sign_new_server(&key(0), NONCE, 0, b"test server", GENERAL, "כללי").unwrap();
    assert_eq!(genesis.server(), server());
    let state = all_orders(&[genesis, general.clone()]);
    assert_eq!(
        state.channels,
        [(
            GENERAL,
            Channel { name: "כללי".into(), adult: false, general: true, created: general.id() }
        )]
        .into()
    );
    assert_eq!(state.owner, Some(actor(0)));
}

#[test]
fn only_the_owner_creates_channels() {
    let (mut records, heads) = founded(&[1, 2]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    let by_admin = create(1, 1, vec![admin.id()], RANDOM, "random");
    let by_member = create(2, 1, vec![admin.id()], RANDOM, "random");
    let by_owner = rec(
        0,
        3,
        vec![admin.id()],
        Action::ChannelCreate { channel: RANDOM, adult: true, name: "after-dark".into() },
    );
    records.extend([admin, by_admin.clone(), by_member.clone(), by_owner.clone()]);
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&by_admin.id()) && !state.accepted.contains(&by_member.id()));
    assert_eq!(state.channels[&RANDOM].name, "after-dark");
    assert!(state.channels[&RANDOM].adult);
}

/// A channel ID is created once. Two concurrent creates of one ID: the first
/// in linear order wins, whatever the delivery order.
#[test]
fn a_channel_id_is_created_once() {
    let (mut records, heads) = founded(&[]);
    let first = create(0, 2, heads.clone(), RANDOM, "one");
    let concurrent = create(0, 3, heads.clone(), RANDOM, "two");
    let again = create(0, 4, vec![first.id(), concurrent.id()], RANDOM, "three");
    records.extend([first.clone(), concurrent.clone(), again.clone()]);
    let state = all_orders(&records);
    let winner = if first.id() < concurrent.id() { &first } else { &concurrent };
    assert_eq!(state.channels[&RANDOM].created, winner.id());
    assert_eq!(
        state
            .accepted
            .iter()
            .filter(|id| [first.id(), concurrent.id(), again.id()].contains(id))
            .count(),
        1
    );
}

#[test]
fn channel_names_count_code_points() {
    assert!(channel_name_ok(&"ש".repeat(MAX_CHANNEL_NAME)));
    assert!(!channel_name_ok(&"ש".repeat(MAX_CHANNEL_NAME + 1)));
    assert!(!channel_name_ok(""));
    assert!(channel_name_ok("😀"));
}

/// Bodies `encode` never produces, or reserved fields set: never accepted.
#[test]
fn hostile_channel_bodies_are_never_accepted() {
    let (mut records, heads) = founded(&[]);
    let id = RANDOM.as_bytes().to_vec();
    let bodies: Vec<Vec<u8>> = vec![
        vec![],
        id[..31].to_vec(),
        [id.clone(), vec![0, 0]].concat(), // empty name
        [id.clone(), vec![1, 0], b"x".to_vec()].concat(), // history byte set (D30)
        [id.clone(), vec![0, 2], b"x".to_vec()].concat(), // unknown flag
        [id.clone(), vec![0, 0], vec![0xff, 0xfe]].concat(), // not UTF-8
        [id.clone(), vec![0, 0], "a".repeat(33).into_bytes()].concat(),
    ];
    let mut refused = Vec::new();
    for (seq, body) in bodies.into_iter().enumerate() {
        let record = Draft {
            server: server(),
            scope: Scope::Control,
            seq: 10 + seq as u64,
            deps: heads.clone(),
            auth_deps: vec![],
            ts: 0,
            kind: Kind::ChannelCreate,
            body,
        }
        .sign(&key(0))
        .unwrap();
        assert_eq!(Action::from_record(&record), None);
        refused.push(record);
    }
    let good = create(0, 30, heads, RANDOM, "ok");
    assert_eq!(
        Action::from_record(&good),
        Some(Action::ChannelCreate { channel: RANDOM, adult: false, name: "ok".into() })
    );
    records.extend(refused);
    let state = all_orders(&records);
    assert!(state.channels.is_empty());
}

fn edit(seq: u64, deps: Vec<authority::RecordId>, channel: ChannelId, name: &str) -> Record {
    rec(0, seq, deps, Action::ChannelEdit { channel, adult: false, name: name.into() })
}

fn delete(author: u8, seq: u64, deps: Vec<authority::RecordId>, channel: ChannelId) -> Record {
    rec(author, seq, deps, Action::ChannelDelete { channel })
}

/// B4-3: only the owner renames or deletes channels (ש-10); the latest
/// rename in linear order counts.
#[test]
fn the_owner_renames_and_deletes_channels() {
    let (mut records, heads) = founded(&[1]);
    let admin = rec(0, 2, heads, Action::Grant { target: actor(1), role: Role::Admin });
    // #general first: the server's first channel can't be deleted.
    let general = create(0, 20, vec![admin.id()], GENERAL, "general");
    let created = create(0, 3, vec![general.id()], RANDOM, "random");
    let by_admin = rec(
        1,
        1,
        vec![created.id()],
        Action::ChannelEdit { channel: RANDOM, adult: true, name: "x".into() },
    );
    let first = edit(4, vec![created.id()], RANDOM, "one");
    let second = edit(5, vec![created.id()], RANDOM, "two");
    records.extend([admin, general, created, by_admin.clone(), first.clone(), second.clone()]);
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&by_admin.id()), "admins don't manage channels");
    let latest = if first.id() > second.id() { "one" } else { "two" };
    assert_eq!(state.channels[&RANDOM].name, latest, "the later rename in linear order counts");

    let later = edit(6, vec![first.id(), second.id()], RANDOM, "three");
    let gone = delete(0, 7, vec![later.id()], RANDOM);
    let after = edit(8, vec![gone.id()], RANDOM, "ghost");
    let again = create(0, 9, vec![gone.id()], RANDOM, "again");
    let by_member = delete(1, 2, vec![later.id()], RANDOM);
    records.extend([later, gone.clone(), after.clone(), again.clone(), by_member.clone()]);
    let state = all_orders(&records);
    assert!(state.accepted.contains(&gone.id()));
    assert!(!state.channels.contains_key(&RANDOM));
    assert!(state.deleted_channels.contains(&RANDOM));
    for refused in [after, again, by_member] {
        assert!(!state.accepted.contains(&refused.id()), "no edit, re-create or member delete");
    }
}

/// ש-20: #general, the server's first channel, can't be deleted; later
/// channels are ordinary.
#[test]
fn general_stays() {
    let [genesis, general] =
        sign_new_server(&key(0), NONCE, 0, b"test server", GENERAL, "general").unwrap();
    let delete_general = delete(0, 2, vec![general.id()], GENERAL);
    let renamed = edit(3, vec![general.id()], GENERAL, "כללי");
    let second = rec(
        0,
        4,
        vec![general.id()],
        Action::ChannelCreate { channel: RANDOM, adult: false, name: "other".into() },
    );
    let records = [genesis, general, delete_general.clone(), renamed, second.clone()];
    let state = all_orders(&records);
    assert!(!state.accepted.contains(&delete_general.id()), "#general can't be deleted");
    assert!(state.accepted.contains(&second.id()));
    assert!(!state.channels[&RANDOM].general, "only the first channel is #general");
    assert_eq!(state.channels[&GENERAL].name, "כללי", "but it can be renamed");
    assert!(state.channels[&GENERAL].general);
}

#[test]
fn channel_edit_and_delete_bodies_are_strict() {
    let (mut records, heads) = founded(&[]);
    let created = create(0, 2, heads, RANDOM, "random");
    let id = RANDOM.as_bytes().to_vec();
    let bodies: Vec<(Kind, Vec<u8>)> = vec![
        (Kind::ChannelEdit, [id.clone(), vec![0, 2], b"x".to_vec()].concat()), // the #general bit
        (Kind::ChannelEdit, [id.clone(), vec![1, 0], b"x".to_vec()].concat()), // history set
        (Kind::ChannelEdit, [id.clone(), vec![0, 0]].concat()),                // empty name
        (Kind::ChannelDelete, id[..31].to_vec()),
        (Kind::ChannelDelete, [id.clone(), vec![0]].concat()),
    ];
    let mut refused = Vec::new();
    for (seq, (kind, body)) in bodies.into_iter().enumerate() {
        let record = Draft {
            server: server(),
            scope: Scope::Control,
            seq: 10 + seq as u64,
            deps: vec![created.id()],
            auth_deps: vec![],
            ts: 0,
            kind,
            body,
        }
        .sign(&key(0))
        .unwrap();
        assert_eq!(Action::from_record(&record), None);
        refused.push(record);
    }
    records.push(created);
    records.extend(refused);
    let state = all_orders(&records);
    assert_eq!(state.channels[&RANDOM].name, "random");
}
