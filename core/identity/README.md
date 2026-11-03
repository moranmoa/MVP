# identity

A user's identity, made from 12 recovery words ("מילות שחזור").

## Derivation tree

```
128-bit entropy (OS RNG)
 └─ 12 words: BIP-39 English, 11 bits each, last 4 bits = SHA-256 checksum
     └─ seed = PBKDF2-HMAC-SHA512(words, "mnemonic", 2048 rounds) — 64 bytes
         (standard BIP-39 seed, empty BIP-39 passphrase)
         └─ PRK = HKDF-SHA256-Extract(salt = "mvs/identity/v1", seed)
             ├─ HKDF-Expand(PRK, "identity/ed25519", 32)  → Ed25519 identity key (the user's ID)
             ├─ HKDF-Expand(PRK, "local-storage/key", 32) → local storage encryption key
             └─ HKDF-Expand(PRK, "device/ed25519/" ‖ u32be(n), 32) → device key n (Subkey::Device)
```

- Every step is deterministic: the same words give the same keys on every
  device. Vectors computed independently in Python are in
  `tests/identity.rs` (`derivation_vectors`).
- Each key has its own HKDF label, so knowing one derived key tells nothing
  about another. New purposes get a new `Subkey` variant and a new label;
  existing labels never change.
- **Frozen for the alpha** (product name MVS, "message voice stream"):
  the HKDF salt `mvs/identity/v1` and the labels above, the keychain
  service `io.github.moranmoa.mvs.identity.v1`, the entry names
  `identity-secret-v1` and `identity-recovery-check-v1`, and the check-tag
  domain `mvs/identity/recovery-check/v1`. Changing the salt or a label
  changes every user's identity; changing a keychain name makes existing
  identities invisible (the app would offer sign-up). Tests pin all of them
  (`derivation_vectors`, `keychain_names_are_frozen`).
- Device keys are for "link another device" (stage 3): the identity key
  signs a certificate for the device key, so a device can be revoked without
  touching the identity. A device may also use a random key instead; only
  the certificate matters to others.
- The storage key is derived, not random, so a backup encrypted with it can
  be read after restoring from the words (the spec keeps the birth date in
  the identity backup).

## On this device

`Vault` keeps only the 16-byte entropy, in a `SecretStore`:

