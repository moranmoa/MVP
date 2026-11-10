use identity::{
    check_positions, is_word, suggest, LockParams, MemoryStore, PhraseError, RecoveryPhrase,
    SecretStore, StoreError, Subkey, Vault, VaultError, VaultState, ENTRY_RECOVERY_CHECK,
    ENTRY_SECRET, KEYCHAIN_SERVICE, WORD_COUNT,
};

const ABANDON: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";
const ZOO: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

/// The cheapest cost a lock may use, so the tests stay fast.
const CHEAP: LockParams = LockParams::MIN;

fn parse(words: &str) -> RecoveryPhrase {
    RecoveryPhrase::parse(words).expect("valid phrase")
}

fn joined(phrase: &RecoveryPhrase) -> String {
    phrase.words().join(" ")
}

// --- Known vectors ---------------------------------------------------------

/// Entropy -> words: the official BIP-39 test vectors (trezor/python-mnemonic
/// vectors.json, 128-bit entries).
#[test]
fn bip39_vectors() {
    let vectors = [
        ([0x00; 16], ABANDON),
        ([0x7f; 16], LEGAL),
        (
            [0x80; 16],
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        ),
        ([0xff; 16], ZOO),
        (
            hex16("9e885d952ad362caeb4efe34a8e91bd2"),
            "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic",
        ),
    ];
    for (entropy, words) in vectors {
        let phrase = RecoveryPhrase::from_entropy(&entropy);
        assert_eq!(joined(&phrase), words);
        assert_eq!(*parse(words).entropy(), entropy);
    }
}

/// Words -> keys. Computed independently with Python (hashlib PBKDF2 + HMAC
/// HKDF + `cryptography` Ed25519), not by this crate. If one of these
/// changes, every user's identity changes.
#[test]
fn derivation_vectors() {
    let vectors = [
        (
            ABANDON,
            "286bcb2c389b909eaacb35cd3e70999f665fac768a97c56d0911a5b630a1899f",
            "19c2f6189452071897d886a0760bbf3a25967660938491e6aec080369cb5ae7e",
            "3da8d69912e964c5cc6851f48307caebb00dd78d875eddb7b8f1a9b708c6d73c",
        ),
        (
            LEGAL,
            "10c9b2cd75b1c2cc6c4b9bca9421e85ee7350ba2641c4d0bfd123d44da2dc4fd",
            "0d40f0b0543e252fe01b5a3c767f8ed34fec026e1bafbbd2a6566f893bd0fed4",
            "309629642dd1b6429f0649aa151b5645b23adaa4262a24a02cb16fd01ab1e00b",
        ),
        (
            ZOO,
            "0d72026f5e46534010058cb58fc4bfd04108965e773488446989154648ea4f43",
            "b0fe92f9c2153f96ab749b47023447733b181ba292b4df91a3639086c9d2a4d7",
            "fbebff33f4f3234ad508fc71083801087cf643e36c70f88d6e81d0936b7fd6ce",
        ),
    ];
    for (words, identity_pub, storage, device0_pub) in vectors {
        let id = parse(words).identity();
        assert_eq!(id.public_key().to_string(), identity_pub, "{words}");
        assert_eq!(hex::encode(id.storage_key().expose_secret()), storage, "{words}");
        let device0 = id.subkey(Subkey::Device(0)).verifying_key();
        assert_eq!(hex::encode(device0.to_bytes()), device0_pub, "{words}");
    }
}

#[test]
fn keys_are_domain_separated() {
    let id = parse(LEGAL).identity();
    let secrets = [
        id.signing_key().to_bytes(),
        *id.storage_key().expose_secret(),
        id.subkey(Subkey::Device(0)).to_bytes(),
        id.subkey(Subkey::Device(1)).to_bytes(),
        id.subkey(Subkey::Device(u32::MAX)).to_bytes(),
    ];
    for i in 0..secrets.len() {
        for j in i + 1..secrets.len() {
            assert_ne!(secrets[i], secrets[j], "keys {i} and {j}");
        }
    }
    // Deterministic.
    assert_eq!(parse(LEGAL).identity().signing_key().to_bytes(), secrets[0]);
}

