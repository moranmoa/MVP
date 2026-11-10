//! End-to-end flows through the typed API.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use api::*;
use common::{Peer, World};
use identity::MemoryStore;
use records::{Draft, Kind, PublicKey, Scope, ServerId, SigningKey};

const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

fn adult() -> NewIdentityInput {
    NewIdentityInput { locale: Locale::He, country: "IL".into(), birth_date: "1990-05-01".into() }
}

/// Answers the current challenge, with "zzzz" at the indexes (into its
/// positions) listed in `wrong`. Returns the positions and the result.
fn answer(
    peer: &Peer,
    words: &[String],
    wrong: &[usize],
) -> (Vec<usize>, CoreResult<RecoveryCheckResult>) {
    let positions = peer.core.get_recovery_challenge().unwrap().positions;
    let answers: BTreeMap<usize, String> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| (*p, if wrong.contains(&i) { "zzzz".into() } else { words[p - 1].clone() }))
        .collect();
    (positions, peer.core.check_recovery_words(answers))
}

/// Creates an identity and passes the recovery check.
fn sign_up(peer: &Peer, name: &str) -> Identity {
    let created = peer.core.create_identity(adult()).unwrap();
    assert!(answer(peer, &created.words, &[]).1.unwrap().ok);
    peer.core
        .update_profile(ProfileInput {
            display_name: name.into(),
            bio: None,
            avatar_data_url: None,
        })
        .unwrap()
}

fn names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            Event::IdentityProgress { .. } => "identity.progress",
            Event::IdentityChanged(_) => "identity.changed",
            Event::ServerAdded(_) => "server.added",
            Event::ServerUpdated(_) => "server.updated",
            Event::ServerRemoved { .. } => "server.removed",
            Event::ServerClosed { .. } => "server.closed",
            Event::ChannelUpdated(_) => "channel.updated",
            Event::ChannelAdded(_) => "channel.added",
            Event::ChannelRemoved { .. } => "channel.removed",
            Event::MembersChanged { .. } => "members.changed",
            Event::JoinStatus(_) => "join.status",
            Event::UpdateReady(_) => "update.ready",
            Event::UpdateInstalled { .. } => "update.installed",
            Event::UpdateRolledBack { .. } => "update.rolledBack",
            Event::Notice { .. } => "notice",
            Event::MessageAdded(_) => "message.added",
            Event::MessageUpdated(_) => "message.updated",
            Event::CallState(_) => "call.state",
            Event::CallSignal { .. } => "call.signal",
            Event::VoiceRoom(_) => "voice.room",
        })
        .collect()
}

/// Says "renewed" on every change, to check the notice is wired.
struct RenewingCipher(std::sync::Mutex<Vec<BTreeSet<PublicKey>>>);

impl GroupCipher for RenewingCipher {
    fn membership_changed(&self, _: ServerId, members: &BTreeSet<PublicKey>) -> CipherOutcome {
        self.0.lock().unwrap().push(members.clone());
        CipherOutcome::Renewed
    }
}

#[test]
fn create_identity_server_grant_kick() {
    let world = World::new();
    let cipher = Arc::new(RenewingCipher(Default::default()));
    let owner = world.peer_with(Arc::new(MemoryStore::new()), None, cipher.clone());
    let alice = world.peer();
    let bob = world.peer();

    // Sign-up: progress events, then the gate.
    let created = owner.core.create_identity(adult()).unwrap();
    assert_eq!(created.words.len(), 12);
    assert!(!created.identity.recovery_checked);
    assert_eq!(names(&owner.events.take()), ["identity.progress"; 4]);
    assert_eq!(
        owner.core.create_server("Team", AgeRestriction::None).unwrap_err().code,
        ErrorCode::RecoveryCheckRequired
    );
    assert_eq!(answer(&owner, &created.words, &[]).1.unwrap(), RecoveryCheckResult::passed());
    assert!(owner.core.get_identity().unwrap().unwrap().recovery_checked);

    let server = owner.core.create_server("Team", AgeRestriction::None).unwrap();
    assert_eq!(server.owner_id, created.identity.id);
    assert!(server.invite_link.starts_with("mvs://join/"));
    // Named in the creator's language (Hebrew here), as stored (UX-04).
    assert_eq!(owner.core.list_channels(&server.id).unwrap()[0].name, "כללי");

    let alice_id = sign_up(&alice, "Alice").id;
    let bob_id = sign_up(&bob, "Bob").id;
    for peer in [&alice, &bob] {
        peer.events.take();
        let first = peer.core.join_server(&server.invite_link).unwrap();
        assert!(matches!(first, JoinStatus::Searching { .. }));
        let events = peer.events.take();
        assert_eq!(names(&events), ["join.status", "server.added", "join.status"]);
        assert!(
            matches!(&events[2], Event::JoinStatus(JoinStatus::Joined { server_id, .. }) if *server_id == server.id)
        );
    }

    owner.core.sync_now().unwrap();
    let members = owner.core.list_members(&server.id).unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0].role, Role::Owner);
    assert!(members.iter().any(|m| m.id == bob_id && m.display_name == "Bob"));

    // A member can't kick; a moderator can kick a member, not the owner.
    // A refused action is never sent.
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let sent = world.network.fetch(server_key).len();
    assert_eq!(
        alice.core.kick_member(&server.id, &bob_id).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(world.network.fetch(server_key).len(), sent);
    assert_eq!(
        alice.core.grant_role(&server.id, &bob_id, Role::Moderator).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    owner.core.grant_role(&server.id, &alice_id, Role::Moderator).unwrap();
    alice.core.sync_now().unwrap();
    assert_eq!(
        alice.core.kick_member(&server.id, &server.owner_id).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    alice.core.kick_member(&server.id, &bob_id).unwrap();

    // Bob learns he was removed; the owner sees two members.
    bob.events.take();
    bob.core.sync_now().unwrap();
    assert_eq!(names(&bob.events.take()), ["server.removed"]);
    assert!(bob.core.list_servers().unwrap().is_empty());
    assert_eq!(bob.core.list_members(&server.id).unwrap_err().code, ErrorCode::NotFound);
    assert!(matches!(
        bob.core.join_server(&server.invite_link).unwrap(),
        JoinStatus::Searching { .. }
    ));
    assert!(matches!(
        bob.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Invalid { .. }))
    ));

    owner.core.sync_now().unwrap();
    let ids: Vec<String> =
        owner.core.list_members(&server.id).unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, [server.owner_id.clone(), alice_id.clone()]);

    // The owner's own kick reaches the group cipher, and the UI gets a notice.
    owner.core.sync_now().unwrap();
    owner.events.take();
    let dave = world.peer();
    let dave_id = sign_up(&dave, "Dave").id;
    dave.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    owner.events.take();
    owner.core.kick_member(&server.id, &dave_id).unwrap();
    // This stand-in cipher never says a key is held, so no rotation and no
    // notice here; the real re-key is in `a_kick_re_keys_the_channel`.
    assert_eq!(names(&owner.events.take()), ["members.changed"]);
    let last = cipher.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.len(), 2);
}

#[test]
fn a_genesis_must_belong_to_its_server_id() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let real = owner.core.create_server("Real", AgeRestriction::None).unwrap();

    // Someone else can't even sign a genesis for the real server's ID
    // (`records` binds the ID to the owner's key).
    let mallory = SigningKey::from_bytes(&[5; 32]);
    let real_id = ServerId::from_bytes(hex::decode(&real.id).unwrap().try_into().unwrap());
    let stolen = Draft {
        server: real_id,
        scope: Scope::Control,
        seq: 0,
        deps: vec![],
        auth_deps: vec![],
        ts: 1,
        kind: Kind::Genesis,
        body: [[0u8; 16].as_slice(), br#"{"v":1,"name":"Fake","ageRestriction":"none"}"#].concat(),
    }
    .sign(&mallory);
    assert!(stolen.is_err());

    // Records for a server ID nobody wrote a genesis for.
    let forged_id = ServerId::from_bytes([9; 32]);
    let orphan = authority::sign_control(
        &mallory,
        forged_id,
        0,
        vec![],
        1,
        &authority::Action::Post { text: "hi".into() },
    )
    .unwrap();
    world.network.publish(&orphan);

    let joiner = world.peer();
    sign_up(&joiner, "Joiner");
    let link = format!(
        "mvs://join/{}/{}/{}",
        hex::encode(forged_id.as_bytes()),
        "00".repeat(32),
        "00".repeat(32)
    );
    joiner.core.join_server(&link).unwrap();
    assert!(matches!(
        joiner.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Invalid { .. }))
    ));
    assert_eq!(joiner.core.list_servers().unwrap().len(), 0);

    // Unknown server, and a link that isn't one.
    joiner
        .core
        .join_server(&format!(
            "mvs://join/{}/{}/{}",
            "ab".repeat(32),
            "00".repeat(32),
            "00".repeat(32)
        ))
        .unwrap();
    assert!(matches!(
        joiner.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::NobodyOnline { .. }))
    ));
    assert!(matches!(
        joiner.core.join_server("https://example.com").unwrap(),
        JoinStatus::Invalid { .. }
    ));
    assert_eq!(real.name, "Real");
}

#[test]
fn hostile_server_records_are_refused() {
    let world = World::new();
    let joiner = world.peer();
    sign_up(&joiner, "Joiner");
    let mallory = SigningKey::from_bytes(&[6; 32]);

    // A real genesis whose metadata isn't ours: garbage, wrong version, huge seq.
    for metadata in [&b"\xff\xfe"[..], br#"{"v":2,"name":"x","ageRestriction":"none"}"#, b""] {
        let genesis =
            authority::sign_genesis(&mallory, [metadata.len() as u8; 16], 1, metadata).unwrap();
        world.network.publish(&genesis);
        let link = format!(
            "mvs://join/{}/{}/{}",
            hex::encode(genesis.server().as_bytes()),
            "00".repeat(32),
            "00".repeat(32)
        );
        joiner.core.join_server(&link).unwrap();
        assert!(matches!(
            joiner.events.take().last(),
            Some(Event::JoinStatus(JoinStatus::Invalid { .. }))
        ));
    }

    // A stranger's post with the largest seq and a body that isn't a join:
    // stored, harmless, and not a member.
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let server = owner.core.create_server("S", AgeRestriction::None).unwrap();
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let heads: Vec<_> = world.network.fetch(server_key).iter().map(|r| r.id()).collect();
    let me = SigningKey::from_bytes(&[7; 32]);
    let post = authority::sign_control(
        &me,
        server_key,
        u64::MAX,
        heads,
        1,
        &authority::Action::Post { text: "{not json".into() },
    )
    .unwrap();
    world.network.publish(&post);
    owner.core.sync_now().unwrap();
    assert_eq!(owner.core.list_members(&server.id).unwrap().len(), 1);

    // A real server with an invite ID it never issued: the join is refused,
    // nothing is stored or sent.
    let sent = world.network.fetch(server_key).len();
    let bogus = format!("mvs://join/{}/{}/{}", server.id, "11".repeat(32), "00".repeat(32));
    joiner.core.join_server(&bogus).unwrap();
    assert!(matches!(
        joiner.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Invalid { .. }))
    ));
    assert_eq!(world.network.fetch(server_key).len(), sent);
    assert!(joiner.core.list_servers().unwrap().is_empty());

    // Names: empty and too long are refused.
    for name in ["   ".to_owned(), "x".repeat(101)] {
        assert_eq!(
            owner.core.create_server(&name, AgeRestriction::None).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }
}

/// A record whose deps never arrive is neither a head nor stored, so it
/// can't freeze our own control actions (review of #18).
#[test]
fn an_orphan_does_not_block_control_actions() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let member = world.peer();
    let member_id = sign_up(&member, "Member").id;
    let server = owner.core.create_server("S", AgeRestriction::None).unwrap();
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());

    // Orphans in the network before a join and after it.
    let mallory = SigningKey::from_bytes(&[8; 32]);
    let orphan = |seq: u64| {
        authority::sign_control(
            &mallory,
            server_key,
            seq,
            vec![records::RecordId::from_bytes([seq as u8 + 1; 32])],
            1,
            &authority::Action::Post { text: "x".into() },
        )
        .unwrap()
    };
    world.network.publish(&orphan(0));
    member.core.join_server(&server.invite_link).unwrap();
    assert_eq!(member.core.list_servers().unwrap().len(), 1);
    world.network.publish(&orphan(1));
    owner.core.sync_now().unwrap();
    assert_eq!(owner.core.list_members(&server.id).unwrap().len(), 2);

    owner.core.grant_role(&server.id, &member_id, Role::Moderator).unwrap();
    owner.core.kick_member(&server.id, &member_id).unwrap();
    assert_eq!(owner.core.list_members(&server.id).unwrap().len(), 1);
}

