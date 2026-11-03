# updater: threat model and audit notes

## Who we defend against

| Attacker | Can | Cannot (by design) |
| --- | --- | --- |
| Network / malicious peer | Send any bytes, replay old manifests, withhold new ones, spoof NTP (wind the clock back) | Sign with root keys |
| Holder of fewer than *threshold* root keys | Sign with that key | Get a manifest or root rotation accepted |
| Local malware running as the user | Everything the user can do | Out of scope. It can already replace the app. We only make sure a *damaged* state file fails safe |

## Rules a client enforces

1. The root chain is applied (`TrustRoot::update_chain`). Each rotation must be signed by a threshold of the old keys **and** of the new keys. Invalid entries from peers are skipped, never fatal, and none is dropped by position; two different valid rotations to one version stop the walk and raise a compromise alarm (`RootChain::fork_at`).
2. A manifest needs a threshold of distinct trusted signatures. At most 8 signature entries are allowed, the manifest is capped at 64 KiB, and the signatures use a manifest-specific domain prefix.
3. The body must be valid:
   - format 1
   - channel and platform names in `[a-z0-9_-]{1,32}`
   - lowercase SHA-256 hashes
   - a validity period of at most 30 days (QA-14)
   - it must not revoke its own version
4. The manifest must not be expired, measured against `max(local clock, highest signed issued_at seen, build time)`, and its `issued_at` must not be more than one day ahead of the local clock (QA-15).
5. Ordered by `(version, issued_at)`, it must not be older than the newest manifest already accepted.
6. The version must not be revoked by any manifest seen so far, and must not be lower than the installed version.
7. The artifact is stored through `store_artifact` (hashed while it is written; only a match gets the final name). It is then read through the handle from `open_verified`, both to install it and to serve it to peers.
8. A version is staged only for this install's platform, only if it is newer, not revoked and has not failed before. It gets 1 to 10 trial boots, then rolls back to the version that was running before.

## Audit (Nov 2026): what was found and fixed

