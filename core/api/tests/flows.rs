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
            Event::ChannelUpdated(_) => "channel.updated",
            Event::MembersChanged { .. } => "members.changed",
            Event::JoinStatus(_) => "join.status",
            Event::UpdateReady(_) => "update.ready",
            Event::Notice { .. } => "notice",
            Event::MessageAdded(_) => "message.added",
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
    assert_eq!(owner.core.list_channels(&server.id).unwrap()[0].name, "general");

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
    let link = format!("mvs://join/{}/{}", hex::encode(forged_id.as_bytes()), "00".repeat(32));
    joiner.core.join_server(&link).unwrap();
    assert!(matches!(
        joiner.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Invalid { .. }))
    ));
    assert_eq!(joiner.core.list_servers().unwrap().len(), 0);

    // Unknown server, and a link that isn't one.
    joiner
        .core
        .join_server(&format!("mvs://join/{}/{}", "ab".repeat(32), "00".repeat(32)))
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
        let link =
            format!("mvs://join/{}/{}", hex::encode(genesis.server().as_bytes()), "00".repeat(32));
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
    let bogus = format!("mvs://join/{}/{}", server.id, "11".repeat(32));
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
        serde_json::json!({ "version": "0.1.0-alpha", "updateChannel": "stable" })
    );
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

    // No key yet, and a plain member can't make one.
    guest.core.sync_now().unwrap();
    assert_eq!(
        guest.core.send_message(&channel, "first?", None).unwrap_err().code,
        ErrorCode::NotAllowed
    );
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
}

impl Spy {
    fn new() -> Spy {
        Spy { inner: api::crypto::GroupKeysCipher::locked(), channels: Default::default() }
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
    let dan = open("zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong", "Dan", Arc::new(GroupKeysCipher::locked()));
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
    assert!(carol_cipher.decrypt(server_key, ch, after_record.author(), after_record.body()).is_err());
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
    let bodies: Vec<String> =
        owner.core.get_messages(&channel, 0, 10).unwrap().messages.into_iter().map(|m| m.body).collect();
    assert_eq!(bodies, ["before", "after", "still here", "back"]);
}