/// The backup keys (docs/design/restore-servers-from-network.md): HKDF
/// labels `backup/key` and `backup/write/ed25519` under the same PRK, and
/// the tag `blake3::derive_key("mvs/backup-tag/v1", w_pub)`. Pinned: a
/// change would orphan every stored backup. The `enc` values were checked
/// independently with Python's hashlib/hmac (HKDF-SHA256 over the BIP-39
/// seed); `w` and the tag are pinned from this implementation.
#[test]
fn backup_key_vectors() {
    let vectors = [
        (
            LEGAL,
            "810371ac5c987d3459319c0de604d11ee11370d9ddca7eb4d7a79924bff5d1a7",
            "89e05e868605fdd2103bdae2d1364a110e05df154f9751c24d0ee1d415d367a0",
            "3c23422d3c7f52cad586ca73e664bdf21a88147ca679693164ebc847cddf3aa9",
        ),
        (
            ZOO,
            "e8a26da80b860e38cc64b5950787f2b80d5dd7a3708e78392a20a5bfba28df8b",
            "2c81b7e2c68846124afdd41b76463bb00afab95f1b36dcce11b93cebccccf26f",
            "fcdbaff9444abfc70278f8b650251905f2ea3fbee8c64d4b11802cf59d396d5d",
        ),
    ];
    for (words, enc, write_pub, tag) in vectors {
        let keys = parse(words).identity().backup_keys();
        assert_eq!(hex::encode(keys.enc_key()), enc, "{words}");
        assert_eq!(hex::encode(keys.write_key().verifying_key().to_bytes()), write_pub, "{words}");
        assert_eq!(hex::encode(keys.tag()), tag, "{words}");
    }
    assert_eq!(identity::BACKUP_TAG_CONTEXT, "mvs/backup-tag/v1");
}

/// The backup keys are none of the other keys, and the tag can't be had
/// from the identity key: it hashes the separate write key.
#[test]
fn backup_keys_are_separate_from_the_identity() {
    let id = parse(LEGAL).identity();
    let keys = id.backup_keys();
    let secrets = [
        id.signing_key().to_bytes(),
        *id.storage_key().expose_secret(),
        id.subkey(Subkey::Device(0)).to_bytes(),
        *keys.enc_key(),
        keys.write_key().to_bytes(),
    ];
    for i in 0..secrets.len() {
        for j in i + 1..secrets.len() {
            assert_ne!(secrets[i], secrets[j], "keys {i} and {j}");
        }
    }
    let me = id.public_key();
    let w_pub = keys.write_key().verifying_key().to_bytes();
    assert_ne!(w_pub, *me.as_bytes());
    assert_ne!(keys.tag(), *me.as_bytes());
    assert_ne!(keys.tag(), identity::backup_tag(me.as_bytes()), "the tag must not come from the ID");
    assert_eq!(keys.tag(), identity::backup_tag(&w_pub));
    // Deterministic, and another person's words give another tag.
    assert_eq!(parse(LEGAL).identity().backup_keys().tag(), keys.tag());
    assert_ne!(parse(ZOO).identity().backup_keys().tag(), keys.tag());
    // Debug shows the tag only.
    let shown = format!("{keys:?}");
    assert!(!shown.contains(&format!("{:?}", keys.enc_key())), "{shown}");
}

// --- Round trip ------------------------------------------------------------

#[test]
fn generate_and_restore_round_trip() {
    for _ in 0..50 {
        let phrase = RecoveryPhrase::generate();
        let words = phrase.words();
        assert_eq!(words.len(), WORD_COUNT);
        let restored = parse(&words.join(" "));
        assert_eq!(*restored.entropy(), *phrase.entropy());
        assert_eq!(restored.identity().public_key(), phrase.identity().public_key());
        assert_eq!(phrase.word(11), Some(words[11]));
        assert_eq!(phrase.word(12), None);
    }
}