| # | Issue | Attack | Fix | Test |
| --- | --- | --- | --- | --- |
| 1 | Expiry checked only against the local clock | Spoof NTP back in time, then replay a manifest that really expired: freezes the user on an old release | `ClientPolicy` keeps `time_floor` = highest signed `issued_at`; `accept` rejects `expires_at <= floor` | `rolled_back_clock_cannot_revive_an_expired_manifest` |
| 2 | Same-version replay | Replay the first 1.2.0 manifest after a re-issue added revocations or extended expiry | Ordering by `(version, issued_at)` | `older_issue_of_the_same_version_is_a_replay` |
| 3 | Deleted policy file reset everything | Delete or corrupt the file, then replay older but unexpired manifests | `ClientPolicy::restore` never goes below the binary's `Baseline` (installed version, build time, revocations baked in at build time) | `deleted_state_falls_back_to_the_binary_baseline`, `tampered_state_cannot_go_below_the_baseline`, `corrupt_state_is_reset_not_trusted` |
| 4 | `InstallState` deserialised without checks | A damaged file sets `max_attempts` to 4 billion (a broken build never rolls back), or adds a "trial" below `current` (a downgrade) | `serde(try_from)` validation; `InstallState::restore` resets invalid or foreign state; attempts clamped to 1..=10 | `tampered_state_is_refused_on_load`, `restore_keeps_valid_state_and_resets_missing_or_foreign_state` |
| 5 | `VerifiedArtifact` had public fields | Any code could build one and stage an unverified file | Private fields; only this crate constructs it | (compile-time) |
| 6 | TOCTOU between verify and install | Swap the file after `verify_artifact` and before the installer reopens it by path | `store_artifact` (temp file → hash while writing → rename) plus `StoredArtifact::open_verified` (re-hash and rewind one handle) | `swapped_file_after_verification_is_caught`, `bad_download_leaves_nothing_behind` |
| 7 | Platform and channel names unrestricted | `"../../x"` as a platform, once names become file names | `is_valid_name` in body validation | `path_like_names_are_rejected` |
| 8 | Staging ignored the platform | A Windows artifact staged on Linux (bricks the trial) | `InstallState` holds its platform; `stage` checks it | `artifact_for_another_platform_cannot_be_staged` |
| 9 | Unbounded signature list | 300 entries claiming a trusted key means 300 Ed25519 verifications per manifest | At most 8 entries; repeats of an already-counted key are skipped before verifying | `too_many_signature_entries_are_refused` |
| 10 | No cap when reading from a stream | An endless manifest exhausts memory | `SignedManifest::from_reader` reads at most 64 KiB + 1 | `oversized_input_is_refused_before_parsing` |
| 11 | No limit on the validity period | A manifest valid for 10 years defeats freeze protection | At most 30 days (was 90; QA-14 asked for 30) | `validity_period_is_capped`, `qa14_manifest_that_never_expires_is_refused` |
| 12 | Blind co-signing | Hand the second key holder a manifest for a different version than discussed | `sign-manifest --manifest` prints the body and requires `--expect-version`; same for `sign-root --expect-root-version` | `co_signer_must_name_the_version_they_sign` |
| 13 | Wrong clock on the signing machine | `issued_at` far in the future pushes every client's time floor ahead, so later manifests look expired | The CLI refuses `issued_at > now + 1h` and already-expired bodies | `refuses_to_sign_a_body_from_the_future` |
| 14 | No way to rotate keys | Moving from 1 key to 2-of-3 meant either trusting both forever or reinstalling | `SignedRoot` + `TrustRoot::rotate` / `update_chain`; the old keys stop counting once applied | `root.rs` tests, `one_key_to_two_of_three_rotation_flow` |
| 15 | Secret key file kept inherited ACLs on Windows | Other accounts and admins on the machine can read the root key; one that opened the empty file before the ACL change keeps its handle | The file is created with **no sharing** (`share_mode(0)`) and `WRITE_DAC`; through that same handle its whole DACL is replaced (`SetSecurityInfo`) by a protected one with a single entry for the current user's SID, so no inherited or token-default entries (SYSTEM, Administrators, logon session) survive; `icacls` then checks it; on any failure the file is deleted before a key is written | `secret_key_acl_is_owner_only_on_windows` (Windows CI job); checked by hand in a normal folder and in one with explicit default ACEs |
| 16 | Root chain could be blocked by peers (reviews of #8, R4) | Any peer adds a junk entry, a forged same-version rotation, a padded copy, or simply many entries ahead of the real one: the client stays on the retired root and rejects every manifest signed with the new keys | `update_chain` never aborts and never drops entries by position. Each distinct entry is parsed once; signatures are checked only for entries for the next version that name a current-root key, and each trusted key gets one verification per envelope, so the work grows with the number of such entries, not with what they contain | `junk_entry_does_not_block_rotation`, `forged_same_version_entry_listed_first_does_not_block_rotation`, `padded_copy_does_not_block_the_clean_one`, `a_thousand_junk_entries_before_the_real_one_do_not_block_rotation`, `a_thousand_forged_v2_entries_do_not_block_rotation_and_stay_cheap`, `identical_entries_are_checked_once` |
| 17 | Two valid rotations to one version were resolved silently | A leaked current-root key forks clients onto the thief's root | Detected and reported as `RootChain::fork_at`; neither is applied; the CLI fails with a compromise message | `two_valid_rotations_to_one_version_raise_the_alarm` |
| 18 | `issued_at` far in the future was accepted (QA-15) | A body dated a year ahead pushes the time floor ahead, so every honest manifest then looks expired | Refused beyond `MAX_CLOCK_SKEW_SECS` (1 day) | `qa15_manifest_issued_far_in_the_future_is_refused` |
| 19 | A state reset forgot failed versions (review of #8) | A corrupt file gives a version that already failed another trial | `InstallState::restore` keeps the readable `failed` list | `reset_keeps_versions_that_already_failed` |
| 20 | Root version overflow | `rotate` at `u64::MAX` panicked in debug builds | `checked_add` | `version_overflow_is_an_error_not_a_panic` |

Checked and found fine: deeply nested JSON. serde_json's recursion limit refuses it without overflowing the stack (`deeply_nested_json_is_refused_not_a_stack_overflow`).

## Design-level notes (not fixable in this crate alone)

- **Withholding.** A network attacker can always stop delivering new manifests. After the last one expires, `ClientPolicy::freshness` reports `Stale`. The app should then tell the user and try the project seeder and, later, HTTPS. Recommendation: manifests valid for 14 days, re-issued weekly.
- **Where the trust root lives.** The client persists the latest root it reached and falls back to the root embedded in its binary. **Every release must embed the latest root**, so deleting state only drops the client back to that release's root, and fresh installs never start from an old one.
- **Rotations do not expire.** If an *old* root key leaks after the rotation, it still can't sign anything for clients that have applied the rotation. Clients that never applied it stay exposed until they do, because the chain arrives with every manifest. Destroy retired keys after rotating. In the single-key closed alpha one leaked key is enough to sign a competing rotation; `fork_at` turns that into an alarm instead of a silent fork.
- **Clock far behind.** A device whose clock is more than a day behind real time refuses fresh manifests (rule 4) until the clock is fixed, like TLS does. The app should say "check your clock".
- **`ClientPolicy` is not validated on load** the way `InstallState` is: its fields can only be checked against the current time, and the point of the time floor is not to trust the local clock. A damaged file is at worst a local denial of service, fixed by deleting it; `restore` never goes below the baseline.
- **Version numbers across channels.** Switching channel resets the replay floor to the installed version but keeps revocations and the time floor. Beta and stable must share one version sequence.
- **Local malware** that runs as the user can edit state files and the install directory. That is outside this model. The OS account boundary and code signing (planned before public launch) are the controls.
- **Delta updates and staged rollout** (stage 2) must keep rule 7: the patched result is hashed against the manifest before staging.

## Storing root keys

The CLI keeps keys as hex in a file restricted to your account. On Unix that's mode 0600; on Windows the file gets a single ACL entry for your SID and nothing inherited. That is a minimum, not the goal:

- **Closed alpha (1 key):** keep the key on an **encrypted volume** (BitLocker To Go or VeraCrypt) on a removable drive. Mount it only while signing, never on a cloud-synced folder, and keep a second encrypted copy in another place.
- **Open alpha (2-of-3):** each of the three keys on its own **hardware token** (e.g. a YubiKey 5 with Ed25519 on firmware 5.7+, or an OpenPGP card), each stored in a separate place. Signing with a token is not implemented in this CLI yet; until it is, use one encrypted removable drive per key, never two keys on the same drive.
- Sign on a machine without network access if you can, and check the printed body (version, hashes, expiry) before every signature.
- `keygen` refuses to write inside any git checkout, never overwrites an existing key, and `.gitignore` excludes `*.key`.