/// The words leave the core only until the check passes, then never again,
/// not even after a restart (security review of #18).
#[test]
fn recovery_words_only_until_the_check_passes() {
    let world = World::new();
    let secrets = Arc::new(MemoryStore::new());
    let dir = std::env::temp_dir().join(format!("mvs-api-words-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("core.db");
    let _ = std::fs::remove_file(&db);
    let peer = world.peer_with(secrets.clone(), Some(db.clone()), Arc::new(PendingGroupKeys));

    let created = peer.core.create_identity(adult()).unwrap();
    assert_eq!(peer.core.get_recovery_words().unwrap(), created.words);
    let (_, result) = answer(&peer, &created.words, &[0]);
    assert!(!result.unwrap().ok);
    assert_eq!(peer.core.get_recovery_words().unwrap(), created.words);

    // Before the check, a restart still hands them out.
    drop(peer);
    let peer = world.peer_with(secrets.clone(), Some(db.clone()), Arc::new(PendingGroupKeys));
    assert_eq!(peer.core.get_recovery_words().unwrap(), created.words);

    let (positions, result) = answer(&peer, &created.words, &[]);
    assert!(result.unwrap().ok);
    assert_eq!(peer.core.get_recovery_words().unwrap_err().code, ErrorCode::NotAllowed);
    assert_eq!(peer.core.get_recovery_challenge().unwrap_err().code, ErrorCode::NotAllowed);
    let again: BTreeMap<usize, String> =
        positions.iter().map(|p| (*p, created.words[p - 1].clone())).collect();
    assert_eq!(peer.core.check_recovery_words(again).unwrap_err().code, ErrorCode::NotAllowed);

    drop(peer);
    let peer = world.peer_with(secrets, Some(db), Arc::new(PendingGroupKeys));
    assert!(peer.core.get_identity().unwrap().unwrap().recovery_checked);
    assert_eq!(peer.core.get_recovery_words().unwrap_err().code, ErrorCode::NotAllowed);
    drop(peer);
    let _ = std::fs::remove_dir_all(&dir);
}

/// B19 §6 / #31: a stable challenge of 3 positions, only its positions
/// accepted, the wrong ones reported, burned on the 5th failure (which
/// itself rejects), across a restart, until getRecoveryWords lifts it.
#[test]
fn the_recovery_challenge_burns_after_five_failures() {
    let world = World::new();
    let secrets = Arc::new(MemoryStore::new());
    let dir = std::env::temp_dir().join(format!("mvs-api-challenge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("core.db");
    let _ = std::fs::remove_file(&db);
    let open = || world.peer_with(secrets.clone(), Some(db.clone()), Arc::new(PendingGroupKeys));
    let peer = open();
    assert_eq!(peer.core.get_recovery_challenge().unwrap_err().code, ErrorCode::NotFound);
    let words = peer.core.create_identity(adult()).unwrap().words;

    let first = peer.core.get_recovery_challenge().unwrap().positions;
    assert_eq!(first.len(), CHALLENGE_POSITIONS);
    assert!(first.windows(2).all(|w| w[0] < w[1]) && first.iter().all(|p| (1..=12).contains(p)));
    assert_eq!(peer.core.get_recovery_challenge().unwrap().positions, first);

    // Other positions (or some missing): not-allowed, and not a failure.
    let other: BTreeMap<usize, String> = (1..=12)
        .filter(|p| !first.contains(p))
        .take(3)
        .map(|p| (p, words[p - 1].clone()))
        .collect();
    let fewer: BTreeMap<usize, String> = [(first[0], words[first[0] - 1].clone())].into();
    for answers in [other, fewer, BTreeMap::new()] {
        assert_eq!(
            peer.core.check_recovery_words(answers).unwrap_err().code,
            ErrorCode::NotAllowed
        );
    }

    // Four failures report the wrong positions; a restart keeps the count.
    for _ in 1..MAX_CHECK_FAILURES {
        let (positions, result) = answer(&peer, &words, &[0, 2]);
        assert_eq!(positions, first);
        assert_eq!(result.unwrap(), RecoveryCheckResult::wrong(vec![first[0], first[2]]));
    }
    drop(peer);
    let peer = open();
    assert_eq!(peer.core.get_recovery_challenge().unwrap().positions, first);
    // The fifth burns it and itself rejects; so does everything after.
    let (_, result) = answer(&peer, &words, &[1]);
    assert_eq!(result.unwrap_err().code, ErrorCode::TooManyAttempts);
    assert_eq!(peer.core.get_recovery_challenge().unwrap_err().code, ErrorCode::TooManyAttempts);
    let right: BTreeMap<usize, String> = first.iter().map(|p| (*p, words[p - 1].clone())).collect();
    assert_eq!(peer.core.check_recovery_words(right).unwrap_err().code, ErrorCode::TooManyAttempts);
    assert!(!peer.core.get_identity().unwrap().unwrap().recovery_checked);

    // Still burned after a restart, until the words are shown again.
    drop(peer);
    let peer = open();
    assert_eq!(peer.core.get_recovery_challenge().unwrap_err().code, ErrorCode::TooManyAttempts);
    assert_eq!(peer.core.get_recovery_words().unwrap(), words);
    let (_, result) = answer(&peer, &words, &[]);
    assert_eq!(result.unwrap(), RecoveryCheckResult::passed());
    assert!(peer.core.get_identity().unwrap().unwrap().recovery_checked);
    drop(peer);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profile_fields_are_capped() {
    let world = World::new();
    let peer = world.peer();
    sign_up(&peer, "Me");
    let input = |name: &str, bio: Option<String>, avatar: Option<&str>| ProfileInput {
        display_name: name.into(),
        bio,
        avatar_data_url: avatar.map(String::from),
    };
    let ok = [
        input("נועה", Some("שורה\nשורה".into()), Some("data:image/png;base64,iVBORw0KGgo=")),
        input("x", None, Some("data:image/webp;base64,UklGRg==")),
    ];
    for profile in ok {
        peer.core.update_profile(profile).unwrap();
    }
    let svg = "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=";
    let bad = [
        input("   ", None, None),
        input(&"x".repeat(129), None, None),
        input("x", Some("y".repeat(1501)), None),
        input("x", Some("\n".repeat(9)), None),
        input("x", None, Some(svg)),
        input("x", None, Some("data:image/png,raw")),
        input("x", None, Some("data:image/png;base64,<script>")),
        input("x", None, Some("https://example.com/a.png")),
        input("x", None, Some(&format!("data:image/png;base64,{}", "A".repeat(700_000)))),
    ];
    for profile in bad {
        assert_eq!(peer.core.update_profile(profile).unwrap_err().code, ErrorCode::BadRequest);
    }
    assert_eq!(peer.core.get_identity().unwrap().unwrap().display_name, "x");
}

#[test]
fn age_restricted_server_blocks_minors() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let server = owner.core.create_server("Adults", AgeRestriction::Eighteen).unwrap();

    let teen = world.peer();
    teen.core
        .create_identity(NewIdentityInput {
            locale: Locale::He,
            country: "IL".into(),
            // 17 on 2026-11-03.
            birth_date: "2008-11-04".into(),
        })
        .unwrap();
    teen.events.take();
    teen.core.join_server(&server.invite_link).unwrap();
    assert!(matches!(
        teen.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::AgeBlocked { required: AgeRestriction::Eighteen, .. }))
    ));
    assert!(teen.core.list_servers().unwrap().is_empty());
}

#[test]
fn state_survives_a_restart_and_the_profile_is_sealed() {
    let dir = std::env::temp_dir().join(format!("mvs-api-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("core.sqlite");
    let _ = std::fs::remove_file(&db);
    let secrets = Arc::new(MemoryStore::new());
    let world = World::new();

    let (me, server) = {
        let peer = world.peer_with(secrets.clone(), Some(db.clone()), Arc::new(PendingGroupKeys));
        let me = sign_up(&peer, "Persistent");
        (me, peer.core.create_server("Kept", AgeRestriction::None).unwrap())
    };

    let peer = world.peer_with(secrets, Some(db.clone()), Arc::new(PendingGroupKeys));
    let again = peer.core.get_identity().unwrap().unwrap();
    assert_eq!(again, me);
    assert_eq!(again.birth_date, "1990-05-01");
    assert_eq!(peer.core.list_servers().unwrap(), [server]);
    drop(peer);

    let bytes = std::fs::read(&db).unwrap();
    // The birth date never leaves the sealed profile. (The display name is
    // in the clear on purpose: it is in the join post members read.)
    assert!(!contains(&bytes, b"1990-05-01"), "birth date stored in the clear");
    assert!(contains(&bytes, b"Persistent"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn restore_and_identity_rules() {
    let world = World::new();
    let peer = world.peer();
    let err = peer.core.restore_identity(vec!["legal".into(); 11]).unwrap_err();
    assert_eq!(err, CoreError { code: ErrorCode::InvalidWords, positions: Some(vec![]) });
    let words: Vec<String> = LEGAL.split(' ').map(String::from).collect();
    let restored = peer.core.restore_identity(words.clone()).unwrap();
    assert!(restored.recovery_checked);
    // Restoring passes the check, so the words are never handed out again.
    assert_eq!(peer.core.get_recovery_words().unwrap_err().code, ErrorCode::NotAllowed);
    assert_eq!(peer.core.create_identity(adult()).unwrap_err().code, ErrorCode::NotAllowed);
    assert_eq!(peer.core.restore_identity(words).unwrap_err().code, ErrorCode::NotAllowed);
    // A restored identity may create a server straight away.
    peer.core.create_server("Mine", AgeRestriction::None).unwrap();
}

#[test]
fn dispatcher_errors() {
    let world = World::new();
    let peer = world.peer();
    let error = |request: &str| -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(&peer.core.handle(request)).unwrap()["error"]
            ["code"]
            .clone()
    };
    assert_eq!(error("not json"), "bad-request");
    assert_eq!(error(r#"{"cmd":"nope"}"#), "bad-request");
    assert_eq!(error(r#"{"cmd":"listChannels","args":{}}"#), "bad-request");
    assert_eq!(error(r#"{"cmd":"listChannels","args":{"serverId":"zz"}}"#), "not-found");
    for cmd in NOT_YET {
        assert_eq!(error(&format!(r#"{{"cmd":"{cmd}","args":{{}}}}"#)), "not-implemented");
    }
    assert_eq!(peer.core.handle(r#"{"cmd":"getIdentity"}"#), r#"{"ok":null}"#);
    let info: serde_json::Value =
        serde_json::from_str(&peer.core.handle(r#"{"cmd":"getAppInfo"}"#)).unwrap();
    assert_eq!(
        info["ok"],
        serde_json::json!({ "version": "0.1.0-alpha", "updateChannel": "stable", "relayOnly": false })
    );
    // A bad argument is a bad request, not a change.
    assert_eq!(error(r#"{"cmd":"setRelayOnly","args":{"on":"yes"}}"#), "bad-request");
    assert!(!peer.core.get_app_info().unwrap().relay_only);
    peer.core.handle(r#"{"cmd":"setUpdateChannel","args":{"channel":"beta"}}"#);
    assert_eq!(peer.core.get_app_info().unwrap().update_channel, UpdateChannel::Beta);
}

#[test]
fn diagnostics_hold_no_secrets() {
    let world = World::new();
    let peer = world.peer();
    // A fixed phrase: a random one could hold "alpha", which the version has.
    let words: Vec<String> = LEGAL.split(' ').map(String::from).collect();
    peer.core.restore_identity(words.clone()).unwrap();
    peer.core
        .update_profile(ProfileInput {
            display_name: "Secret Name".into(),
            bio: None,
            avatar_data_url: None,
        })
        .unwrap();
    let report = peer.core.export_diagnostics().unwrap();
    assert!(!report.contains("Secret Name"));
    for word in words {
        assert!(!report.split(|c: char| !c.is_ascii_alphabetic()).any(|t| t == word), "{word}");
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Encrypted chat in process (the relay version is tests/two_peers.rs).
#[test]
fn members_chat_through_the_group_cipher() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    // Locked ciphers: the core unlocks them when the identity is restored.
    let spy = Arc::new(Spy::new());
    let open = |words: &str, name: &str, cipher: Arc<dyn GroupCipher>| {
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let owner = open(OWNER_WORDS, "Owner", spy.clone());
    let guest = open(LEGAL, "Guest", Arc::new(GroupKeysCipher::locked()));
    let server = owner.core.create_server("Chat", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;

    // The owner's sync saw the join and keyed the channel for both (a plain
    // member writing first: `a_joiner_can_write_first_once_the_owner_syncs`).
    guest.core.sync_now().unwrap();
    // Bad input, and replies (not yet).
    assert_eq!(
        owner.core.send_message(&channel, "  ", None).unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        owner.core.send_message(&channel, "x", Some("ab")).unwrap_err().code,
        ErrorCode::NotImplemented
    );
    assert_eq!(owner.core.get_message_count("nope").unwrap_err().code, ErrorCode::NotFound);

    owner.events.take();
    let sent = owner.core.send_message(&channel, "שלום", None).unwrap();
    assert_eq!((sent.index, sent.send_state), (0, Some(SendState::Sent)));
    assert_eq!(names(&owner.events.take()), ["message.added"]);

    guest.core.sync_now().unwrap();
    let got = guest.core.get_messages(&channel, 0, 10).unwrap();
    assert_eq!(got.messages.len(), 1);
    assert_eq!((got.messages[0].body.as_str(), got.messages[0].send_state), ("שלום", None));
    guest.core.send_message(&channel, "hi", None).unwrap();

    owner.core.sync_now().unwrap();
    let bodies: Vec<String> = owner
        .core
        .get_messages(&channel, 0, 10)
        .unwrap()
        .messages
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert_eq!(bodies, ["שלום", "hi"]);
    let page = owner.core.get_messages(&channel, 1, 99).unwrap();
    assert_eq!((page.start, page.messages.len(), page.messages[0].index), (1, 1, 1));

    // App lock: nothing opens; after unlock the stored rotations are
    // replayed and the old messages open again.
    owner.core.lock_keys();
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 0);
    owner.core.unlock_keys();
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 2);

    // A valid rotation and message for a channel authority doesn't know:
    // stored, never handed to the cipher, never shown.
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let key = identity::RecoveryPhrase::parse(OWNER_WORDS).unwrap().identity();
    let control: Vec<_> = world
        .network
        .fetch(server_key)
        .iter()
        .filter(|r| r.scope() == Scope::Control)
        .map(|r| r.id())
        .collect();
    let unknown = records::ChannelId::from_bytes([0xee; 32]);
    // A real rotation part's body, so the record itself is valid.
    let part = world
        .network
        .fetch(server_key)
        .into_iter()
        .find(|r| r.kind() == Kind::KeyRotation)
        .expect("the owner rotated")
        .body()
        .to_vec();
    for (seq, kind, body) in [(0, Kind::KeyRotation, part), (1, Kind::Message, vec![1; 40])] {
        let record = Draft {
            server: server_key,
            scope: Scope::Channel(unknown),
            seq,
            deps: vec![],
            auth_deps: vec![*control.iter().max().unwrap()],
            ts: 1,
            kind,
            body,
        }
        .sign(key.signing_key())
        .unwrap_or_else(|e| panic!("{kind:?}: {e:?}"));
        world.network.publish(&record);
    }
    owner.core.sync_now().unwrap();
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 2);
    assert!(
        !spy.channels.lock().unwrap().contains(&unknown),
        "an unknown channel reached the cipher"
    );
    assert!(spy.channels.lock().unwrap().contains(&records::ChannelId::from_bytes(
        hex::decode(&channel).unwrap().try_into().unwrap()
    )));
}

const OWNER_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// The real cipher, recording every channel it is asked about.
struct Spy {
    inner: api::crypto::GroupKeysCipher,
    channels: std::sync::Mutex<BTreeSet<records::ChannelId>>,
    /// When set, `rotate` fails (a re-key that can't happen).
    fail_rotate: std::sync::atomic::AtomicBool,
}

impl Spy {
    fn new() -> Spy {
        Spy {
            inner: api::crypto::GroupKeysCipher::locked(),
            channels: Default::default(),
            fail_rotate: Default::default(),
        }
    }

    fn saw(&self, channel: records::ChannelId) {
        self.channels.lock().unwrap().insert(channel);
    }
}

impl GroupCipher for Spy {
    fn membership_changed(&self, server: ServerId, members: &BTreeSet<PublicKey>) -> CipherOutcome {
        self.inner.membership_changed(server, members)
    }
    fn encrypt(
        &self,
        s: ServerId,
        c: records::ChannelId,
        p: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        self.saw(c);
        self.inner.encrypt(s, c, p)
    }
    fn decrypt(
        &self,
        s: ServerId,
        c: records::ChannelId,
        a: PublicKey,
        b: &[u8],
    ) -> Result<Opened, CipherError> {
        self.saw(c);
        self.inner.decrypt(s, c, a, b)
    }
    fn apply_rotation(&self, r: &records::Record) -> Result<Vec<records::RecordId>, CipherError> {
        if let Scope::Channel(c) = r.scope() {
            self.saw(c);
        }
        self.inner.apply_rotation(r)
    }
    fn set_accepted(&self, s: ServerId, ids: &BTreeSet<records::RecordId>) {
        self.inner.set_accepted(s, ids)
    }
    fn set_channels(&self, s: ServerId, channels: &BTreeSet<records::ChannelId>) {
        channels.iter().for_each(|c| self.saw(*c));
        self.inner.set_channels(s, channels)
    }
    fn unlock(&self, secret: Arc<groupkeys::MemberSecret>) {
        self.inner.unlock(secret)
    }
    fn lock(&self) {
        self.inner.lock()
    }
    fn rotate(
        &self,
        s: ServerId,
        c: records::ChannelId,
        m: &BTreeSet<PublicKey>,
    ) -> Result<Vec<Vec<u8>>, CipherError> {
        self.saw(c);
        if self.fail_rotate.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CipherError::Rejected);
        }
        self.inner.rotate(s, c, m)
    }
}

fn notices(peer: &Peer) -> Vec<NoticeKind> {
    peer.events
        .take()
        .into_iter()
        .filter_map(|e| match e {
            Event::Notice { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

/// M2: a kick re-keys the channel, so the kicked member can't read what
/// comes next and the others can still write. A kick nobody re-keyed for
/// is re-keyed by the next moderator who reads it; a plain member is told
/// it waits for one.
#[test]
fn a_kick_re_keys_the_channel() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    let open = |words: &str, name: &str, cipher: Arc<GroupKeysCipher>| {
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let owner = open(OWNER_WORDS, "Owner", Arc::new(GroupKeysCipher::locked()));
    let guest = open(LEGAL, "Guest", Arc::new(GroupKeysCipher::locked()));
    let carol_cipher = Arc::new(GroupKeysCipher::locked());
    let carol = open(
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        "Carol",
        carol_cipher.clone(),
    );
    let dan = open(
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        "Dan",
        Arc::new(GroupKeysCipher::locked()),
    );
    // A moderator whose build has no group encryption: its kick re-keys nothing.
    let moderator = world.peer();
    let moderator_id = sign_up(&moderator, "Mod").id;

    let server = owner.core.create_server("Kicks", AgeRestriction::None).unwrap();
    for peer in [&guest, &carol, &dan, &moderator] {
        peer.core.join_server(&server.invite_link).unwrap();
    }
    owner.core.sync_now().unwrap();
    owner.core.grant_role(&server.id, &moderator_id, Role::Moderator).unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    owner.core.send_message(&channel, "before", None).unwrap();
    for peer in [&guest, &carol, &dan] {
        peer.core.sync_now().unwrap();
        assert_eq!(peer.core.get_message_count(&channel).unwrap(), 1);
    }

    // Our own kick: a fresh key at once.
    let carol_id = carol.core.get_identity().unwrap().unwrap().id;
    owner.events.take();
    owner.core.kick_member(&server.id, &carol_id).unwrap();
    assert_eq!(notices(&owner), [NoticeKind::EncryptionRenewed]);
    let after = owner.core.send_message(&channel, "after", None).unwrap();

    guest.core.sync_now().unwrap();
    assert_eq!(guest.core.get_message_count(&channel).unwrap(), 2);
    guest.core.send_message(&channel, "still here", None).unwrap();
    owner.core.sync_now().unwrap();
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 3);

    // Carol holds every record but nothing after her kick opens for her.
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let held = world.network.fetch(server_key);
    for record in held.iter().filter(|r| r.kind() == Kind::KeyRotation) {
        let _ = carol_cipher.apply_rotation(record);
    }
    let rotations = held.iter().filter(|r| r.kind() == Kind::KeyRotation).map(|r| r.id()).collect();
    carol_cipher.set_accepted(server_key, &rotations);
    let after_record =
        held.iter().find(|r| hex::encode(r.id().as_bytes()) == after.id).expect("published");
    let Scope::Channel(ch) = after_record.scope() else { panic!("a channel record") };
    assert!(carol_cipher
        .decrypt(server_key, ch, after_record.author(), after_record.body())
        .is_err());
    carol.core.sync_now().unwrap();
    assert!(carol.core.list_servers().unwrap().is_empty());

    // A kick nobody re-keyed for (the moderator's build can't).
    let dan_id = dan.core.get_identity().unwrap().unwrap().id;
    moderator.core.sync_now().unwrap();
    moderator.core.kick_member(&server.id, &dan_id).unwrap();
    guest.events.take();
    guest.core.sync_now().unwrap();
    assert_eq!(notices(&guest), [NoticeKind::RekeyWaiting]);
    assert_eq!(
        guest.core.send_message(&channel, "anyone?", None).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    // The owner reads the kick and re-keys; then the guest can write again.
    owner.events.take();
    owner.core.sync_now().unwrap();
    assert_eq!(notices(&owner), [NoticeKind::EncryptionRenewed]);
    guest.core.sync_now().unwrap();
    guest.core.send_message(&channel, "back", None).unwrap();
    owner.core.sync_now().unwrap();
    let bodies: Vec<String> = owner
        .core
        .get_messages(&channel, 0, 10)
        .unwrap()
        .messages
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert_eq!(bodies, ["before", "after", "still here", "back"]);
}

/// Edits (author only), deletes (author, or a higher rank) as tombstones that
/// keep their place, and retry (same record, mine only), all judged by
/// authority before anything is stored or sent.
#[test]
fn edit_delete_and_retry_messages() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    let open = |words: &str, name: &str| {
        let cipher = Arc::new(GroupKeysCipher::locked());
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let owner = open(OWNER_WORDS, "Owner");
    let guest = open(LEGAL, "Guest");
    let server = owner.core.create_server("Edits", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    let one = owner.core.send_message(&channel, "one", None).unwrap();
    guest.core.sync_now().unwrap();
    let two = guest.core.send_message(&channel, "two", None).unwrap();
    let sent = world
        .network
        .fetch(ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap()))
        .len();

    // The author edits; everyone sees the new text and when.
    guest.events.take();
    guest.core.edit_message(&two.id, "two, edited").unwrap();
    assert_eq!(names(&guest.events.take()), ["message.updated"]);
    owner.core.sync_now().unwrap();
    let got = owner.core.get_messages(&channel, 0, 10).unwrap().messages;
    assert_eq!(got[1].body, "two, edited");
    assert!(got[1].edited_at.is_some() && got[1].deleted.is_none());

    // Refused before anything is stored or sent: someone else's message, a
    // higher rank's message, bad input, an unknown ID.
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let before = world.network.fetch(server_key).len();
    assert_eq!(before, sent + 1);
    assert_eq!(
        guest.core.edit_message(&one.id, "mine now").unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(guest.core.delete_message(&one.id).unwrap_err().code, ErrorCode::NotAllowed);
    assert_eq!(guest.core.edit_message(&two.id, " ").unwrap_err().code, ErrorCode::BadRequest);
    assert_eq!(guest.core.delete_message(&"ab".repeat(32)).unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(world.network.fetch(server_key).len(), before);

    // The owner outranks the guest: deleting leaves a tombstone in place.
    owner.core.delete_message(&two.id).unwrap();
    guest.core.sync_now().unwrap();
    assert_eq!(guest.core.get_message_count(&channel).unwrap(), 2);
    let got = guest.core.get_messages(&channel, 0, 10).unwrap().messages;
    assert_eq!((got[1].index, got[1].body.as_str(), got[1].deleted), (1, "", Some(true)));
    assert_eq!(got[0].body, "one");

    // Retry: my own message, the same record again; nobody else's; and
    // not a deleted one, pruned to its stub (B6-p), with nothing to send.
    assert_eq!(guest.core.retry_message(&two.id).unwrap_err().code, ErrorCode::NotAllowed);
    let three = guest.core.send_message(&channel, "three", None).unwrap();
    let held = world.network.fetch(server_key).len();
    guest.core.retry_message(&three.id).unwrap();
    assert_eq!(world.network.fetch(server_key).len(), held, "the same record, nothing new signed");
    assert_eq!(guest.core.retry_message(&one.id).unwrap_err().code, ErrorCode::NotAllowed);
    assert_eq!(guest.core.retry_message("nope").unwrap_err().code, ErrorCode::NotFound);
}

/// A re-key that fails after a kick fails closed: the kick still stands
/// (Ok), no `encryption-renewed` is claimed, sending stays blocked, and
/// sync carries on; the next send after the cause is gone re-keys.
#[test]
fn a_failed_re_key_fails_closed() {
    use api::crypto::GroupKeysCipher;
    use std::sync::atomic::Ordering;

    let world = World::new();
    let open = |words: &str, name: &str, cipher: Arc<dyn GroupCipher>| {
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let spy = Arc::new(Spy::new());
    let owner = open(OWNER_WORDS, "Owner", spy.clone());
    let guest = open(LEGAL, "Guest", Arc::new(GroupKeysCipher::locked()));
    let carol = open(
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        "Carol",
        Arc::new(GroupKeysCipher::locked()),
    );
    let server = owner.core.create_server("Fails", AgeRestriction::None).unwrap();
    for peer in [&guest, &carol] {
        peer.core.join_server(&server.invite_link).unwrap();
    }
    owner.core.sync_now().unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    owner.core.send_message(&channel, "before", None).unwrap();

    spy.fail_rotate.store(true, Ordering::SeqCst);
    let carol_id = carol.core.get_identity().unwrap().unwrap().id;
    owner.events.take();
    owner.core.kick_member(&server.id, &carol_id).unwrap();
    let after_kick = notices(&owner);
    assert_eq!(after_kick, [NoticeKind::RekeyWaiting]);
    assert!(!after_kick.contains(&NoticeKind::EncryptionRenewed));
    assert_eq!(
        owner.core.send_message(&channel, "too soon", None).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    // Sync doesn't fail either.
    guest.core.sync_now().unwrap();
    owner.core.sync_now().unwrap();

    // The cause gone, the next send re-keys and the guest reads it.
    spy.fail_rotate.store(false, Ordering::SeqCst);
    owner.core.send_message(&channel, "after", None).unwrap();
    guest.core.sync_now().unwrap();
    let bodies: Vec<String> = guest
        .core
        .get_messages(&channel, 0, 10)
        .unwrap()
        .messages
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert_eq!(bodies, ["before", "after"]);
}

/// Sync announces what is new: `message.added` once per message,
/// `message.updated` when an edit or delete changes it; history already
/// there at the first sync isn't announced again.
#[test]
fn sync_announces_new_and_changed_messages_once() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    let open = |words: &str, name: &str| {
        let cipher = Arc::new(GroupKeysCipher::locked());
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let owner = open(OWNER_WORDS, "Owner");
    let guest = open(LEGAL, "Guest");
    let server = owner.core.create_server("News", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    let first = owner.core.send_message(&channel, "first", None).unwrap();
    guest.core.sync_now().unwrap();
    guest.events.take();

    let second = owner.core.send_message(&channel, "second", None).unwrap();
    guest.core.sync_now().unwrap();
    let added = |events: Vec<Event>| -> Vec<(String, String)> {
        events
            .into_iter()
            .filter_map(|e| match e {
                Event::MessageAdded(m) => Some(("added".into(), m.id)),
                Event::MessageUpdated(m) => Some(("updated".into(), m.id)),
                _ => None,
            })
            .collect()
    };
    assert_eq!(added(guest.events.take()), [("added".into(), second.id.clone())]);
    guest.core.sync_now().unwrap();
    assert!(added(guest.events.take()).is_empty(), "nothing new, nothing announced");

    owner.core.edit_message(&first.id, "first!").unwrap();
    guest.core.sync_now().unwrap();
    assert_eq!(added(guest.events.take()), [("updated".into(), first.id.clone())]);

    // The owner's own send was announced by the command, not again by sync.
    owner.events.take();
    owner.core.sync_now().unwrap();
    assert!(added(owner.events.take()).is_empty());
}

/// Channels are authority's (`State.channels`, #113): #general from
/// `sign_new_server`, one the owner creates, and not one a member tries to.
#[test]
fn channels_come_from_channel_create_records() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    let open = |words: &str, name: &str| {
        let cipher = Arc::new(GroupKeysCipher::locked());
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let owner = open(OWNER_WORDS, "Owner");
    let guest = open(LEGAL, "Guest");
    let server = owner.core.create_server("Rooms", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    let names = |peer: &Peer| -> Vec<String> {
        peer.core.list_channels(&server.id).unwrap().into_iter().map(|c| c.name).collect()
    };
    assert_eq!(names(&owner), ["general"]);

    // A ChannelCreate each: the owner's is accepted, the guest's isn't.
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let heads = |w: &World| -> Vec<records::RecordId> {
        let mut ids: Vec<_> = w
            .network
            .fetch(server_key)
            .iter()
            .filter(|r| r.scope() == Scope::Control)
            .map(|r| r.id())
            .collect();
        ids.sort();
        ids
    };
    let create = |words: &str, seq: u64, id: u8, name: &str| {
        let key = identity::RecoveryPhrase::parse(words).unwrap().identity();
        let action = authority::Action::ChannelCreate {
            channel: records::ChannelId::from_bytes([id; 32]),
            adult: false,
            name: name.into(),
        };
        let mut deps = heads(&world);
        deps.truncate(records::MAX_DEPS);
        let record =
            authority::sign_control(key.signing_key(), server_key, seq, deps, 5, &action).unwrap();
        world.network.publish(&record);
    };
    create(OWNER_WORDS, 10, 0x51, "random");
    create(LEGAL, 10, 0x52, "guests-only");
    for peer in [&owner, &guest] {
        peer.core.sync_now().unwrap();
    }
    let mut got = names(&guest);
    got.sort();
    assert_eq!(got, ["general", "random"]);

    // The new channel works end to end; the refused one isn't a channel.
    let random = hex::encode([0x51; 32]);
    owner.core.send_message(&random, "in random", None).unwrap();
    guest.core.sync_now().unwrap();
    assert_eq!(guest.core.get_messages(&random, 0, 9).unwrap().messages[0].body, "in random");
    assert_eq!(
        guest.core.get_message_count(&hex::encode([0x52; 32])).unwrap_err().code,
        ErrorCode::NotFound
    );
}

/// #general is named in the creator's language when created, and keeps that
/// name for everyone after, whatever their own language.
#[test]
fn general_is_named_in_the_creators_language() {
    let world = World::new();
    let make = |locale: Locale| {
        let peer = world.peer();
        let input =
            NewIdentityInput { locale, country: "IL".into(), birth_date: "1990-05-01".into() };
        let words = peer.core.create_identity(input).unwrap().words;
        let positions = peer.core.get_recovery_challenge().unwrap().positions;
        let answers: BTreeMap<usize, String> =
            positions.iter().map(|p| (*p, words[p - 1].clone())).collect();
        assert!(peer.core.check_recovery_words(answers).unwrap().ok);
        let profile = ProfileInput { display_name: "X".into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let hebrew = make(Locale::He);
    let english = make(Locale::En);
    let he_server = hebrew.core.create_server("א", AgeRestriction::None).unwrap();
    let en_server = english.core.create_server("A", AgeRestriction::None).unwrap();
    assert_eq!(hebrew.core.list_channels(&he_server.id).unwrap()[0].name, "כללי");
    assert_eq!(english.core.list_channels(&en_server.id).unwrap()[0].name, "general");
    // The English speaker joins the Hebrew server: the stored name stays.
    english.core.join_server(&he_server.invite_link).unwrap();
    assert_eq!(english.core.list_channels(&he_server.id).unwrap()[0].name, "כללי");
}

/// GAP-4: each server's sync secret is in its invite link, stored sealed
/// (never in the clear in the database), and the same after a restart.
#[test]
fn the_server_secret_is_sealed_and_kept() {
    let world = World::new();
    let secrets = Arc::new(MemoryStore::new());
    let dir = std::env::temp_dir().join(format!("mvs-api-secret-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("core.db");
    let _ = std::fs::remove_file(&db);
    let peer = world.peer_with(secrets.clone(), Some(db.clone()), Arc::new(PendingGroupKeys));
    sign_up(&peer, "Owner");
    let server = peer.core.create_server("Secret", AgeRestriction::None).unwrap();
    let parts: Vec<&str> =
        server.invite_link.trim_start_matches("mvs://join/").split('/').collect();
    assert_eq!(parts.len(), 3, "server, invite, secret");
    let secret = hex::decode(parts[2]).unwrap();
    assert_eq!(secret.len(), 32);
    drop(peer);

    let bytes = std::fs::read(&db).unwrap();
    assert!(!contains(&bytes, &secret), "the secret is in the db in the clear");
    assert!(!contains(&bytes, parts[2].as_bytes()), "the secret's hex is in the db");

    let peer = world.peer_with(secrets, Some(db), Arc::new(PendingGroupKeys));
    let again = peer.core.list_servers().unwrap().remove(0);
    assert_eq!(again.invite_link, server.invite_link);
    drop(peer);
    let _ = std::fs::remove_dir_all(&dir);
}

/// B6-p (the product lead's acceptance): after a delete or an edit, the
/// databases hold no ciphertext and no plaintext of the original or older
/// versions; what's shown is unchanged (edited text, tombstone).
#[test]
fn deletes_and_edits_prune_old_bodies_from_the_db() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    let dir = std::env::temp_dir().join(format!("mvs-api-b6p-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let open = |words: &str, name: &str| {
        let db = dir.join(format!("{name}.db"));
        let cipher = Arc::new(GroupKeysCipher::locked());
        let peer = world.peer_with(Arc::new(MemoryStore::new()), Some(db.clone()), cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        (peer, db)
    };
    let (owner, owner_db) = open(OWNER_WORDS, "owner");
    let (guest, guest_db) = open(LEGAL, "guest");
    let server = owner.core.create_server("Prune", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let body_of = |id: &str| -> Vec<u8> {
        world
            .network
            .fetch(server_key)
            .into_iter()
            .find(|r| hex::encode(r.id().as_bytes()) == id)
            .expect("published")
            .body()
            .to_vec()
    };
    // Bodies to find later: the original and the first edit of `edited`,
    // the original of `gone`.
    let edited = owner.core.send_message(&channel, "PLAIN-original-edited", None).unwrap();
    let gone = owner.core.send_message(&channel, "PLAIN-original-gone", None).unwrap();
    owner.core.edit_message(&edited.id, "PLAIN-first-edit").unwrap();
    let first_edit = world
        .network
        .fetch(server_key)
        .into_iter()
        .filter(|r| r.kind() == Kind::MessageEdit)
        .map(|r| r.body().to_vec())
        .next()
        .unwrap();
    let mut old_bodies = vec![body_of(&edited.id), body_of(&gone.id), first_edit];
    owner.core.edit_message(&edited.id, "latest edit").unwrap();
    owner.core.delete_message(&gone.id).unwrap();
    for body in &mut old_bodies {
        body.truncate(64);
    }
    guest.core.sync_now().unwrap();
    owner.core.sync_now().unwrap();

    // Shown: the latest edit, and a tombstone in place.
    for peer in [&owner, &guest] {
        let got = peer.core.get_messages(&channel, 0, 10).unwrap().messages;
        assert_eq!(got.len(), 2);
        let by_id = |id: &str| got.iter().find(|m| m.id == id).unwrap().clone();
        assert_eq!(by_id(&edited.id).body, "latest edit");
        assert!(by_id(&edited.id).edited_at.is_some());
        assert_eq!((by_id(&gone.id).body.as_str(), by_id(&gone.id).deleted), ("", Some(true)));
    }
    drop((owner, guest));

    for path in [&owner_db, &guest_db] {
        // The db and its WAL and rollback journal, if any.
        let mut bytes = std::fs::read(path).unwrap();
        for suffix in ["-wal", "-journal"] {
            let mut side = path.as_os_str().to_owned();
            side.push(suffix);
            bytes.extend(std::fs::read(side).unwrap_or_default());
        }
        let db = path;
        for body in &old_bodies {
            assert!(
                !contains(&bytes, body),
                "{}: an old ciphertext is still in the db",
                db.display()
            );
        }
        for text in ["PLAIN-original-edited", "PLAIN-original-gone", "PLAIN-first-edit"] {
            assert!(!contains(&bytes, text.as_bytes()), "{}: {text} is in the db", db.display());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// #110, #116, #151: the owner's removed list and undoKick; 18+ channels
/// only for members whose age claim says 18+; names with bidi or zero-width
/// characters refused.
#[test]
fn removed_members_age_claims_and_clean_names() {
    let world = World::new();
    let born = |peer: &Peer, birth: &str, name: &str| -> String {
        let input =
            NewIdentityInput { locale: Locale::En, country: "IL".into(), birth_date: birth.into() };
        let words = peer.core.create_identity(input).unwrap().words;
        let positions = peer.core.get_recovery_challenge().unwrap().positions;
        let answers: BTreeMap<usize, String> =
            positions.iter().map(|p| (*p, words[p - 1].clone())).collect();
        assert!(peer.core.check_recovery_words(answers).unwrap().ok);
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap().id
    };
    let owner = world.peer();
    owner.core.restore_identity(OWNER_WORDS.split(' ').map(String::from).collect()).unwrap();
    let profile = ProfileInput { display_name: "Owner".into(), bio: None, avatar_data_url: None };
    owner.core.update_profile(profile).unwrap();
    let adult = world.peer();
    let adult_id = born(&adult, "1990-05-01", "Adult");
    let minor = world.peer();
    born(&minor, "2014-01-01", "Minor");

    // B23: names others see are clean.
    let bidi = ProfileInput { display_name: "a\u{202E}b".into(), bio: None, avatar_data_url: None };
    assert_eq!(adult.core.update_profile(bidi).unwrap_err().code, ErrorCode::BadRequest);
    assert_eq!(
        owner.core.create_server("x\u{200B}y", AgeRestriction::None).unwrap_err().code,
        ErrorCode::BadRequest
    );

    let server = owner.core.create_server("Ages", AgeRestriction::None).unwrap();
    for peer in [&adult, &minor] {
        peer.core.join_server(&server.invite_link).unwrap();
    }
    // An 18+ channel by the owner.
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let mut deps: Vec<_> = world
        .network
        .fetch(server_key)
        .iter()
        .filter(|r| r.scope() == Scope::Control)
        .map(|r| r.id())
        .collect();
    deps.sort();
    let key = identity::RecoveryPhrase::parse(OWNER_WORDS).unwrap().identity();
    let night = records::ChannelId::from_bytes([0x18; 32]);
    let create =
        authority::Action::ChannelCreate { channel: night, adult: true, name: "night".into() };
    let record =
        authority::sign_control(key.signing_key(), server_key, 20, deps, 9, &create).unwrap();
    world.network.publish(&record);
    for peer in [&owner, &adult, &minor] {
        peer.core.sync_now().unwrap();
    }
    let names = |peer: &Peer| -> Vec<String> {
        let mut n: Vec<String> =
            peer.core.list_channels(&server.id).unwrap().into_iter().map(|c| c.name).collect();
        n.sort();
        n
    };
    assert_eq!(names(&adult), ["general", "night"]);
    assert_eq!(names(&minor), ["general"]);
    assert_eq!(
        minor.core.get_message_count(&hex::encode([0x18; 32])).unwrap_err().code,
        ErrorCode::NotFound
    );

    // Kick, the owner's removed list, undo, join again.
    owner.core.kick_member(&server.id, &adult_id).unwrap();
    let removed = owner.core.list_removed_members(&server.id).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(
        (removed[0].id.as_str(), removed[0].display_name.as_str()),
        (adult_id.as_str(), "Adult")
    );
    assert!(removed[0].removed_at > 0);
    assert_eq!(
        minor.core.list_removed_members(&server.id).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(
        minor.core.undo_kick(&server.id, &adult_id).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    adult.core.sync_now().unwrap();
    assert!(adult.core.list_servers().unwrap().is_empty());
    owner.core.undo_kick(&server.id, &adult_id).unwrap();
    assert!(owner.core.list_removed_members(&server.id).unwrap().is_empty());
    adult.core.join_server(&server.invite_link).unwrap();
    assert_eq!(adult.core.list_servers().unwrap().len(), 1);
}

/// A network that never reaches anyone: what it publishes stays `pending`,
/// or `received` once told so. For send states across a restart.
#[derive(Default)]
struct OfflineNet {
    states: std::sync::Mutex<BTreeMap<records::RecordId, SendState>>,
    received: std::sync::atomic::AtomicBool,
}

impl Network for OfflineNet {
    fn publish(&self, record: &records::Record) {
        self.states.lock().unwrap().entry(record.id()).or_insert(SendState::Pending);
    }

    fn fetch(&self, _: ServerId) -> Vec<records::Record> {
        Vec::new()
    }

    fn send_state(&self, id: records::RecordId) -> Option<SendState> {
        let state = *self.states.lock().unwrap().get(&id)?;
        Some(if self.received.load(std::sync::atomic::Ordering::SeqCst) {
            SendState::Received
        } else {
            state
        })
    }
}

/// The world as a joiner sees it: nothing, until the inviter comes online.
struct LateNet {
    world: Arc<LoopbackNetwork>,
    online: std::sync::atomic::AtomicBool,
}

impl Network for LateNet {
    fn publish(&self, record: &records::Record) {
        self.world.publish(record);
    }

    fn fetch(&self, server: ServerId) -> Vec<records::Record> {
        if self.online.load(std::sync::atomic::Ordering::SeqCst) {
            self.world.fetch(server)
        } else {
            Vec::new()
        }
    }
}

/// v0.2.4 join hang (mitigation): a join that finds nobody online keeps
/// trying in the background, so it completes once the inviter comes
/// online, here 20 s later, without the user joining again.
#[test]
fn a_join_completes_when_the_inviter_comes_online_later() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let server = owner.core.create_server("Late", AgeRestriction::None).unwrap();
    let net = Arc::new(LateNet { world: world.network.clone(), online: Default::default() });
    let joiner =
        world.peer_on(net.clone(), Arc::new(MemoryStore::new()), None, Arc::new(PendingGroupKeys));
    sign_up(&joiner, "Joiner");
    joiner.events.take();

    let start = std::time::Instant::now();
    joiner.core.join_server(&server.invite_link).unwrap();
    assert!(matches!(
        joiner.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::NobodyOnline { .. }))
    ));
    let joined = |peer: &Peer| peer.core.list_servers().unwrap().iter().any(|s| s.id == server.id);
    let mut online = false;
    while !joined(&joiner) {
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "the join never completed");
        if !online && start.elapsed() >= std::time::Duration::from_secs(20) {
            net.online.store(true, std::sync::atomic::Ordering::SeqCst);
            online = true;
        }
        // What the background sync loop does every `SYNC_EVERY`.
        joiner.core.sync_now().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(online, "joined before the inviter came online");
    assert!(joiner
        .events
        .take()
        .iter()
        .any(|e| matches!(e, Event::JoinStatus(JoinStatus::Joined { .. }))));

    // A cancelled join stops retrying: nothing more is announced.
    let other = owner.core.create_server("Other", AgeRestriction::None).unwrap();
    net.online.store(false, std::sync::atomic::Ordering::SeqCst);
    joiner.core.join_server(&other.invite_link).unwrap();
    joiner.core.cancel_join(&other.invite_link).unwrap();
    net.online.store(true, std::sync::atomic::Ordering::SeqCst);
    joiner.events.take();
    std::thread::sleep(api::JOIN_RETRY_FIRST + std::time::Duration::from_millis(200));
    joiner.core.sync_now().unwrap();
    assert!(!joined_other(&joiner, &other.id), "a cancelled join went on");
}

fn joined_other(peer: &Peer, id: &str) -> bool {
    peer.core.list_servers().unwrap().iter().any(|s| s.id == id)
}

/// B6: a message written offline is still `pending` after a restart (the
/// state is kept in the db), and a later `received` is kept too.
#[test]
fn send_states_survive_a_restart() {
    use api::crypto::GroupKeysCipher;

    let secrets = Arc::new(MemoryStore::new());
    let dir = std::env::temp_dir().join(format!("mvs-api-sendstate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let open = |net: Arc<OfflineNet>| {
        Core::open(
            Config { db_path: Some(dir.join("core.db")), app_version: "test".into() },
            Ports {
                secrets: secrets.clone(),
                network: net,
                events: Arc::new(CollectingSink::new()),
                clock: Arc::new(SystemClock),
                cipher: Arc::new(GroupKeysCipher::locked()),
            },
        )
        .unwrap()
    };
    let core = open(Arc::new(OfflineNet::default()));
    core.restore_identity(OWNER_WORDS.split(' ').map(String::from).collect()).unwrap();
    let profile = ProfileInput { display_name: "Owner".into(), bio: None, avatar_data_url: None };
    core.update_profile(profile).unwrap();
    let server = core.create_server("Offline", AgeRestriction::None).unwrap();
    let channel = core.list_channels(&server.id).unwrap().remove(0).id;
    let sent = core.send_message(&channel, "written offline", None).unwrap();
    assert_eq!(sent.send_state, Some(SendState::Pending));
    drop(core);

    // A fresh network knows nothing of it: the db still says pending.
    let core = open(Arc::new(OfflineNet::default()));
    let got = core.get_messages(&channel, 0, 9).unwrap().messages;
    assert_eq!(got[0].send_state, Some(SendState::Pending));
    drop(core);

    // A network that has it as received: stored, and kept after a restart.
    let net = Arc::new(OfflineNet::default());
    net.states.lock().unwrap().insert(
        records::RecordId::from_bytes(hex::decode(&sent.id).unwrap().try_into().unwrap()),
        SendState::Pending,
    );
    net.received.store(true, std::sync::atomic::Ordering::SeqCst);
    let core = open(net);
    assert_eq!(
        core.get_messages(&channel, 0, 9).unwrap().messages[0].send_state,
        Some(SendState::Received)
    );
    drop(core);
    let core = open(Arc::new(OfflineNet::default()));
    assert_eq!(
        core.get_messages(&channel, 0, 9).unwrap().messages[0].send_state,
        Some(SendState::Received)
    );
    drop(core);
    let _ = std::fs::remove_dir_all(&dir);
}

/// v0.2.0 smoke: a member who joins writes first, and the owner never
/// sends. The owner's sync sees the join and keys the channel for the new
/// member set, so the joiner can write (and the owner can read it).
#[test]
fn a_joiner_can_write_first_once_the_owner_syncs() {
    use api::crypto::GroupKeysCipher;

    let world = World::new();
    let open = |words: &str, name: &str| {
        let cipher = Arc::new(GroupKeysCipher::locked());
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let owner = open(OWNER_WORDS, "Owner");
    let guest = open(LEGAL, "Guest");
    let server = owner.core.create_server("Smoke", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    guest.core.sync_now().unwrap();
    let channel = guest.core.list_channels(&server.id).unwrap().remove(0).id;
    guest.core.send_message(&channel, "hello from B", None).unwrap();
    owner.core.sync_now().unwrap();
    let got = owner.core.get_messages(&channel, 0, 9).unwrap().messages;
    assert_eq!(got.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["hello from B"]);

    // A second joiner after a key exists gets it too.
    let third = open(
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        "Third",
    );
    third.core.join_server(&server.invite_link).unwrap();
    // Join re-keys are batched (GAP-8): past the window, the next round keys C.
    *world.clock.0.lock().unwrap() += 31_000;
    owner.core.sync_now().unwrap();
    third.core.sync_now().unwrap();
    third.core.send_message(&channel, "hi from C", None).unwrap();
    guest.core.sync_now().unwrap();
    assert_eq!(guest.core.get_message_count(&channel).unwrap(), 2);
}

/// The join re-key fails closed too: a failed rotation gives
/// `rekey-waiting`, never `encryption-renewed`, and the joiner can't write.
#[test]
fn a_failed_join_re_key_fails_closed() {
    use api::crypto::GroupKeysCipher;
    use std::sync::atomic::Ordering;

    let world = World::new();
    let open = |words: &str, name: &str, cipher: Arc<dyn GroupCipher>| {
        let peer = world.peer_with(Arc::new(MemoryStore::new()), None, cipher);
        peer.core.restore_identity(words.split(' ').map(String::from).collect()).unwrap();
        let profile = ProfileInput { display_name: name.into(), bio: None, avatar_data_url: None };
        peer.core.update_profile(profile).unwrap();
        peer
    };
    let spy = Arc::new(Spy::new());
    let owner = open(OWNER_WORDS, "Owner", spy.clone());
    let guest = open(LEGAL, "Guest", Arc::new(GroupKeysCipher::locked()));
    let server = owner.core.create_server("Closed", AgeRestriction::None).unwrap();
    spy.fail_rotate.store(true, Ordering::SeqCst);
    guest.core.join_server(&server.invite_link).unwrap();
    owner.events.take();
    owner.core.sync_now().unwrap();
    let got = notices(&owner);
    assert!(
        got.contains(&NoticeKind::RekeyWaiting) && !got.contains(&NoticeKind::EncryptionRenewed)
    );
    guest.core.sync_now().unwrap();
    let channel = guest.core.list_channels(&server.id).unwrap().remove(0).id;
    assert_eq!(
        guest.core.send_message(&channel, "x", None).unwrap_err().code,
        ErrorCode::NotAllowed
    );
}

/// A server, its owner (real cipher), and a counter of rotations (one
/// rotation's parts share their deps).
fn keyed_server(world: &World, name: &str) -> (Peer, ServerSummary, String) {
    use api::crypto::GroupKeysCipher;
    let owner =
        world.peer_with(Arc::new(MemoryStore::new()), None, Arc::new(GroupKeysCipher::locked()));
    owner.core.restore_identity(OWNER_WORDS.split(' ').map(String::from).collect()).unwrap();
    let profile = ProfileInput { display_name: "Owner".into(), bio: None, avatar_data_url: None };
    owner.core.update_profile(profile).unwrap();
    let server = owner.core.create_server(name, AgeRestriction::None).unwrap();
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    (owner, server, channel)
}

fn rotations(world: &World, server: &ServerSummary) -> usize {
    let key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let mut seen = BTreeSet::new();
    for r in world.network.fetch(key).iter().filter(|r| r.kind() == Kind::KeyRotation) {
        seen.insert(r.deps().to_vec());
    }
    seen.len()
}

/// A member with the real cipher who joined `server` (the owner hasn't
/// synced yet).
fn joiner(world: &World, server: &ServerSummary, name: &str) -> Peer {
    use api::crypto::GroupKeysCipher;
    let peer =
        world.peer_with(Arc::new(MemoryStore::new()), None, Arc::new(GroupKeysCipher::locked()));
    sign_up(&peer, name);
    peer.core.join_server(&server.invite_link).unwrap();
    peer
}

fn advance(world: &World, ms: u64) {
    *world.clock.0.lock().unwrap() += ms;
}

/// The security lead's token bucket for join re-keys (GAP-8): a burst of 3
/// re-keys at once; a 4th waits for the next token (due 15 s after the
/// first was spent) and shares that re-key with everyone who joined
/// meanwhile. The new key only: a joiner can't open what came before.
#[test]
fn join_re_keys_spend_a_token_bucket() {
    let world = World::new();
    let (owner, server, channel) = keyed_server(&world, "Bucket");
    owner.core.send_message(&channel, "before anyone joined", None).unwrap();

    // A burst of 3: each re-keyed at once, each can write at once.
    let mut first = Vec::new();
    for i in 0..3 {
        let before = rotations(&world, &server);
        let peer = joiner(&world, &server, &format!("J{i}"));
        advance(&world, 100);
        owner.core.sync_now().unwrap();
        assert_eq!(rotations(&world, &server), before + 1, "join {i} waited");
        peer.core.sync_now().unwrap();
        peer.core.send_message(&channel, &format!("J{i} here"), None).unwrap();
        first.push(peer);
    }
    assert!(owner.core.next_rekey_in().is_none());

    // The 4th and 5th: the bucket is empty, so they wait, together.
    let after_burst = rotations(&world, &server);
    let fourth = joiner(&world, &server, "J3");
    let fifth = joiner(&world, &server, "J4");
    owner.core.sync_now().unwrap();
    assert_eq!(rotations(&world, &server), after_burst, "a 4th re-key without a token");
    for peer in [&fourth, &fifth] {
        peer.core.sync_now().unwrap();
        assert_eq!(
            peer.core.send_message(&channel, "too soon", None).unwrap_err().code,
            ErrorCode::NotAllowed
        );
    }
    // Due one refill after the first token was spent (a few hundred ms of
    // test clock ago); the loop wakes exactly then.
    let due = owner.core.next_rekey_in().expect("a waiting re-key");
    let refill = std::time::Duration::from_millis(JOIN_REKEY_REFILL_MS);
    assert!(due < refill && due > refill - std::time::Duration::from_secs(1), "{due:?}");
    // The test clock moves 1 ms per read, so check within 50 ms either way.
    advance(&world, due.as_millis() as u64 - 50);
    owner.core.sync_now().unwrap();
    assert_eq!(rotations(&world, &server), after_burst, "re-keyed before the token");
    advance(&world, 100);
    owner.core.sync_now().unwrap();
    assert_eq!(rotations(&world, &server), after_burst + 1, "one shared re-key");
    assert!(owner.core.next_rekey_in().is_none());
    for peer in [&fourth, &fifth] {
        peer.core.sync_now().unwrap();
        peer.core.send_message(&channel, "now", None).unwrap();
    }
    owner.core.sync_now().unwrap();
    // 1 from before, 3 from the burst, 2 shared.
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 6);
    // Nobody who joined opens the message from before they joined.
    first[0].core.sync_now().unwrap();
    let bodies: Vec<String> = first[0]
        .core
        .get_messages(&channel, 0, 9)
        .unwrap()
        .messages
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert!(!bodies.iter().any(|b| b == "before anyone joined"), "{bodies:?}");
}

/// GAP-8 amplification: 10 joins in 5 s, with the owner syncing every
/// 500 ms, give at most 3 rotations (the burst); the next token keys
/// everyone else at once, and all 10 can write.
#[test]
fn ten_joins_in_five_seconds_make_at_most_three_rotations() {
    let world = World::new();
    let (owner, server, channel) = keyed_server(&world, "Flood");
    let joiners: Vec<Peer> = (0..10)
        .map(|i| {
            let peer = joiner(&world, &server, &format!("J{i}"));
            advance(&world, 500);
            owner.core.sync_now().unwrap();
            peer
        })
        .collect();
    let n = rotations(&world, &server);
    assert!(n <= JOIN_REKEY_BURST as usize, "{n} rotations for 10 joins in 5 s");

    advance(&world, JOIN_REKEY_REFILL_MS);
    owner.core.sync_now().unwrap();
    assert!(rotations(&world, &server) <= n + 1);
    for (i, joiner) in joiners.iter().enumerate() {
        joiner.core.sync_now().unwrap();
        joiner.core.send_message(&channel, &format!("hi {i}"), None).unwrap();
    }
    owner.core.sync_now().unwrap();
    assert_eq!(owner.core.get_message_count(&channel).unwrap(), 10);
}

/// The Tech Lead's hot-loop fix: the wait for a due re-key never drops
/// below 250 ms, and a rotator who is demoted stops waiting at once (no
/// due re-key left to spin the sync loop on).
#[test]
fn a_waiting_re_key_never_spins_and_a_demoted_rotator_stops_waiting() {
    let world = World::new();
    let (owner, server, _) = keyed_server(&world, "Spin");
    let moderator = joiner(&world, &server, "Mod");
    let mod_id = moderator.core.get_identity().unwrap().unwrap().id;
    owner.core.sync_now().unwrap();
    owner.core.grant_role(&server.id, &mod_id, Role::Moderator).unwrap();
    moderator.core.sync_now().unwrap();

    // Only the moderator syncs: it spends its 3 tokens, then waits.
    for i in 0..4 {
        joiner(&world, &server, &format!("J{i}"));
        moderator.core.sync_now().unwrap();
    }
    assert!(moderator.core.next_rekey_in().is_some(), "the moderator isn't waiting");

    // Past due, and the re-key hasn't happened yet: still at least 250 ms.
    advance(&world, 2 * JOIN_REKEY_REFILL_MS);
    assert_eq!(moderator.core.next_rekey_in(), Some(REKEY_WAKE_FLOOR));

    // Demoted: the next round clears the wait.
    owner.core.revoke_role(&server.id, &mod_id).unwrap();
    moderator.core.sync_now().unwrap();
    assert_eq!(moderator.core.next_rekey_in(), None, "a demoted rotator still waits");
    moderator.core.sync_now().unwrap();
    assert_eq!(moderator.core.next_rekey_in(), None);
}

/// A kick never waits for a token: with the bucket empty, it re-keys at
/// once, and that re-key covers a joiner who was waiting too.
#[test]
fn a_kick_with_an_empty_bucket_re_keys_at_once() {
    let world = World::new();
    let (owner, server, channel) = keyed_server(&world, "Kick");
    let mut ids = Vec::new();
    for i in 0..3 {
        let peer = joiner(&world, &server, &format!("J{i}"));
        owner.core.sync_now().unwrap();
        ids.push(peer.core.get_identity().unwrap().unwrap().id);
    }
    let waiting = joiner(&world, &server, "J3");
    owner.core.sync_now().unwrap();
    assert!(owner.core.next_rekey_in().is_some(), "the bucket isn't empty");
    let before = rotations(&world, &server);
    owner.core.kick_member(&server.id, &ids[0]).unwrap();
    assert_eq!(rotations(&world, &server), before + 1, "the kick waited for a token");
    assert!(owner.core.next_rekey_in().is_none());
    waiting.core.sync_now().unwrap();
    waiting.core.send_message(&channel, "covered by the kick's re-key", None).unwrap();
}

/// B11: the owner's channel commands, with names normalised and checked,
/// #general (`isDefault`) undeletable, and the channel events.
#[test]
fn channel_commands() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let guest = world.peer();
    sign_up(&guest, "Guest");
    let server = owner.core.create_server("Rooms", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    let general = owner.core.list_channels(&server.id).unwrap().remove(0);
    assert!(general.is_default);

    owner.events.take();
    let room = owner.core.create_channel(&server.id, "  my   big room ", false).unwrap();
    assert_eq!((room.name.as_str(), room.is_default, room.adult), ("my-big-room", false, false));
    assert!(names(&owner.events.take()).contains(&"channel.added"));
    assert_eq!(
        owner.core.create_channel(&server.id, "my big room", false).unwrap_err().code,
        ErrorCode::NameTaken
    );
    assert_eq!(
        owner.core.create_channel(&server.id, "a\u{202E}b", false).unwrap_err().code,
        ErrorCode::BadRequest
    );
    guest.core.sync_now().unwrap();
    assert_eq!(
        guest.core.create_channel(&server.id, "mine", false).unwrap_err().code,
        ErrorCode::NotAllowed
    );

    let renamed = owner.core.update_channel(&room.id, Some("lounge"), None).unwrap();
    assert_eq!((renamed.name.as_str(), renamed.id.as_str()), ("lounge", room.id.as_str()));
    assert!(names(&owner.events.take()).contains(&"channel.updated"));
    assert_eq!(
        owner.core.update_channel(&room.id, Some(&general.name), None).unwrap_err().code,
        ErrorCode::NameTaken
    );

    assert_eq!(owner.core.delete_channel(&general.id).unwrap_err().code, ErrorCode::NotAllowed);
    owner.core.delete_channel(&room.id).unwrap();
    let removed = owner.events.take().into_iter().find_map(|e| match e {
        Event::ChannelRemoved { channel_id, name, .. } => Some((channel_id, name)),
        _ => None,
    });
    assert_eq!(removed, Some((room.id.clone(), "lounge".to_owned())));
    let left: Vec<String> =
        owner.core.list_channels(&server.id).unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(left.as_slice(), std::slice::from_ref(&general.id));
    assert_eq!(owner.core.get_message_count(&room.id).unwrap_err().code, ErrorCode::NotFound);
}

/// #185 review: an owner without an 18+ claim can't make or mark an 18+
/// channel: `not-allowed`, and nothing is signed or published.
#[test]
fn an_owner_without_an_adult_claim_cannot_make_adult_channels() {
    let world = World::new();
    let owner = world.peer();
    // Restored: no birth date, so the age claim says "not 18+".
    owner.core.restore_identity(OWNER_WORDS.split(' ').map(String::from).collect()).unwrap();
    let profile = ProfileInput { display_name: "Owner".into(), bio: None, avatar_data_url: None };
    owner.core.update_profile(profile).unwrap();
    let server = owner.core.create_server("Minors", AgeRestriction::None).unwrap();
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let room = owner.core.create_channel(&server.id, "room", false).unwrap();
    let held = world.network.fetch(server_key).len();
    assert_eq!(
        owner.core.create_channel(&server.id, "night", true).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(
        owner.core.update_channel(&room.id, None, Some(true)).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(world.network.fetch(server_key).len(), held, "something was published");
    // A long name is cut to 32 before trimming its dashes.
    let long =
        owner.core.create_channel(&server.id, &format!("{}-x", "a".repeat(31)), false).unwrap();
    assert_eq!(long.name, format!("{}", "a".repeat(31)));
}

/// The in-process network, plus a presence set the test controls.
struct PresenceNet {
    inner: Arc<LoopbackNetwork>,
    online: std::sync::Mutex<BTreeSet<PublicKey>>,
    direct: std::sync::Mutex<BTreeSet<PublicKey>>,
}

impl Network for PresenceNet {
    fn publish(&self, record: &records::Record) {
        self.inner.publish(record)
    }

    fn fetch(&self, server: ServerId) -> Vec<records::Record> {
        self.inner.fetch(server)
    }

    fn online(&self, _: ServerId) -> Option<BTreeSet<PublicKey>> {
        Some(self.online.lock().unwrap().clone())
    }

    fn presence(&self, _: ServerId) -> Option<Reach> {
        let online = self.online.lock().unwrap().clone();
        Some(Reach { online, direct: self.direct.lock().unwrap().clone() })
    }
}

/// M2: members show online from sync's presence; someone online who isn't
/// a member never shows; a change in who is online is `members.changed`.
#[test]
fn presence_shows_online_members_only() {
    let world = World::new();
    let net = Arc::new(PresenceNet {
        inner: world.network.clone(),
        online: Default::default(),
        direct: Default::default(),
    });
    let events = Arc::new(CollectingSink::new());
    let owner = Core::open(
        Config { db_path: None, app_version: "test".into() },
        Ports {
            secrets: Arc::new(MemoryStore::new()),
            network: net.clone(),
            events: events.clone(),
            clock: world.clock.clone(),
            cipher: Arc::new(PendingGroupKeys),
        },
    )
    .unwrap();
    owner.restore_identity(OWNER_WORDS.split(' ').map(String::from).collect()).unwrap();
    let profile = ProfileInput { display_name: "Owner".into(), bio: None, avatar_data_url: None };
    owner.update_profile(profile).unwrap();
    let server = owner.create_server("Here", AgeRestriction::None).unwrap();
    let guest = world.peer();
    let guest_id = sign_up(&guest, "Guest").id;
    guest.core.join_server(&server.invite_link).unwrap();
    owner.sync_now().unwrap();
    let key = |id: &str| PublicKey::from_bytes(hex::decode(id).unwrap().try_into().unwrap());
    let presence = |id: &str| {
        owner.list_members(&server.id).unwrap().into_iter().find(|m| m.id == id).map(|m| m.presence)
    };
    assert_eq!(presence(&guest_id), Some(Presence::Offline));

    // The guest and a stranger with the secret come online.
    let stranger = PublicKey::from_bytes([0x55; 32]);
    net.online.lock().unwrap().extend([key(&guest_id), stranger]);
    events.take();
    owner.sync_now().unwrap();
    assert!(events.take().iter().any(|e| matches!(e, Event::MembersChanged { .. })));
    assert_eq!(presence(&guest_id), Some(Presence::Online));
    assert_eq!(presence(&hex::encode([0x55; 32])), None, "a non-member shows");

    // #187: online through the relay, then direct; null for us and offline.
    let connection = |id: &str| {
        owner
            .list_members(&server.id)
            .unwrap()
            .into_iter()
            .find(|m| m.id == id)
            .and_then(|m| m.connection)
    };
    let me = owner.get_identity().unwrap().unwrap().id;
    assert_eq!(connection(&guest_id), Some(Connection::Relay));
    assert_eq!(connection(&me), None);
    net.direct.lock().unwrap().insert(key(&guest_id));
    events.take();
    owner.sync_now().unwrap();
    assert!(
        events.take().iter().any(|e| matches!(e, Event::MembersChanged { .. })),
        "relay to direct"
    );
    assert_eq!(connection(&guest_id), Some(Connection::Direct));
    net.direct.lock().unwrap().clear();
    owner.sync_now().unwrap();
    assert!(
        events.take().iter().any(|e| matches!(e, Event::MembersChanged { .. })),
        "direct to relay"
    );

    // Nothing changed: nothing announced. Then the guest goes offline.
    owner.sync_now().unwrap();
    assert!(!events.take().iter().any(|e| matches!(e, Event::MembersChanged { .. })));
    net.online.lock().unwrap().remove(&key(&guest_id));
    owner.sync_now().unwrap();
    assert!(events.take().iter().any(|e| matches!(e, Event::MembersChanged { .. })));
    assert_eq!(presence(&guest_id), Some(Presence::Offline));
    assert_eq!(connection(&guest_id), None, "offline: no connection");
}

/// A secret store that always fails, like Linux without a Secret Service.
struct NoKeyring;

impl identity::SecretStore for NoKeyring {
    fn load(&self, _: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, identity::StoreError> {
        Err(identity::StoreError("no Secret Service".into()))
    }

    fn save(&self, _: &str, _: &[u8]) -> Result<(), identity::StoreError> {
        Err(identity::StoreError("no Secret Service".into()))
    }

    fn delete(&self, _: &str) -> Result<(), identity::StoreError> {
        Err(identity::StoreError("no Secret Service".into()))
    }
}

/// QA-20: no usable OS keychain. The core still starts, and says
/// `keyring-unavailable` instead of crashing or looking like a fresh install.
#[test]
fn no_keyring_is_its_own_error() {
    let core = Core::open(
        Config { db_path: None, app_version: "test".into() },
        Ports {
            secrets: Arc::new(NoKeyring),
            network: Arc::new(LoopbackNetwork::new()),
            events: Arc::new(CollectingSink::new()),
            clock: Arc::new(SystemClock),
            cipher: Arc::new(PendingGroupKeys),
        },
    )
    .expect("the core starts without a keychain");
    assert_eq!(core.get_identity().unwrap_err().code, ErrorCode::KeyringUnavailable);
    assert_eq!(core.create_identity(adult()).unwrap_err().code, ErrorCode::KeyringUnavailable);
    let reply = core.handle(r#"{"cmd":"getIdentity"}"#);
    assert_eq!(reply, r#"{"error":{"code":"keyring-unavailable"}}"#);
    assert!(core.get_app_info().is_ok(), "the rest of the core still answers");
}

/// T8-02 piece 4: a short link joins (the envelope fetched, opened and
/// checked), junk under its handle doesn't stop a join (C3), a forged
/// publication is refused (C1), and the envelope stops being served when
/// its invite is used up, revoked or expired (S1).
#[test]
fn short_links_join_and_die_with_their_invite() {
    use api::crypto::invite::{handle, seal, InvitePublication};
    use api::invite_link::{parse, InviteLink};

    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let server = owner.core.create_server("Short", AgeRestriction::None).unwrap();
    let server_id = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let made = owner.core.create_invite(&server.id, Some(common::NOW_MS + 3_600_000), Some(2)).unwrap();
    let invite_id = records::RecordId::from_bytes(hex::decode(&made.id).unwrap().try_into().unwrap());

    // Short, and shown again to its inviter; the v0 link is still made.
    let payload = made.short_link.strip_prefix("mvs://join/").unwrap();
    assert!(payload.len() <= 51 && !payload.contains('/'), "{} characters", payload.len());
    assert!(made.link.contains(&made.id));
    let listed = owner.core.list_invites(&server.id).unwrap();
    assert_eq!(listed.iter().find(|i| i.id == made.id).unwrap().short_link, made.short_link);
    let Ok(InviteLink::V1 { key, .. }) = parse(&made.short_link) else { panic!("not a v1 link") };
    let h = handle(&key);

    // Someone joins by it.
    let joined = |peer: &Peer| peer.core.list_servers().unwrap().iter().any(|s| s.id == server.id);
    let guest = world.peer();
    sign_up(&guest, "Guest");
    guest.core.join_server(&made.short_link).unwrap();
    assert!(joined(&guest), "joined by the short link");
    owner.core.sync_now().unwrap();

    // Only junk under the handle: garbage, and a publication with the right
    // k and envelope shape but signed by someone else. No join.
    world.network.drop_invite(h);
    world.network.serve_invite(server_id, h, b"junk", false);
    let forged_envelope = seal(&key, &server_id, &invite_id, &[0; 32]);
    let forged = InvitePublication::sign(&SigningKey::from_bytes(&[9; 32]), invite_id, h, forged_envelope).unwrap();
    world.network.serve_invite(server_id, h, &forged.encode(), false);
    let third = world.peer();
    sign_up(&third, "Third");
    third.core.join_server(&made.short_link).unwrap();
    assert!(!joined(&third), "joined on a forged publication");
    // The owner's next round serves the real one again, after the junk: it joins.
    owner.core.sync_now().unwrap();
    third.core.join_server(&made.short_link).unwrap();
    assert!(joined(&third), "the junk blocked the real answer");

    // That was its second use: used up, so no longer served or listed.
    owner.core.sync_now().unwrap();
    owner.core.sync_now().unwrap();
    // Only the junk planted above is left under the handle.
    let left = world.network.fetch_invite(h);
    assert!(!left.iter().filter_map(|p| InvitePublication::decode(p)).any(|p| p != forged), "a used-up invite is still served");
    let listed = owner.core.list_invites(&server.id).unwrap();
    assert!(listed.iter().find(|i| i.id == made.id).unwrap().short_link.is_empty());

    // Revoked: gone at once.
    let open = owner.core.create_invite(&server.id, None, None).unwrap();
    let Ok(InviteLink::V1 { key, .. }) = parse(&open.short_link) else { panic!("not a v1 link") };
    assert_eq!(world.network.fetch_invite(handle(&key)).len(), 1);
    owner.core.revoke_invite(&server.id, &open.id).unwrap();
    assert!(world.network.fetch_invite(handle(&key)).is_empty(), "served after a revoke");

    // Expired: gone at the next round.
    let soon = owner.core.create_invite(&server.id, Some(common::NOW_MS + 7_200_000), None).unwrap();
    let Ok(InviteLink::V1 { key, .. }) = parse(&soon.short_link) else { panic!("not a v1 link") };
    assert_eq!(world.network.fetch_invite(handle(&key)).len(), 1);
    *world.clock.0.lock().unwrap() += 10_000_000;
    owner.core.sync_now().unwrap();
    assert!(world.network.fetch_invite(handle(&key)).is_empty(), "served after it expired");
}

/// T8-02 + #261: closing a server stops its short links at once
/// (`stop_using`), and later rounds don't serve them again while it stays
/// listed, closed.
#[test]
fn a_closed_servers_short_links_stop() {
    use api::crypto::invite::handle;
    use api::invite_link::{parse, InviteLink};

    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let server = owner.core.create_server("Closing", AgeRestriction::None).unwrap();
    let made = owner.core.create_invite(&server.id, None, None).unwrap();
    let Ok(InviteLink::V1 { key, .. }) = parse(&made.short_link) else { panic!("not a v1 link") };
    assert_eq!(world.network.fetch_invite(handle(&key)).len(), 1);
    owner.core.close_server(&server.id).unwrap();
    assert!(world.network.fetch_invite(handle(&key)).is_empty(), "served after the close");
    owner.core.sync_now().unwrap();
    assert!(world.network.fetch_invite(handle(&key)).is_empty(), "served again while closed");
}

/// A joiner's network that records which servers it was told to close.
struct ClosingNet {
    world: Arc<LoopbackNetwork>,
    closed: std::sync::Mutex<Vec<ServerId>>,
}

impl Network for ClosingNet {
    fn publish(&self, record: &records::Record) {
        self.world.publish(record);
    }

    fn fetch(&self, server: ServerId) -> Vec<records::Record> {
        self.world.fetch(server)
    }

    fn fetch_invite(&self, handle: [u8; 32]) -> Vec<Vec<u8>> {
        self.world.fetch_invite(handle)
    }

    fn close_server(&self, server: ServerId) {
        self.closed.lock().unwrap().push(server);
    }
}

/// Tech lead (#263): a forged answer that fails the join closes the server
/// it made us open, so nothing stays subscribed for it.
#[test]
fn a_failed_short_link_join_closes_its_candidate() {
    use api::crypto::invite::{handle, seal, InvitePublication};
    use api::invite_link::{parse, InviteLink};

    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let server = owner.core.create_server("Candidate", AgeRestriction::None).unwrap();
    let server_id = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let made = owner.core.create_invite(&server.id, None, None).unwrap();
    let invite_id = records::RecordId::from_bytes(hex::decode(&made.id).unwrap().try_into().unwrap());
    let Ok(InviteLink::V1 { key, .. }) = parse(&made.short_link) else { panic!("not a v1 link") };
    let h = handle(&key);
    // Only a forgery under the handle: right k, signed by someone else.
    world.network.drop_invite(h);
    let forged = seal(&key, &server_id, &invite_id, &[0; 32]);
    let forged = InvitePublication::sign(&SigningKey::from_bytes(&[9; 32]), invite_id, h, forged).unwrap();
    world.network.serve_invite(server_id, h, &forged.encode(), false);

    let net = Arc::new(ClosingNet { world: world.network.clone(), closed: Default::default() });
    let joiner = world.peer_on(net.clone(), Arc::new(MemoryStore::new()), None, Arc::new(PendingGroupKeys));
    sign_up(&joiner, "Joiner");
    joiner.core.join_server(&made.short_link).unwrap();
    assert!(joiner.core.list_servers().unwrap().is_empty());
    assert_eq!(*net.closed.lock().unwrap(), [server_id], "the candidate stayed open");
}

/// #208: invites made, listed and revoked; moderator and up, and a revoke
/// only by the invite's author or someone ranked above it.
#[test]
fn invites_are_made_listed_and_revoked() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let guest = world.peer();
    let guest_id = sign_up(&guest, "Guest").id;
    let server = owner.core.create_server("Doors", AgeRestriction::None).unwrap();
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    guest.core.sync_now().unwrap();

    // A member can't make or list invites.
    assert_eq!(
        guest.core.create_invite(&server.id, None, None).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(guest.core.list_invites(&server.id).unwrap_err().code, ErrorCode::NotAllowed);

    // The owner makes one with a limit and an expiry; one in the past is refused.
    let at = common::NOW_MS + 3_600_000;
    assert_eq!(
        owner.core.create_invite(&server.id, Some(common::NOW_MS), None).unwrap_err().code,
        ErrorCode::BadRequest
    );
    owner.events.take();
    let made = owner.core.create_invite(&server.id, Some(at), Some(2)).unwrap();
    assert_eq!((made.expires_at, made.max_uses), (Some(at), Some(2)));
    assert!(made.link.starts_with("mvs://join/") && made.link.contains(&made.id));
    assert!(names(&owner.events.take()).contains(&"server.updated"));

    // Someone joins through it: one use.
    let third = world.peer();
    sign_up(&third, "Third");
    third.core.join_server(&made.link).unwrap();
    owner.core.sync_now().unwrap();
    let invites = owner.core.list_invites(&server.id).unwrap();
    assert_eq!(invites.len(), 2);
    let listed = invites.iter().find(|i| i.id == made.id).unwrap();
    assert_eq!(
        (listed.created_by.as_str(), listed.expires_at, listed.max_uses, listed.uses),
        (server.owner_id.as_str(), Some(at), 2, 1)
    );
    assert!(!listed.revoked);

    // Revoked once; then it's gone, and so is a bad ID.
    owner.core.revoke_invite(&server.id, &made.id).unwrap();
    let invites = owner.core.list_invites(&server.id).unwrap();
    assert!(invites.iter().find(|i| i.id == made.id).unwrap().revoked);
    assert_eq!(
        owner.core.revoke_invite(&server.id, &made.id).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(owner.core.revoke_invite(&server.id, "zz").unwrap_err().code, ErrorCode::NotFound);

    // A moderator revokes their own invite, not the owner's.
    owner.core.grant_role(&server.id, &guest_id, Role::Moderator).unwrap();
    guest.core.sync_now().unwrap();
    let first = owner.core.list_invites(&server.id).unwrap().remove(0);
    assert_eq!(
        guest.core.revoke_invite(&server.id, &first.id).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    let mine = guest.core.create_invite(&server.id, None, None).unwrap();
    assert_eq!((mine.expires_at, mine.max_uses), (None, None));
    guest.core.revoke_invite(&server.id, &mine.id).unwrap();
}

/// Voice transport inside the process: blobs queue until `pump`.
#[derive(Default)]
struct VoiceHub {
    queue: std::sync::Mutex<Vec<(PublicKey, PublicKey, Vec<u8>)>>,
    relay_only: std::sync::atomic::AtomicBool,
}

struct HubPort {
    hub: Arc<VoiceHub>,
    me: PublicKey,
}

impl VoiceNet for HubPort {
    fn relay_only(&self) -> bool {
        self.hub.relay_only.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn send(&self, to: PublicKey, blob: Vec<u8>) {
        assert!(blob.len() <= 16 * 1024);
        self.hub.queue.lock().unwrap().push((self.me, to, blob));
    }
}

fn user_key(id: &str) -> PublicKey {
    PublicKey::from_bytes(hex::decode(id).unwrap().try_into().unwrap())
}

impl VoiceHub {
    fn connect(self: &Arc<Self>, peer: &Peer, id: &str) {
        peer.core.set_voice(Arc::new(HubPort { hub: self.clone(), me: user_key(id) }));
    }

    /// Delivers until nothing is queued; returns what was delivered.
    fn pump(&self, peers: &[(&str, &Peer)]) -> Vec<(PublicKey, PublicKey, Vec<u8>)> {
        let mut delivered = Vec::new();
        loop {
            let batch = std::mem::take(&mut *self.queue.lock().unwrap());
            if batch.is_empty() {
                return delivered;
            }
            for (from, to, blob) in batch {
                if let Some((_, peer)) = peers.iter().find(|(id, _)| user_key(id) == to) {
                    peer.core.voice_received(Some(from), &blob);
                }
                delivered.push((from, to, blob));
            }
        }
    }
}

fn call_states(events: Vec<Event>) -> Vec<CallState> {
    events
        .into_iter()
        .filter_map(|e| match e {
            Event::CallState(state) => Some(state),
            _ => None,
        })
        .collect()
}

fn who(state: &CallState) -> Vec<&str> {
    state.participants.iter().map(|p| p.identity.as_str()).collect()
}

const SDP: &str = "v=0\r\na=fingerprint:sha-256 AB:CD:EF\r\na=setup:actpass\r\n";

/// Voice B: join, see each other, signal (verified only), mute, leave,
/// time out; nothing forged or replayed gets through.
#[test]
fn a_call_between_members() {
    let world = World::new();
    let hub = Arc::new(VoiceHub::default());
    let owner = world.peer();
    let owner_id = sign_up(&owner, "Owner").id;
    let alice = world.peer();
    let alice_id = sign_up(&alice, "Alice").id;
    let bob = world.peer();
    let bob_id = sign_up(&bob, "Bob").id;
    let server = owner.core.create_server("Talk", AgeRestriction::None).unwrap();
    for peer in [&alice, &bob] {
        peer.core.join_server(&server.invite_link).unwrap();
    }
    for peer in [&owner, &alice, &bob] {
        peer.core.sync_now().unwrap();
    }
    let room = owner.core.get_voice_room(&server.id).unwrap().room_id;
    let peers = [(owner_id.as_str(), &owner), (alice_id.as_str(), &alice), (bob_id.as_str(), &bob)];

    // No transport yet: not-implemented. Then relay-only: calls are off.
    assert_eq!(owner.core.join_call(&room).unwrap_err().code, ErrorCode::NotImplemented);
    for (id, peer) in peers {
        hub.connect(peer, id);
    }
    hub.relay_only.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(owner.core.join_call(&room).unwrap_err().code, ErrorCode::RelayOnly);
    hub.relay_only.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(owner.core.join_call(&"ab".repeat(32)).unwrap_err().code, ErrorCode::NotFound);
    // Calls happen only in the room: a text channel is not-allowed.
    let general = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    assert_ne!(general, room);
    assert_eq!(owner.core.join_call(&general).unwrap_err().code, ErrorCode::NotAllowed);
    // Every member derives the same room; an unknown server is not-found.
    assert_eq!(bob.core.get_voice_room(&server.id).unwrap().room_id, room);
    assert_eq!(bob.core.get_voice_room(&"ab".repeat(32)).unwrap_err().code, ErrorCode::NotFound);

    // The owner, then Alice, join; each sees both.
    let state = owner.core.join_call(&room).unwrap();
    assert_eq!((who(&state), state.full), (vec![owner_id.as_str()], false));
    assert!(owner.core.in_call());
    hub.pump(&peers);
    let state = alice.core.join_call(&room).unwrap();
    assert_eq!(who(&state), [alice_id.as_str(), owner_id.as_str()]);
    owner.events.take();
    hub.pump(&peers);
    let seen = call_states(owner.events.take());
    assert_eq!(who(seen.last().unwrap()), [owner_id.as_str(), alice_id.as_str()]);

    // Bob isn't in the call but sees who is: one voice.room at his tick,
    // and the same from getVoiceRoom. Identity and muted only.
    bob.events.take();
    bob.core.voice_tick();
    let rooms: Vec<VoiceRoom> = bob
        .events
        .take()
        .into_iter()
        .filter_map(|e| match e {
            Event::VoiceRoom(room) => Some(room),
            _ => None,
        })
        .collect();
    assert_eq!(rooms.len(), 1);
    let mut inside: Vec<&str> = rooms[0].participants.iter().map(|p| p.user_id.as_str()).collect();
    inside.sort();
    let mut want = [owner_id.as_str(), alice_id.as_str()];
    want.sort();
    assert_eq!(inside, want);
    assert_eq!(
        (rooms[0].server_id.as_str(), rooms[0].room_id.as_str()),
        (server.id.as_str(), room.as_str())
    );
    assert_eq!(bob.core.get_voice_room(&server.id).unwrap(), rooms[0]);
    let json = serde_json::to_value(&rooms[0]).unwrap();
    assert_eq!(json["participants"][0].as_object().unwrap().len(), 2, "more than id and muted");

    // Signalling: only to a participant, offers and answers with a fingerprint.
    assert_eq!(
        alice.core.send_signal(&bob_id, SignalKind::Offer, SDP.into()).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(
        alice.core.send_signal(&owner_id, SignalKind::Offer, "v=0\r\n".into()).unwrap_err().code,
        ErrorCode::BadRequest
    );
    let big = "x".repeat(12 * 1024 + 1);
    assert_eq!(
        alice.core.send_signal(&owner_id, SignalKind::Ice, big).unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        bob.core.send_signal(&owner_id, SignalKind::Offer, SDP.into()).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    alice.core.send_signal(&owner_id, SignalKind::Offer, SDP.into()).unwrap();
    let delivered = hub.pump(&peers);
    let signals: Vec<Event> =
        owner.events.take().into_iter().filter(|e| matches!(e, Event::CallSignal { .. })).collect();
    assert_eq!(
        signals,
        [Event::CallSignal {
            from: alice_id.clone(),
            kind: SignalKind::Offer,
            payload: SDP.into(),
            fingerprint_verified: true,
        }]
    );

    // The same blob again (a replay), and one changed in transit: dropped.
    let (from, _, blob) = delivered.last().unwrap().clone();
    owner.core.voice_received(Some(from), &blob);
    let changed = String::from_utf8(blob.clone()).unwrap().replace("AB:CD:EF", "AB:CD:EE");
    owner.core.voice_received(Some(from), changed.as_bytes());
    // Signed by Alice, but sync says Bob sent it.
    owner.core.voice_received(Some(user_key(&bob_id)), &blob);
    assert!(owner.events.take().is_empty());

    // Mute shows; leave drops; a quiet participant times out.
    alice.core.set_muted(true).unwrap();
    hub.pump(&peers);
    let seen = call_states(owner.events.take());
    assert!(seen.last().unwrap().participants.iter().any(|p| p.identity == alice_id && p.muted));
    alice.core.leave_call().unwrap();
    alice.core.leave_call().unwrap();
    assert!(!alice.core.in_call());
    assert_eq!(alice.core.set_muted(false).unwrap_err().code, ErrorCode::NotAllowed);
    hub.pump(&peers);
    let seen = call_states(owner.events.take());
    assert_eq!(who(seen.last().unwrap()), [owner_id.as_str()]);

    bob.core.join_call(&room).unwrap();
    hub.pump(&peers);
    *world.clock.0.lock().unwrap() += 31_000;
    owner.events.take();
    owner.core.voice_tick();
    let seen = call_states(owner.events.take());
    assert_eq!(who(seen.last().unwrap()), [owner_id.as_str()], "Bob didn't time out");

    // Relay-only turns on mid-call: the owner leaves at the next tick, says
    // bye, and call.state keeps its shape.
    hub.pump(&peers);
    hub.relay_only.store(true, std::sync::atomic::Ordering::SeqCst);
    owner.events.take();
    owner.core.voice_tick();
    assert!(!owner.core.in_call());
    let seen = call_states(owner.events.take());
    let last = seen.last().unwrap();
    assert_eq!(
        (last.channel_id.as_str(), last.participants.len(), last.full),
        (room.as_str(), 0, false)
    );
    let sent = hub.pump(&peers);
    assert!(
        !sent.is_empty()
            && sent.iter().all(|(from, _, blob)| {
                *from == user_key(&owner_id)
                    && String::from_utf8_lossy(blob).contains(r#"\"kind\":\"bye\""#)
            })
    );
    hub.relay_only.store(false, std::sync::atomic::Ordering::SeqCst);
    owner.core.leave_call().unwrap();
    assert!(!owner.core.in_call());
}

/// A network that records each relay-only setting it was given.
struct RelayOnlyNet {
    world: Arc<LoopbackNetwork>,
    applied: std::sync::Mutex<Vec<bool>>,
}

impl RelayOnlyNet {
    fn new(world: &Arc<LoopbackNetwork>) -> Arc<RelayOnlyNet> {
        Arc::new(RelayOnlyNet { world: world.clone(), applied: Default::default() })
    }

    fn applied(&self) -> Vec<bool> {
        self.applied.lock().unwrap().clone()
    }
}

impl Network for RelayOnlyNet {
    fn publish(&self, record: &records::Record) {
        self.world.publish(record);
    }

    fn fetch(&self, server: ServerId) -> Vec<records::Record> {
        self.world.fetch(server)
    }

    fn set_relay_only(&self, on: bool) {
        self.applied.lock().unwrap().push(on);
    }
}

fn voice_rooms(events: Vec<Event>) -> Vec<VoiceRoom> {
    events
        .into_iter()
        .filter_map(|e| match e {
            Event::VoiceRoom(room) => Some(room),
            _ => None,
        })
        .collect()
}

/// GAP-6r: `setRelayOnly` is stored, given to the network at start and on
/// each change, ends my call (with a bye), refuses calls while on, and is
/// shown up front on `getAppInfo`, `getVoiceRoom`, `voice.room` and
/// `call.state`. It survives a restart.
#[test]
fn relay_only_is_stored_applied_and_turns_calls_off() {
    let dir = std::env::temp_dir().join(format!("mvs-api-relayonly-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("core.db");
    let world = World::new();
    let hub = Arc::new(VoiceHub::default());
    let net = RelayOnlyNet::new(&world.network);
    let open = |net: Arc<RelayOnlyNet>, secrets: Arc<MemoryStore>| {
        world.peer_on(net, secrets, Some(db.clone()), Arc::new(PendingGroupKeys))
    };
    let owner = open(net.clone(), Arc::new(MemoryStore::new()));
    let owner_id = sign_up(&owner, "Owner").id;
    let alice = world.peer();
    let alice_id = sign_up(&alice, "Alice").id;
    let server = owner.core.create_server("Quiet", AgeRestriction::None).unwrap();
    alice.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    alice.core.sync_now().unwrap();
    let peers = [(owner_id.as_str(), &owner), (alice_id.as_str(), &alice)];
    for (id, peer) in peers {
        hub.connect(peer, id);
    }

    // Off by default, and the network was told so at start.
    assert_eq!(net.applied(), [false]);
    assert!(!owner.core.get_app_info().unwrap().relay_only);
    let room = owner.core.get_voice_room(&server.id).unwrap();
    assert!(!room.relay_only);
    let state = owner.core.join_call(&room.room_id).unwrap();
    assert!(!state.relay_only);
    hub.pump(&peers);
    alice.core.join_call(&room.room_id).unwrap();
    hub.pump(&peers);
    assert!(owner.core.in_call());

    // On, through the dispatcher: my call ends at once with a bye, and the
    // ended call.state says why.
    owner.events.take();
    let answer: serde_json::Value =
        serde_json::from_str(&owner.core.handle(r#"{"cmd":"setRelayOnly","args":{"on":true}}"#))
            .unwrap();
    assert_eq!(answer, serde_json::json!({ "ok": null }));
    assert!(!owner.core.in_call());
    let ended = call_states(owner.events.take());
    let last = ended.last().expect("a call.state for the ended call");
    assert_eq!((last.participants.len(), last.relay_only), (0, true));
    let sent = hub.pump(&peers);
    assert!(sent.iter().any(|(from, to, blob)| {
        *from == user_key(&owner_id)
            && *to == user_key(&alice_id)
            && String::from_utf8_lossy(blob).contains(r#"\"kind\":\"bye\""#)
    }));
    assert_eq!(net.applied(), [false, true]);

    // Up front: app info, the room, and one voice.room per server at the
    // next tick. Joining is refused.
    assert!(owner.core.get_app_info().unwrap().relay_only);
    assert!(owner.core.get_voice_room(&server.id).unwrap().relay_only);
    owner.core.voice_tick();
    let rooms = voice_rooms(owner.events.take());
    assert_eq!(rooms.len(), 1);
    assert!(rooms[0].relay_only && rooms[0].server_id == server.id);
    assert_eq!(owner.core.join_call(&room.room_id).unwrap_err().code, ErrorCode::RelayOnly);
    // Another member's setting is theirs: Alice's room isn't relay-only.
    assert!(!alice.core.get_voice_room(&server.id).unwrap().relay_only);

    // A restart keeps it: read before the network starts, and given to the
    // network as the core opens.
    let secrets = owner.secrets.clone();
    drop(owner);
    assert!(api::stored_relay_only(&db));
    let net = RelayOnlyNet::new(&world.network);
    let owner = open(net.clone(), secrets);
    assert_eq!(net.applied(), [true]);
    assert!(owner.core.get_app_info().unwrap().relay_only);
    hub.connect(&owner, &owner_id);
    assert_eq!(owner.core.join_call(&room.room_id).unwrap_err().code, ErrorCode::RelayOnly);

    // Off again: applied, stored, and calls work.
    owner.core.set_relay_only(false).unwrap();
    assert_eq!(net.applied(), [true, false]);
    assert!(!api::stored_relay_only(&db));
    assert!(!owner.core.get_voice_room(&server.id).unwrap().relay_only);
    assert!(!owner.core.join_call(&room.room_id).unwrap().relay_only);
    owner.core.leave_call().unwrap();

    // No database yet (or an unreadable one): off.
    assert!(!api::stored_relay_only(&dir.join("missing").join("core.db")));
    drop(owner);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Voice B: 5 is the cap; a kicked member's hello doesn't count.
#[test]
fn a_call_holds_five_and_kicked_members_are_ignored() {
    let world = World::new();
    let hub = Arc::new(VoiceHub::default());
    let owner = world.peer();
    let owner_id = sign_up(&owner, "Owner").id;
    let server = owner.core.create_server("Crowd", AgeRestriction::None).unwrap();
    let mut others = Vec::new();
    for n in 0..6 {
        let peer = world.peer();
        let id = sign_up(&peer, &format!("P{n}")).id;
        peer.core.join_server(&server.invite_link).unwrap();
        others.push((id, peer));
    }
    owner.core.sync_now().unwrap();
    for (_, peer) in &others {
        peer.core.sync_now().unwrap();
    }
    let room = owner.core.get_voice_room(&server.id).unwrap().room_id;
    hub.connect(&owner, &owner_id);
    for (id, peer) in &others {
        hub.connect(peer, id);
    }
    let mut peers: Vec<(&str, &Peer)> = vec![(owner_id.as_str(), &owner)];
    peers.extend(others.iter().map(|(id, peer)| (id.as_str(), peer)));

    owner.core.join_call(&room).unwrap();
    hub.pump(&peers);
    for (_, peer) in &others[..4] {
        peer.core.join_call(&room).unwrap();
        hub.pump(&peers);
    }
    let state = owner.core.join_call(&room).unwrap();
    assert_eq!((state.participants.len(), state.full), (5, true));
    let (_, sixth) = &others[4];
    assert_eq!(sixth.core.join_call(&room).unwrap_err().code, ErrorCode::CallFull);
    assert!(!sixth.core.in_call());

    // P0 is kicked; before it learns, its hellos go out and are ignored.
    let (kicked_id, kicked) = &others[0];
    owner.core.kick_member(&server.id, kicked_id).unwrap();
    owner.events.take();
    owner.core.voice_tick();
    let seen = call_states(owner.events.take());
    assert_eq!(seen.last().unwrap().participants.len(), 4);
    *world.clock.0.lock().unwrap() += 11_000;
    kicked.core.voice_tick();
    hub.pump(&peers);
    let state = owner.core.join_call(&room).unwrap();
    assert!(!who(&state).contains(&kicked_id.as_str()));
    let roster = owner.core.get_voice_room(&server.id).unwrap();
    assert!(!roster.participants.iter().any(|p| &p.user_id == kicked_id));
    assert!(roster.participants.len() <= 5);

    // Once P0 reads its kick: no roster for it, by command or event.
    kicked.core.sync_now().unwrap();
    kicked.events.take();
    kicked.core.voice_tick();
    assert!(!kicked.events.take().iter().any(|e| matches!(e, Event::VoiceRoom(_))));
    let code = kicked.core.get_voice_room(&server.id).unwrap_err().code;
    assert!(matches!(code, ErrorCode::NotFound | ErrorCode::NotAllowed), "{code:?}");
}

/// Server lifecycle (#229, #251), api piece: who may rename, leave and
/// delete; the rollout gate (an older member, or one with no profile post,
/// gives `update-needed` before anything is signed); and the purge.
#[test]
fn servers_are_renamed_left_and_deleted_behind_the_rollout_gate() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let guest = world.peer();
    let guest_id = sign_up(&guest, "Guest").id;
    let third = world.peer();
    sign_up(&third, "Third");
    let server = owner.core.create_server("Old name", AgeRestriction::None).unwrap();
    for peer in [&guest, &third] {
        peer.core.join_server(&server.invite_link).unwrap();
    }
    for peer in [&owner, &guest, &third] {
        peer.core.sync_now().unwrap();
    }
    let channel = owner.core.list_channels(&server.id).unwrap().remove(0).id;
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());

    // Rename: trimmed, strict rule, owner or admin only.
    assert_eq!(
        guest.core.rename_server(&server.id, "Mine now").unwrap_err().code,
        ErrorCode::NotAllowed
    );
    for bad in ["   ", "a\u{202E}b", &"x".repeat(101)] {
        assert_eq!(
            owner.core.rename_server(&server.id, bad).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }
    let renamed = owner.core.rename_server(&server.id, "  New name ").unwrap();
    assert_eq!(renamed.name, "New name");
    guest.events.take();
    guest.core.sync_now().unwrap();
    let updated: Vec<String> = guest
        .events
        .take()
        .into_iter()
        .filter_map(|e| match e {
            Event::ServerUpdated(s) => Some(s.name),
            _ => None,
        })
        .collect();
    assert_eq!(updated, ["New name"]);
    assert_eq!(guest.core.list_servers().unwrap()[0].name, "New name");

    // The gate: a member on an older version (a fixed key here) joins with
    // no profile post, then posts one without the cap.
    let invite = model_invite(&server.invite_link);
    let old = SigningKey::from_bytes(&[42; 32]);
    let mut deps: BTreeSet<_> = world.network.fetch(server_key).iter().map(|r| r.id()).collect();
    deps.insert(invite);
    let join = authority::sign_control(
        &old,
        server_key,
        1,
        deps.into_iter().collect(),
        1,
        &authority::Action::Join { invite },
    )
    .unwrap();
    world.network.publish(&join);
    owner.core.sync_now().unwrap();
    let signed = world.network.fetch(server_key).len();
    assert_eq!(
        owner.core.rename_server(&server.id, "Blocked").unwrap_err().code,
        ErrorCode::UpdateNeeded,
        "a member with no profile post"
    );
    let profile = |caps: &str, seq| {
        let text = format!(r#"{{"mvs":"mvs/profile/v1","displayName":"Old"{caps}}}"#);
        let heads = world.network.fetch(server_key).iter().map(|r| r.id()).collect();
        authority::sign_control(&old, server_key, seq, heads, 2, &authority::Action::Post { text })
            .unwrap()
    };
    world.network.publish(&profile("", 2));
    owner.core.sync_now().unwrap();
    for code in [
        owner.core.rename_server(&server.id, "Blocked").unwrap_err().code,
        owner.core.close_server(&server.id).unwrap_err().code,
    ] {
        assert_eq!(code, ErrorCode::UpdateNeeded, "an older member's profile");
    }
    // The UI can say who blocks it.
    let old_id = hex::encode(old.verifying_key().to_bytes());
    assert_eq!(owner.core.get_update_needed(&server.id).unwrap(), [old_id]);
    assert_eq!(world.network.fetch(server_key).len(), signed + 1, "signed past the gate");
    // It updates: the cap in its latest profile post opens the gate.
    world.network.publish(&profile(r#","caps":["server-lifecycle-v1"]"#, 3));
    for peer in [&owner, &guest, &third] {
        peer.core.sync_now().unwrap();
    }
    assert!(owner.core.get_update_needed(&server.id).unwrap().is_empty());

    // Leave: not the owner; a member leaves and the server leaves their
    // device; the others see them gone.
    assert_eq!(owner.core.leave_server(&server.id).unwrap_err().code, ErrorCode::NotAllowed);
    guest.events.take();
    guest.core.leave_server(&server.id).unwrap();
    assert!(guest.events.take().contains(&Event::ServerRemoved {
        server_id: server.id.clone(),
        reason: Some(RemovedReason::Left),
    }));
    assert!(guest.core.list_servers().unwrap().is_empty());
    assert_eq!(guest.core.get_messages(&channel, 0, 9).unwrap_err().code, ErrorCode::NotFound);
    owner.core.sync_now().unwrap();
    assert!(!owner.core.list_members(&server.id).unwrap().iter().any(|m| m.id == guest_id));

    // Delete: the owner only. Not purged at once (the Tech Lead's #229
    // answer): closed, read-only, still listed; no longer synced.
    assert_eq!(third.core.close_server(&server.id).unwrap_err().code, ErrorCode::NotAllowed);
    owner.events.take();
    owner.core.close_server(&server.id).unwrap();
    let closed = Event::ServerClosed { server_id: server.id.clone() };
    assert!(owner.events.take().contains(&closed));
    third.events.take();
    third.core.sync_now().unwrap();
    assert!(third.events.take().contains(&closed));
    for peer in [&owner, &third] {
        let listed = peer.core.list_servers().unwrap();
        assert_eq!((listed.len(), listed[0].closed), (1, true));
        assert_eq!(peer.core.list_channels(&server.id).unwrap().len(), 1, "history gone at once");
    }
    // A closed server isn't synced: a record a stale peer still serves
    // isn't read.
    let late = authority::sign_control(
        &old,
        server_key,
        9,
        world.network.fetch(server_key).iter().map(|r| r.id()).collect(),
        3,
        &authority::Action::Post { text: "late".into() },
    )
    .unwrap();
    world.network.publish(&late);
    third.events.take();
    third.core.sync_now().unwrap();
    assert!(third.events.take().is_empty(), "a closed server was synced");

    // An old invite, for someone who was never in it: closed, not unknown.
    let stranger = world.peer();
    sign_up(&stranger, "Stranger");
    stranger.core.join_server(&server.invite_link).unwrap();
    assert!(matches!(
        stranger.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Closed { .. }))
    ));
    assert!(stranger.core.list_servers().unwrap().is_empty());

    // 30 days after the close was received, the rest goes; the genesis and
    // the close stay, so the old invite still reads as closed.
    *world.clock.0.lock().unwrap() += CLOSED_RETENTION_MS - 10_000;
    third.events.take();
    third.core.sync_now().unwrap();
    assert_eq!(third.core.list_servers().unwrap().len(), 1, "purged before 30 days");
    *world.clock.0.lock().unwrap() += 20_000;
    third.core.sync_now().unwrap();
    let removed =
        Event::ServerRemoved { server_id: server.id.clone(), reason: Some(RemovedReason::Closed) };
    assert!(third.events.take().contains(&removed));
    assert!(third.core.list_servers().unwrap().is_empty());
    assert_eq!(third.core.list_channels(&server.id).unwrap_err().code, ErrorCode::NotFound);
    third.core.join_server(&server.invite_link).unwrap();
    assert!(matches!(
        third.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Closed { .. }))
    ));
    assert!(third.core.list_servers().unwrap().is_empty());
}

/// The invite record ID an invite link names.
fn model_invite(link: &str) -> records::RecordId {
    let invite = link.strip_prefix("mvs://join/").unwrap().split('/').nth(1).unwrap();
    records::RecordId::from_bytes(hex::decode(invite).unwrap().try_into().unwrap())
}

/// A peer with the real group cipher (so re-keys happen), optionally on a
/// database file (so it can restart).
fn keyed_peer(world: &World, secrets: Arc<MemoryStore>, db: Option<std::path::PathBuf>) -> Peer {
    world.peer_with(secrets, db, Arc::new(api::crypto::GroupKeysCipher::locked()))
}

fn key_rotations(world: &World, server: ServerId) -> usize {
    let mut seen = BTreeSet::new();
    for r in world.network.fetch(server).iter().filter(|r| r.kind() == Kind::KeyRotation) {
        seen.insert(r.deps().to_vec());
    }
    seen.len()
}

fn leaves_by(world: &World, server: ServerId, who: &str) -> usize {
    let who = PublicKey::from_bytes(hex::decode(who).unwrap().try_into().unwrap());
    world
        .network
        .fetch(server)
        .iter()
        .filter(|r| r.kind() == Kind::Leave && r.author() == who)
        .count()
}

/// An older member (a fixed key): joins, then posts a profile with or
/// without the lifecycle cap.
fn older_member(world: &World, server: &ServerSummary, key: &SigningKey) {
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let invite = model_invite(&server.invite_link);
    let mut deps: BTreeSet<_> = world.network.fetch(server_key).iter().map(|r| r.id()).collect();
    deps.insert(invite);
    let join = authority::sign_control(
        key,
        server_key,
        1,
        deps.into_iter().collect(),
        1,
        &authority::Action::Join { invite },
    )
    .unwrap();
    world.network.publish(&join);
}

fn older_profile(world: &World, server: &ServerSummary, key: &SigningKey, seq: u64, cap: bool) {
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    let caps = if cap { r#","caps":["server-lifecycle-v1"]"# } else { "" };
    let text = format!(r#"{{"mvs":"mvs/profile/v1","displayName":"Old"{caps}}}"#);
    let heads = world
        .network
        .fetch(server_key)
        .iter()
        .filter(|r| r.scope() == Scope::Control)
        .map(|r| r.id())
        .collect();
    let post =
        authority::sign_control(key, server_key, seq, heads, 2, &authority::Action::Post { text })
            .unwrap();
    world.network.publish(&post);
}

/// The Tech Lead's #261 condition: the `Leave` reaches the others, who see
/// the member gone and re-key.
#[test]
fn a_leave_reaches_the_others_and_they_re_key() {
    let world = World::new();
    let owner = keyed_peer(&world, Arc::new(MemoryStore::new()), None);
    sign_up(&owner, "Owner");
    let guest = keyed_peer(&world, Arc::new(MemoryStore::new()), None);
    let guest_id = sign_up(&guest, "Guest").id;
    let server = owner.core.create_server("Keys", AgeRestriction::None).unwrap();
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    guest.core.join_server(&server.invite_link).unwrap();
    owner.core.sync_now().unwrap();
    guest.core.sync_now().unwrap();
    let before = key_rotations(&world, server_key);

    guest.core.leave_server(&server.id).unwrap();
    assert_eq!(leaves_by(&world, server_key, &guest_id), 1, "the Leave never went out");
    owner.core.sync_now().unwrap();
    assert!(!owner.core.list_members(&server.id).unwrap().iter().any(|m| m.id == guest_id));
    assert!(key_rotations(&world, server_key) > before, "nobody re-keyed after the leave");
    // Delivered: nothing is retried.
    guest.core.sync_now().unwrap();
    assert_eq!(leaves_by(&world, server_key, &guest_id), 1);
}

/// Both leads (#261): a leave is never blocked by the rollout gate. With
/// an older member in, it's gone from the UI at once and its `Leave` waits
/// in the outbox, across a restart, until the gate opens; a kick first
/// drops it.
#[test]
fn a_leave_waits_in_the_outbox_until_the_gate_opens() {
    let world = World::new();
    let owner = keyed_peer(&world, Arc::new(MemoryStore::new()), None);
    sign_up(&owner, "Owner");
    let dir = std::env::temp_dir().join(format!("mvs-leave-outbox-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let guest = keyed_peer(&world, secrets.clone(), Some(dir.join("core.db")));
    let guest_id = sign_up(&guest, "Guest").id;
    let kicked = keyed_peer(&world, Arc::new(MemoryStore::new()), None);
    let kicked_id = sign_up(&kicked, "Kicked").id;
    let server = owner.core.create_server("Outbox", AgeRestriction::None).unwrap();
    let server_key = ServerId::from_bytes(hex::decode(&server.id).unwrap().try_into().unwrap());
    for peer in [&guest, &kicked] {
        peer.core.join_server(&server.invite_link).unwrap();
    }
    let old = SigningKey::from_bytes(&[43; 32]);
    older_member(&world, &server, &old);
    older_profile(&world, &server, &old, 2, false);
    for peer in [&owner, &guest, &kicked] {
        peer.core.sync_now().unwrap();
    }

    // The gate is shut: both leave anyway, and the server is gone at once.
    for (peer, id) in [(&guest, &guest_id), (&kicked, &kicked_id)] {
        peer.events.take();
        peer.core.leave_server(&server.id).unwrap();
        assert!(peer.events.take().contains(&Event::ServerRemoved {
            server_id: server.id.clone(),
            reason: Some(RemovedReason::Left),
        }));
        assert!(peer.core.list_servers().unwrap().is_empty());
        assert_eq!(leaves_by(&world, server_key, id), 0, "a Leave went out past the gate");
    }

    // A kick while the Leave waits: it's dropped, never sent.
    owner.core.kick_member(&server.id, &kicked_id).unwrap();
    kicked.core.sync_now().unwrap();

    // The guest restarts with the Leave still waiting.
    drop(guest);
    let guest = keyed_peer(&world, secrets, Some(dir.join("core.db")));
    guest.core.sync_now().unwrap();
    assert_eq!(leaves_by(&world, server_key, &guest_id), 0);
    assert!(guest.core.list_servers().unwrap().is_empty());

    // The older member updates: the gate opens, the waiting Leave goes out
    // once, and the owner sees it and re-keys. The kicked one's never does.
    owner.core.sync_now().unwrap();
    let before = key_rotations(&world, server_key);
    older_profile(&world, &server, &old, 3, true);
    guest.core.sync_now().unwrap();
    kicked.core.sync_now().unwrap();
    assert_eq!(leaves_by(&world, server_key, &guest_id), 1);
    assert_eq!(leaves_by(&world, server_key, &kicked_id), 0, "a kicked member's Leave went out");
    owner.core.sync_now().unwrap();
    assert!(!owner.core.list_members(&server.id).unwrap().iter().any(|m| m.id == guest_id));
    assert!(key_rotations(&world, server_key) > before, "nobody re-keyed after the leave");
    guest.core.sync_now().unwrap();
    assert_eq!(leaves_by(&world, server_key, &guest_id), 1, "resent after delivery");
    drop(guest);
    let _ = std::fs::remove_dir_all(&dir);
}