#[test]
fn generated_phrases_differ() {
    let a = RecoveryPhrase::generate();
    let b = RecoveryPhrase::generate();
    assert_ne!(*a.entropy(), *b.entropy());
}

#[test]
fn restore_on_a_new_device() {
    let first = Vault::new(MemoryStore::new());
    let phrase = first.create().unwrap();
    let words = joined(&phrase);

    let second = Vault::new(MemoryStore::new());
    second.restore(&parse(&words)).unwrap();
    assert_eq!(second.state().unwrap(), VaultState::Ready);
    assert_eq!(
        second.open().unwrap().identity().public_key(),
        first.open().unwrap().identity().public_key()
    );
    // Typing all 12 words is the recovery check.
    assert!(second.recovery_check_done(&second.open().unwrap().identity()).unwrap());
}

#[test]
fn parse_is_forgiving_about_formatting() {
    let expected = parse(LEGAL).identity().public_key();
    let inputs = [
        "  LEGAL Winner thank year wave sausage worth useful legal winner thank yellow \n",
        "legal, winner, thank, year, wave, sausage, worth, useful, legal, winner, thank, yellow",
        "1. legal 2. winner 3. thank 4. year 5. wave 6. sausage\n7. worth 8. useful 9. legal 10. winner 11. thank 12. yellow",
        "legal\twinner\nthank\r\nyear wave sausage worth useful legal winner thank yellow",
    ];
    for input in inputs {
        assert_eq!(parse(input).identity().public_key(), expected, "{input:?}");
    }
}

#[test]
fn parse_maps_hebrew_keyboard_layout() {
    // "abandon" and "about" typed with the Hebrew layout on.
    let abandon = "שנשמגםמ";
    let about = "שנםוא";
    assert!(is_word(abandon));
    let input = format!("{} {about}", [abandon; 11].join(" "));
    assert_eq!(*parse(&input).entropy(), [0u8; 16]);
    // Q and W are '/' and '\'' on that layout: "quiz", "wrong".
    assert!(is_word("/וןז"));
    assert!(is_word("'רםמע"));
}

// --- Wrong words and typo hints -------------------------------------------

#[test]
fn unknown_words_get_suggestions() {
    assert_eq!(suggest("abandn"), vec!["abandon"]); // deletion
    assert_eq!(suggest("abandonn")[0], "abandon"); // insertion
    assert_eq!(suggest("abnadon")[0], "abandon"); // transposition
    assert_eq!(suggest("abendon")[0], "abandon"); // substitution
    assert_eq!(suggest("aban"), vec!["abandon"]); // unique 4-letter prefix
    assert_eq!(suggest("Zoo"), Vec::<&str>::new()); // already a word
    assert_eq!(suggest("qqqqqqqq"), Vec::<&str>::new()); // nothing close
    let hints = suggest("wron");
    assert!(hints.contains(&"wrong"));
    assert!(hints.len() <= 4);
}

#[test]
fn every_unknown_word_is_reported_with_its_position() {
    let input = "legal winnr thank year wave sausage wurth useful legal winner thank yelow";
    let err = RecoveryPhrase::parse(input).unwrap_err();
    assert_eq!(err.positions(), vec![1, 6, 11]);
    let PhraseError::UnknownWords(words) = &err else { panic!("{err:?}") };
    assert_eq!(words[0].suggestions[0], "winner");
    assert_eq!(words[1].suggestions[0], "worth");
    assert_eq!(words[2].suggestions[0], "yellow");
    assert_eq!(err.to_string(), "not in the word list: word 2, 7, 12");
}

