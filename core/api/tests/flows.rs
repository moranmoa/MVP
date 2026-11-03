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

/// Creates an identity and passes the recovery check.
fn sign_up(peer: &Peer, name: &str) -> Identity {
    let created = peer.core.create_identity(adult()).unwrap();
    let answers: BTreeMap<usize, String> = [(1, created.words[0].clone())].into();
    assert!(peer.core.check_recovery_words(answers).unwrap());
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
    let answers: BTreeMap<usize, String> =
        [(3, created.words[2].clone()), (9, created.words[8].clone())].into();
    assert!(owner.core.check_recovery_words(answers).unwrap());
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
    assert_eq!(
        alice.core.kick_member(&server.id, &bob_id).unwrap_err().code,
        ErrorCode::NotAllowed
    );
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
    assert_eq!(names(&owner.events.take()), ["members.changed", "notice"]);
    let last = cipher.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.len(), 2);
}

#[test]
fn a_genesis_must_belong_to_its_server_id() {
    let world = World::new();
    let owner = world.peer();
    sign_up(&owner, "Owner");
    let real = owner.core.create_server("Real", AgeRestriction::None).unwrap();

    // Someone else signs a genesis claiming a made-up server ID.
    let forged_id = ServerId::from_bytes([9; 32]);
    let body =
        format!(r#"{{"v":1,"name":"Fake","ageRestriction":"none","nonce":"{}"}}"#, "00".repeat(16));
    let genesis = Draft {
        server: forged_id,
        scope: Scope::Control,
        seq: 0,
        deps: vec![],
        auth_deps: vec![],
        ts: 1,
        kind: Kind::Genesis,
        body: body.into_bytes(),
    }
    .sign(&SigningKey::from_bytes(&[5; 32]))
    .unwrap();
    world.network.publish(&genesis);

    let joiner = world.peer();
    sign_up(&joiner, "Joiner");
    let link = format!("mvs://join/{}", hex::encode(forged_id.as_bytes()));
    joiner.core.join_server(&link).unwrap();
    assert!(matches!(
        joiner.events.take().last(),
        Some(Event::JoinStatus(JoinStatus::Invalid { .. }))
    ));
    assert_eq!(joiner.core.list_servers().unwrap().len(), 0);

    // Unknown server, and a link that isn't one.
    joiner.core.join_server(&format!("mvs://join/{}", "ab".repeat(32))).unwrap();
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
    assert_eq!(peer.core.get_recovery_words().unwrap(), words);
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
