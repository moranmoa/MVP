use identity::{
    check_positions, is_word, suggest, LockParams, MemoryStore, PhraseError, RecoveryPhrase,
    SecretStore, Subkey, Vault, VaultError, VaultState, WORD_COUNT,
};

const ABANDON: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";
const ZOO: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

/// Cheap Argon2id cost so the tests stay fast.
const CHEAP: LockParams = LockParams { m_kib: 256, t: 1, p: 1 };

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
            "ac20fde54abad8a65e772c0503b171075817b7aa33187cb5a028d7776e8897c3",
            "27b8566b81e40bb9105c816c50deb3fc0ef7fd1d0be73a9ca76eee0e3c97b776",
            "ed8ba6b74776e59c2a728c3a0b5839c99e22d36782a6a40b2a63b3eeccb3086f",
        ),
        (
            LEGAL,
            "26deff96806ea1c93e9764bd4cc26a90e3d790b3ca3d3a9263f65b09e289fd45",
            "d71db4b555fe4535cf9b3af4073c3f98ddd758e878fe3af3af9e2ee40c6e73bb",
            "5918170040e1803c78ab78d409650774d8b61686c787c747503361039228a609",
        ),
        (
            ZOO,
            "f331d003edf20c15be53f87b16cd3de5b705b4ba746208fd9ef5039807b1b92c",
            "634fe4e0769cc341cbc4cbb238fe23c3fb19f8a74de24b95250b88b024c1c03d",
            "28493e23ce9b9c5e985e290259ce0e7f18971293583958042b34a96ba5feab57",
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
    assert!(second.recovery_check_done().unwrap());
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
    assert!(!vault.recovery_check_done().unwrap());
    assert_eq!(vault.require_recovery_check(), Err(VaultError::RecoveryCheckRequired));

    let positions = check_positions(3);
    assert_eq!(positions.len(), 3);
    assert!(positions.windows(2).all(|w| w[0] < w[1]) && positions[2] < WORD_COUNT);

    let wrong: Vec<(usize, &str)> =
        positions.iter().map(|&p| (p, other_word(phrase.word(p).unwrap()))).collect();
    assert!(!vault.confirm_recovery_check(&phrase, &wrong).unwrap());
    assert!(!vault.confirm_recovery_check(&phrase, &[]).unwrap());
    assert!(!vault.confirm_recovery_check(&phrase, &[(12, "abandon")]).unwrap());
    assert!(vault.require_recovery_check().is_err());

    let right: Vec<(usize, &str)> =
        positions.iter().map(|&p| (p, phrase.word(p).unwrap())).collect();
    assert!(vault.confirm_recovery_check(&phrase, &right).unwrap());
    assert_eq!(vault.require_recovery_check(), Ok(()));
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
    let blob = store.load("identity").unwrap().unwrap();
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
    let blob = store.load("identity").unwrap().unwrap().to_vec();

    // Flipping any byte after the format header fails: the cost and salt
    // are authenticated, the ciphertext and tag too.
    for i in 2..blob.len() {
        let mut bad = blob.clone();
        bad[i] ^= 1;
        store.save("identity", &bad).unwrap();
        assert!(vault.unlock("pass").is_err(), "byte {i}");
    }
    // A huge cost in a corrupted entry is refused before running Argon2.
    let mut huge = blob.clone();
    huge[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
    store.save("identity", &huge).unwrap();
    assert_eq!(vault.unlock("pass").unwrap_err(), VaultError::Corrupt);

    store.save("identity", &blob[..blob.len() - 1]).unwrap();
    assert_eq!(vault.state().unwrap_err(), VaultError::Corrupt);
    store.save("identity", &blob).unwrap();
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
    secrets.push(store.load("identity").unwrap().unwrap().to_vec());

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