#[test]
fn wrong_but_valid_word_fails_the_checksum() {
    // Last word changed from "about" to "abandon": all words valid.
    let input = ["abandon"; 12].join(" ");
    assert_eq!(RecoveryPhrase::parse(&input).unwrap_err(), PhraseError::Checksum);
    // Two words swapped.
    let swapped = "winner legal thank year wave sausage worth useful legal winner thank yellow";
    assert_eq!(RecoveryPhrase::parse(swapped).unwrap_err(), PhraseError::Checksum);
}

#[test]
fn only_twelve_words_are_accepted() {
    let words = word_list_cycle();
    for count in 0..=30 {
        if count == WORD_COUNT {
            continue;
        }
        let input = words[..count].join(" ");
        assert_eq!(
            RecoveryPhrase::parse(&input).unwrap_err(),
            PhraseError::WordCount { found: count },
            "{count} words"
        );
    }
    // Valid BIP-39 phrases of other lengths are rejected too.
    let valid_24 = format!("{} art", ["abandon"; 23].join(" "));
    let valid_15 = format!("{} address", ["abandon"; 14].join(" "));
    for input in [valid_24, valid_15] {
        assert!(matches!(RecoveryPhrase::parse(&input), Err(PhraseError::WordCount { .. })));
    }
}

// --- Recovery-word check ---------------------------------------------------

#[test]
fn recovery_check_can_wait_but_not_past_server_creation() {
    let vault = Vault::new(MemoryStore::new());
    let phrase = vault.create().unwrap();
    let id = phrase.identity();
    assert!(!vault.recovery_check_done(&id).unwrap());
    assert_eq!(vault.require_recovery_check(&id), Err(VaultError::RecoveryCheckRequired));

    let positions = check_positions(3);
    assert_eq!(positions.len(), 3);
    assert!(positions.windows(2).all(|w| w[0] < w[1]) && positions[2] < WORD_COUNT);

    let wrong: Vec<(usize, &str)> =
        positions.iter().map(|&p| (p, other_word(phrase.word(p).unwrap()))).collect();
    assert!(!vault.confirm_recovery_check(&phrase, &wrong).unwrap());
    assert!(!vault.confirm_recovery_check(&phrase, &[]).unwrap());
    assert!(!vault.confirm_recovery_check(&phrase, &[(12, "abandon")]).unwrap());
    assert!(vault.require_recovery_check(&id).is_err());

    let right: Vec<(usize, &str)> =
        positions.iter().map(|&p| (p, phrase.word(p).unwrap())).collect();
    assert!(vault.confirm_recovery_check(&phrase, &right).unwrap());
    assert_eq!(vault.require_recovery_check(&id), Ok(()));
}

// --- Vault and app lock ----------------------------------------------------

#[test]
fn create_refuses_to_overwrite() {
    let vault = Vault::new(MemoryStore::new());
    assert_eq!(vault.state().unwrap(), VaultState::Empty);
    assert_eq!(vault.open().unwrap_err(), VaultError::NotFound);
    vault.create().unwrap();
    assert_eq!(vault.create().unwrap_err(), VaultError::AlreadyExists);
    assert_eq!(vault.restore(&parse(ZOO)).unwrap_err(), VaultError::AlreadyExists);
    vault.forget().unwrap();
    assert_eq!(vault.state().unwrap(), VaultState::Empty);
    vault.restore(&parse(ZOO)).unwrap();
}