| Store | Where |
| --- | --- |
| `OsKeychain` | Windows Credential Manager, macOS Keychain, Linux Secret Service (GNOME Keyring / KWallet). Feature `os-keychain`, on by default. Always under service `KEYCHAIN_SERVICE`. On Windows: target name `<service>/<entry>` set explicitly (so no other app's `user.service` pair can collide with it) and persistence `Local`, so the credential never roams with an enterprise or roaming profile. |
| `MemoryStore` | In memory, for tests. |

Optional app lock: the entropy is sealed with XChaCha20-Poly1305 under a key
from Argon2id(passphrase, random 16-byte salt). Default cost is RFC 9106's
second recommendation (64 MiB, 3 passes, 4 lanes); the cost is stored with
the entry and authenticated, so it can be raised later. A new lock needs at
least OWASP's minimum (`LockParams::MIN`: 19 MiB, 2 passes); a stored cost
above 256 MiB, 10 passes or 8 lanes is refused before Argon2 runs. The
passphrase is NFC-normalised first, so the same Hebrew text with niqqud or
a geresh unlocks on any OS. A passphrase of only whitespace is refused.

The recovery-word check (`confirm_recovery_check`) can be postponed, but
`require_recovery_check(&identity)` fails with `RecoveryCheckRequired` (the
UI's `recovery-check-required`) until it passes. Call it before creating a
server. Restoring from all 12 words counts as passing it. The flag stores
the first 16 bytes of SHA-256(`mvs/identity/recovery-check/v1` ‖ public
key), so a flag left over from another identity never counts, and `create`
clears the old flag before it stores anything.

## Restore screen

`RecoveryPhrase::parse` accepts any case, commas, line breaks and numbering
("1. abandon 2. ..."). It reports, in this order: not exactly 12 words (15,
18, 21 and 24-word BIP-39 phrases are rejected too); every word not in the
list, with its position and up to 4 closest words (same 4-letter prefix,
autocomplete, or 1–2 typos, preferring the same first letters); or a
checksum mismatch (all words valid but one is wrong or two are swapped).
`suggest(word)` gives the same hints live, per field.

Words typed with the Hebrew keyboard layout on by mistake are mapped to the
letter on the same key: "שנשמגםמ" is "abandon".

The list is standard BIP-39, so someone could paste a crypto wallet's 12
words into restore. That can't be detected; the restore screen should say
"never use words from a crypto wallet".

## Why not a Hebrew word list

Not advised:

- BIP-39 has no Hebrew list. Ours would be non-standard: no other tool
  could read the words, and we would have to design and freeze 2048 words
  ourselves (unique 4-letter prefixes, no near-duplicates).
- Hebrew spelling is not fixed (כתיב מלא/חסר: "שמיכה"/"שמכה"), final
  letters (ם/מ) and optional niqqud: the same word can be typed several
  ways, and the checksum then fails with no good hint.
- Mixed RTL/LTR text on the restore screen, and users who move to an
  English-only device, make it worse.

Better: keep the English list (the words are a code, not prose), and help
Hebrew speakers with the keyboard-layout mapping above and the typo hints.
If wanted, the UI can show a Hebrew gloss next to each word, as a display
aid only.

## Security properties

- 128 bits of entropy. The words are the identity: whoever has them
  controls it; lose them and it is gone (spec).
- Secrets are zeroized on drop: `RecoveryPhrase` (via bip39's `zeroize`),
  the seed, PRK, derived keys (`Zeroizing`, ed25519-dalek's `SigningKey`),
  the Argon2 output, decrypted plaintext and keychain reads. Best effort:
  copies made inside dependencies (PBKDF2/HMAC state, the `[u8; 64]` that
  bip39 returns before we wrap it) and strings owned by the caller (the
  typed words and passphrase) are outside our control. The UI should clear
  its own input fields.
- No type prints a secret: `Debug` on `RecoveryPhrase`, `StorageKey`,
  `Identity` (public key only), `PhraseError` (positions and counts, never
  the typed word or the suggestions), `MemoryStore` (entry names only) and
  `VaultError` is redacted. The crate does not log. Test:
  `debug_and_display_never_show_secrets`.
- `Identity` keeps the HKDF PRK (zeroized on drop) so subkeys can be derived
  later. It is no more sensitive than the identity key held next to it.
- Without the app lock, the entropy is as safe as the OS keychain: other
  software running as the same user may be able to read it. The app lock
  protects it at rest against that, at the cost of a passphrase that, if
  forgotten, needs the 12 words.
- The OS keychain is only reached through `SecretStore`; tests never touch
  it.
- **No secret store, no identity.** On Linux without a running Secret
  Service (headless, minimal window managers), and on Android and iOS
  (keyring has no store there), `OsKeychain` returns an error and nothing
  is stored; there is deliberately no silent fallback. Open design item: a
  fallback such as an encrypted file with the app lock made mandatory.

## Dependencies and licences

| Crate | Licence | For |
| --- | --- | --- |
| bip39 2 | CC0-1.0 | Word list, checksum, seed |
| ed25519-dalek 2 | BSD-3-Clause | Identity and device keys |
| hkdf, sha2, argon2, chacha20poly1305 | MIT OR Apache-2.0 | KDF, app lock |
| zeroize, rand_core | MIT OR Apache-2.0 | |
| strsim | MIT | Typo distance |
| keyring 4, keyring-core 1 | MIT OR Apache-2.0 | OS keychain |
| unicode-normalization | MIT OR Apache-2.0 | NFC for the app lock passphrase |

keyring's default stores are used. On Linux it talks to the Secret Service
over zbus (pure Rust), so no libdbus (AFL/GPL) is linked. `cargo-deny check
licenses sources bans` passes with `core/deny.toml`.

Known advisories (non-blocking CI job): zbus 5.19 brings three RustSec
"unmaintained" notices, RUSTSEC-2026-0370 (`zvariant`), -0371
(`zbus_names`) and -0372 (`zvariant_derive`). Not vulnerabilities; revisit
when keyring's zbus store moves on. Switching to the `dbus-secret-service`
store to avoid them is not an option: it links libdbus.

## Tests

From `core/`:

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm cargo test --release -p identity
```