#[test]
fn app_lock() {
    let store = MemoryStore::new();
    let vault = Vault::new(&store).with_lock_params(CHEAP);
    vault.restore(&parse(LEGAL)).unwrap();
    let expected = parse(LEGAL).identity().public_key();

    assert_eq!(vault.set_app_lock(None, ""), Err(VaultError::EmptyPassphrase));
    vault.set_app_lock(None, "סיסמה ארוכה").unwrap();
    assert_eq!(vault.state().unwrap(), VaultState::Locked);
    assert_eq!(vault.open().unwrap_err(), VaultError::Locked);
    assert_eq!(vault.unlock("wrong").unwrap_err(), VaultError::WrongPassphrase);
    assert_eq!(vault.unlock("סיסמה ארוכה").unwrap().identity().public_key(), expected);

    // The stored blob does not contain the entropy.
    let blob = store.load(ENTRY_SECRET).unwrap().unwrap();
    assert!(!contains(&blob, &[0x7f; 16]));

    // Change the passphrase.
    assert_eq!(vault.set_app_lock(Some("wrong"), "new"), Err(VaultError::WrongPassphrase));
    vault.set_app_lock(Some("סיסמה ארוכה"), "new").unwrap();
    assert_eq!(vault.unlock("סיסמה ארוכה").unwrap_err(), VaultError::WrongPassphrase);
    assert_eq!(vault.unlock("new").unwrap().identity().public_key(), expected);

    // Remove it.
    assert_eq!(vault.remove_app_lock("wrong"), Err(VaultError::WrongPassphrase));
    vault.remove_app_lock("new").unwrap();
    assert_eq!(vault.state().unwrap(), VaultState::Ready);
    assert_eq!(vault.open().unwrap().identity().public_key(), expected);
}

#[test]
fn app_lock_with_default_cost() {
    let vault = Vault::new(MemoryStore::new());
    vault.restore(&parse(ZOO)).unwrap();
    vault.set_app_lock(None, "pass").unwrap();
    assert_eq!(*vault.unlock("pass").unwrap().entropy(), [0xff; 16]);
}

#[test]
fn tampered_app_lock_is_rejected() {
    let store = MemoryStore::new();
    let vault = Vault::new(&store).with_lock_params(CHEAP);
    vault.restore(&parse(LEGAL)).unwrap();
    vault.set_app_lock(None, "pass").unwrap();
    let blob = store.load(ENTRY_SECRET).unwrap().unwrap().to_vec();

    // Flipping any byte after the format header fails: the cost and salt
    // are authenticated, the ciphertext and tag too.
    for i in 2..blob.len() {
        let mut bad = blob.clone();
        bad[i] ^= 1;
        store.save(ENTRY_SECRET, &bad).unwrap();
        assert!(vault.unlock("pass").is_err(), "byte {i}");
    }
    // A huge cost in a corrupted entry is refused before running Argon2.
    let mut huge = blob.clone();
    huge[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
    store.save(ENTRY_SECRET, &huge).unwrap();
    assert_eq!(vault.unlock("pass").unwrap_err(), VaultError::Corrupt);

    store.save(ENTRY_SECRET, &blob[..blob.len() - 1]).unwrap();
    assert_eq!(vault.state().unwrap_err(), VaultError::Corrupt);
    store.save(ENTRY_SECRET, &blob).unwrap();
    assert!(vault.unlock("pass").is_ok());
}

// --- Secrets never reach Debug or Display ---------------------------------

#[test]
fn debug_and_display_never_show_secrets() {
    let store = MemoryStore::new();
    let vault = Vault::new(&store).with_lock_params(CHEAP);
    // A fixed phrase: a random one could contain a word such as "key" or
    // "public" that the Debug output legitimately uses.
    let phrase = RecoveryPhrase::from_entropy(&hex16("9e885d952ad362caeb4efe34a8e91bd2"));
    vault.restore(&phrase).unwrap();
    let id = phrase.identity();
    let device = id.subkey(Subkey::Device(0));

    let mut secrets: Vec<Vec<u8>> = vec![
        phrase.entropy().to_vec(),
        id.signing_key().to_bytes().to_vec(),
        id.storage_key().expose_secret().to_vec(),
        device.to_bytes().to_vec(),
    ];
    secrets.push(store.load(ENTRY_SECRET).unwrap().unwrap().to_vec());

    // A typo in every word, so the error carries suggestions.
    let typos: Vec<String> = phrase.words().iter().map(|w| format!("{w}x")).collect();
    let unknown = RecoveryPhrase::parse(&typos.join(" ")).unwrap_err();
    assert_eq!(unknown.positions().len(), WORD_COUNT);

    let mut outputs = vec![
        format!("{phrase:?}"),
        format!("{phrase:#?}"),
        format!("{id:?}"),
        format!("{id:#?}"),
        format!("{:?}", id.storage_key()),
        format!("{:?}", id.signing_key()),
        format!("{device:?}"),
        format!("{unknown:?}"),
        format!("{unknown:#?}"),
        format!("{unknown}"),
        format!("{store:?}"),
        format!("{vault:?}"),
        format!("{:?}", vault.unlock("x")),
    ];
    vault.set_app_lock(None, "secret passphrase").unwrap();
    outputs.push(format!("{store:?}"));
    outputs.push(format!("{:?}", vault.unlock("secret passphrase")));
    outputs.push(format!("{:?}", vault.unlock("wrong")));
    outputs.push(format!("{}", vault.unlock("wrong").unwrap_err()));

    let words = phrase.words();
    for output in &outputs {
        let lower = output.to_lowercase();
        let tokens: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphabetic()).collect();
        for word in words {
            assert!(!tokens.contains(&word), "word {word:?} in {output:?}");
        }
        assert!(!output.contains("secret passphrase"), "{output:?}");
        for secret in &secrets {
            for form in [hex::encode(secret), format!("{secret:?}")] {
                // Any 8 bytes of a secret in a row, as hex or as a byte list.
                let window = if form.starts_with('[') { 24 } else { 16 };
                for start in (0..form.len().saturating_sub(window)).step_by(2) {
                    let piece = &form[start..start + window];
                    assert!(!output.contains(piece), "secret bytes {piece:?} in {output:?}");
                }
            }
        }
    }
    // The public key may be shown, and is.
    assert!(format!("{id:?}").contains(&format!("{:?}", id.public_key())));
}

// --- Keychain names and the recovery-check flag -----------------------------

/// Changing any of these makes existing identities invisible to the app.
#[test]
fn keychain_names_are_frozen() {
    assert_eq!(KEYCHAIN_SERVICE, "io.github.moranmoa.mvs.identity.v1");
    assert_eq!(ENTRY_SECRET, "identity-secret-v1");
    assert_eq!(ENTRY_RECOVERY_CHECK, "identity-recovery-check-v1");

    let store = MemoryStore::new();
    let vault = Vault::new(&store);
    vault.restore(&parse(LEGAL)).unwrap();
    assert_eq!(
        format!("{store:?}"),
        r#"MemoryStore { names: ["identity-recovery-check-v1", "identity-secret-v1"] }"#
    );
}

/// A store whose `delete` always fails, like a locked keychain.
struct DeleteFails(MemoryStore);

impl SecretStore for DeleteFails {
    fn load(&self, name: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, StoreError> {
        self.0.load(name)
    }
    fn save(&self, name: &str, secret: &[u8]) -> Result<(), StoreError> {
        self.0.save(name, secret)
    }
    fn delete(&self, _name: &str) -> Result<(), StoreError> {
        Err(StoreError("delete refused".into()))
    }
}

#[test]
fn create_stores_nothing_if_the_old_flag_cannot_be_cleared() {
    // A previous identity passed the check, then its secret went away but
    // its flag stayed.
    let store = DeleteFails(MemoryStore::new());
    Vault::new(&store.0).restore(&parse(ZOO)).unwrap();
    store.0.delete(ENTRY_SECRET).unwrap();

    let vault = Vault::new(&store);
    assert!(matches!(vault.create(), Err(VaultError::Store(_))));
    assert_eq!(vault.state().unwrap(), VaultState::Empty);
}

#[test]
fn a_leftover_flag_does_not_count_for_another_identity() {
    let store = MemoryStore::new();
    let vault = Vault::new(&store);
    vault.restore(&parse(ZOO)).unwrap();
    assert!(vault.recovery_check_done(&parse(ZOO).identity()).unwrap());
    store.delete(ENTRY_SECRET).unwrap(); // the flag stays behind

    let phrase = vault.create().unwrap();
    let id = phrase.identity();
    assert!(!vault.recovery_check_done(&id).unwrap());
    assert_eq!(vault.require_recovery_check(&id), Err(VaultError::RecoveryCheckRequired));

    // Even a flag that survives is bound to the identity it was passed for.
    store.save(ENTRY_RECOVERY_CHECK, &[1]).unwrap();
    assert!(!vault.recovery_check_done(&id).unwrap());
    let answers: Vec<(usize, &str)> = (0..3).map(|p| (p, phrase.word(p).unwrap())).collect();
    assert!(vault.confirm_recovery_check(&phrase, &answers).unwrap());
    assert!(vault.recovery_check_done(&id).unwrap());
    assert!(!vault.recovery_check_done(&parse(ZOO).identity()).unwrap());
}

// --- App lock cost and passphrase -------------------------------------------

#[test]
fn app_lock_cost_has_a_floor_and_a_ceiling() {
    let weak = [
        LockParams { m_kib: 8, t: 1, p: 1 },
        LockParams { m_kib: CHEAP.m_kib - 1, ..CHEAP },
        LockParams { t: 1, ..CHEAP },
        LockParams { m_kib: 256 * 1024 + 1, ..CHEAP },
        LockParams { t: 11, ..CHEAP },
        LockParams { p: 9, ..CHEAP },
    ];
    for params in weak {
        let vault = Vault::new(MemoryStore::new()).with_lock_params(params);
        vault.restore(&parse(LEGAL)).unwrap();
        assert_eq!(vault.set_app_lock(None, "pass"), Err(VaultError::WeakLockParams), "{params:?}");
        assert_eq!(vault.state().unwrap(), VaultState::Ready);
    }
    assert!(LockParams::default().m_kib >= CHEAP.m_kib && LockParams::default().t >= CHEAP.t);
}

#[test]
fn passphrase_is_compared_after_nfc() {
    let vault = Vault::new(MemoryStore::new()).with_lock_params(CHEAP);
    vault.restore(&parse(LEGAL)).unwrap();
    // "é" precomposed vs. e + combining acute; Hebrew shin with its marks in
    // either order (shin dot U+05C1, qamats U+05B8).
    vault.set_app_lock(None, "caf\u{e9} \u{5e9}\u{5c1}\u{5b8}").unwrap();
    assert!(vault.unlock("cafe\u{301} \u{5e9}\u{5b8}\u{5c1}").is_ok());
    assert_eq!(vault.unlock("cafe \u{5e9}").unwrap_err(), VaultError::WrongPassphrase);
}

#[test]
fn whitespace_passphrase_is_empty() {
    let vault = Vault::new(MemoryStore::new()).with_lock_params(CHEAP);
    vault.restore(&parse(LEGAL)).unwrap();
    for passphrase in ["", " ", "\t\n", "\u{3000}"] {
        assert_eq!(vault.set_app_lock(None, passphrase), Err(VaultError::EmptyPassphrase));
    }
    // Spaces inside or around a real passphrase are kept, not trimmed.
    vault.set_app_lock(None, " a b ").unwrap();
    assert_eq!(vault.unlock("a b").unwrap_err(), VaultError::WrongPassphrase);
    assert!(vault.unlock(" a b ").is_ok());
}

// --- Helpers ---------------------------------------------------------------

fn hex16(s: &str) -> [u8; 16] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn word_list_cycle() -> Vec<&'static str> {
    identity::word_list().iter().copied().step_by(67).take(30).collect()
}

fn other_word(word: &str) -> &'static str {
    if word == "zoo" {
        "abandon"
    } else {
        "zoo"
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
