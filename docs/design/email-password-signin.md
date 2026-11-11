# Sign in with email and password (with email recovery)

- **Status:** proposal, by dev-identity (2026-11-11), for review by the Security Lead, Tech Lead and PO. Build after the Nov 17 reset, aiming at 0.4.4.
- **Builds on:** `restore-servers-from-network.md` §10 (approved direction) and the backup store (#317, #339).
- **The rule that stays:** the 12 words are always the root. Email and password are an optional, second way to reach the same words.

## 0. What already holds (from §10 and the leads)
- **Nothing to look up:** the email is never stored or sent in clear to a holder. It becomes a **blinded handle** through an OPRF.
- **The password is hardened online:** OPAQUE-style, through a **threshold OPRF across at least 2 bootstraps**, so a stolen blob can't be guessed offline.
- **Gated (Security Lead):** the password feature is **off while there's only one bootstrap**. So 0.4.4 needs the second bootstrap (GAP-9 / #220) live, or it ships behind a flag.
- **The envelope wraps the seed**, and the seed opens everything else: identity, backup, servers. The normal restore follows (#290, #317).

## 1. Sign-in on a new device
1. **The email is blinded:** the client normalises the email (lower-case, NFC, trimmed) and blinds it.
   - The OPRF servers (≥ 2 bootstraps, threshold t-of-n) evaluate it without seeing it, each rate-limited per IP.
   - The client unblinds `E`. Then `handle = derive_key("mvs/email-handle/v1", E)` and `tag_e = derive_key("mvs/email-tag/v1", handle)`.
2. **The password is hardened the same way:** `P = OPRF(password, salt = handle)` across the same servers, then `k_pw = Argon2id(P, salt = handle, m = 256 MiB, t = 3, p = 1)`, tuned to about 1 s on a mid laptop.
3. **The envelope:** the client fetches the **password envelope** from the backup holders: dev-sync's `/mvs/backup/1` stores plus the relay mailbox, the same paths as the server backup.
   - The envelope is `{v: 1, tag_e, w_pub, version, nonce, AEAD(k_pw, seed)}`, signed by `w`.
   - `w` is the backup write key from the seed (#292). Only the account can write or replace its envelope, and the tag is checked against `w`, as for backups.
4. **Opening:** the client opens it, gets the seed, and from there it's the normal restore: identity, the backup of servers, rejoin.
   - A wrong password fails the AEAD.
   - Holders and OPRF servers return a constant "not found" for unknown handles.
- **Writing:** "Add email and password" (Settings) runs steps 1–2 and writes the envelope with `w`. Changing the password replaces it (`version + 1`), and removing it writes a tombstone.

## 2. Email recovery ("forgot password")
- **The hard part:** recovery needs mail, and mail needs a sender, the one central piece. Here is the least-trust shape:
- **The mailer:** a small service run by devops on a bootstrap. It holds an SMTP relay account, a per-account **recovery share** `r`, and nothing else.
- **The recovery envelope** is a second envelope under `tag_e`: `AEAD(k_rec, seed)`, with `k_rec = derive_key("mvs/email-recovery/v1", r ‖ E)`.
  - `r` is random, made by the client when the user turns email recovery on. It's sent to the mailer **encrypted to the mailer's public key**, and stored there keyed by `tag_e`.
- **The flow:**
  1. The user asks for a reset, entering the email. The client computes `tag_e` (OPRF) and asks the mailer.
  2. The mailer sends a one-time code to the address. It needs the address to send at all; the client passes it, and the mailer doesn't keep it.
  3. The user enters the code. The mailer checks it and **starts a 72-hour hold**.
  4. Meanwhile, the client posts a signed **"recovery pending" notice** (signed by `w`) where my other devices see it, through the backup store. Any signed-in device can **cancel**.
  5. After 72 hours with no cancel, the mailer releases `r`. The client derives `k_rec`, opens the recovery envelope and gets the seed. The user sets a new password, which writes a new password envelope.
- **What the mailer learns:**
  - the email address, at send time only;
  - `tag_e`;
  - when resets are asked for;
  - `r`, which on its own opens nothing.
- **What the mailer can't do:**
  - it can't open anything without also knowing `E`; it saw the email once, and OPRF holders run separately;
  - it can't read the backup;
  - it can't change the account.
- **Plainly, the residual risk:** a mailer operator that keeps the email it sends to, and also colludes with the OPRF holders, **can** recover the seed after the 72-hour hold. The same goes for **anyone who controls the inbox**. Email recovery means email can take over the account, slowed by 72 hours and visible to my other devices.
  - So it's **off by default**, and turning it on shows that sentence.
  - A user with the 12 words never needs it.
- **Abuse limits:**
  - 3 code requests per `tag_e` per day and 10 per IP per day;
  - the code: 6 digits, 10 minutes, 5 attempts, then a 24-hour lock;
  - one pending reset per account;
  - an SMTP send quota per day.
  - The mailer answers the same whether or not the account exists (no enumeration).
- **If the mailer is down:** sign-in with email and password still works (no mailer involved), and so do the words. Only "forgot password" waits.

## 3. Threat model (for the Security Lead)
| Threat | Defence |
|---|---|
| **Offline guessing** from a stolen envelope | The key needs the threshold OPRF on `password` (online, rate-limited, t-of-n), then Argon2id. With fewer than t servers compromised, guesses go online and are throttled. Off with one bootstrap |
| **Email enumeration** (does x@y have an account?) | The handle is OPRF-blinded; holders, OPRF servers and the mailer answer the same; per-IP limits |
| **Takeover by whoever controls the email** | Recovery is opt-in, with a 72-hour hold, a notice to my devices and a cancel. Without opt-in, the email alone gets nothing |
| **A holder serving an old envelope** after a password change | Signed versions, newest valid wins (as for backups); the old password still opens an old copy until holders refresh. Accepted, documented |
| **Phishing a password** | Out of scope; a password is weaker than words by nature. The words stay the root, and the UI says so |
| **Age and 18+** | Email sign-in restores the same identity. The birth date isn't in the envelope: restore asks again, and the 30-day rule (#306) applies to a changed date. No age shortcut |
| **Seed exposure on a weak password** | Strength check (zxcvbn ≥ 3) before enabling. A breached-password check, k-anonymity style, is an option for later |

## 4. UX touchpoints (dev-ui and PO)
- **Onboarding:** after the words, an optional step, "Also sign in with email and password". It can be skipped and done later.
- **Settings → Account:**
  - add, change or remove email and password;
  - turn "email recovery" on or off, with the takeover sentence;
  - see and cancel a pending recovery.
- **Sign-in on a new device:** the first screen offers "Restore with words" or "Sign in with email". There's a "Forgot password" link, available when recovery is on. The 72-hour hold is shown with a countdown.
- **Copy:** never "your account is stored with us". It's "an encrypted copy of your keys is kept in the network; only your password opens it".

## 5. Build split (after Nov 17)
| Piece | Owner |
|---|---|
| Normalisation, OPRF client, Argon2id, both envelopes (format, sign, seal, open), api (`addEmailPassword`, `signInWithEmail`, `requestRecovery`, `completeRecovery`, `cancelRecovery`), notices, tests | dev-identity (core/identity, core/api) |
| Holders for envelopes and notices (the `/mvs/backup/1` record type), fetch by tag | dev-sync |
| Threshold OPRF service on ≥ 2 bootstraps; the mailer (SMTP, codes, 72-hour hold, limits, the encrypted `r` store); relay mailbox entries | devops |
| Screens and copy (§4) | dev-ui (PO for copy) |
| Protocol, parameters, mailer trust | Security Lead review |

## 6. Questions for the reviewers
1. **Ship gating:** is 0.4.4 gated on two bootstraps being live, or does it go behind a flag until then?
2. **Mailer trust:** run by us on a bootstrap, or a third-party transactional mailer (it sees addresses either way)?
3. **Recovery:** is the 72-hour hold right, and should recovery default to off?
4. **Argon2id parameters:** about 1 s on a mid laptop; what's the phone budget later?

## 7. PO conditions (accepted, for the build)
1. **Opt-in only:** email and password, and email recovery on top.
2. **Warnings when enabling:** a plain sentence about the 72-hour takeover, and a single-device warning (with one device there's nobody to cancel a recovery).
3. **Same release:** the invite pack (it says "no email" today) and the privacy note are updated.
4. **Deleting from the mailer:** removing the email or password deletes the mailer's copy (`r`, and the email if it was ever kept), with a test.
5. **Wording and logs:** neutral wording, and the password never in any log.
- **PO's data rule for the mailer:** it keeps the email only to send codes, deletes it on removal, and never logs it. The design is stricter still: the client passes the address at send time, so the mailer needn't keep it at all.
- **Still open:** the Security Lead's and Tech Lead's verdicts, and the owner's choice on the second bootstrap (ship behind a flag, or wait).

## 8. Security Lead's changes (at fc26da1; these replace §1–§3 where they differ)
- **E1, the recovery notice and cancel:**
  - The recovering client has no seed, so it can't sign with `w`, and an attacker wouldn't post a notice anyway.
  - So when recovery is turned on, the client **registers `w_pub` with the mailer**. The mailer itself records the pending reset, and my signed-in devices **poll the mailer** for it.
  - **A cancel is signed by `w`**, and the mailer checks it against the registered `w_pub`.
  - **Stated plainly:** the 72-hour hold relies on an honest mailer.
- **E2, a password change kills old envelopes:**
  - Each account has an **OPRF key version** at the OPRF servers. Changing the password rotates it, and the servers **delete the old share**, so an old envelope can no longer be opened by anyone.
  - The envelope header carries that version, and the Argon2id parameters.
  - This replaces "an old copy opens with the old password until holders refresh" in §3.
- **E3, one operator today:** for now the same operator runs the OPRF servers and the mailer. That operator could take over recovery-enabled accounts, and could guess passwords offline against stolen envelopes.
  - §3's table gets this as a row.
  - **The opt-in text discloses it.**
- **More limits:** OPRF evaluations are also rate-limited **per handle**, not only per IP.
- **Answers to §6:**
  1. **Gating:** it ships **behind a flag, off until at least 2 independent bootstraps are live**.
  2. **The mailer:** it's ours, on a host **outside the OPRF set**. The SMTP provider may be a third party.
  3. **Recovery:** 72 hours and **off by default**, with an optional 7-day hold.
  4. **Argon2id parameters:** they go in the envelope header; 64–128 MiB on phones, 256 MiB on desktop.

## 9. Tech Lead's changes (at fc26da1; these replace earlier sections where they differ)
1. **The mailer alone must not be able to recover.** The OPRF is public, so the mailer can turn any address it mails into `E`, and it enforces its own hold.
   - So `r` is **split**: `r = r_m ⊕ r_1 ⊕ … ⊕ r_n`, with `r_m` at the mailer and one share at each OPRF bootstrap. Each is encrypted to its holder at opt-in.
   - A verified code makes the mailer issue a signed "code verified at T" for `tag_e`. **Each share holder starts its own 72-hour hold** from that, and releases its share only after the hold, if no cancel came.
   - A cancel, signed by `w` (whose public key is registered with every holder at opt-in, as in E1), goes to all of them.
   - Takeover then needs the mailer **and** every share holder, or the inbox plus 72 hours with no cancel.
   - The recovery envelope is kept with the share holders, not in the open backup store.
2. **The pending notice:** the services publish it themselves, signed by the mailer under `tag_e`. My devices poll for it, and a cancel is signed by `w`. Same as E1.
3. **Nobody can block sign-in by knowing only the email.**
   - The password envelope's lookup tag and write key come from **both** email and password: `tag_ep = derive_key("mvs/email-pw-tag/v1", handle ‖ P)`, and the write key `w_ep` is Ed25519 from `derive_key("mvs/email-pw-write/v1", handle ‖ P)`.
   - Holders accept writes under `tag_ep` only if signed by the key whose hash gives `tag_ep`, as for backups.
   - Knowing the email alone gives neither.
- **The API:**
  - `setEmailPassword`, `removeEmailPassword`, `setEmailRecovery`;
  - `getEmailSignIn() → { available, … }` (`available: false` until two bootstraps are live);
  - `signInWithEmail`;
  - `requestRecovery`, then `confirmRecoveryCode`, then `completeRecovery`;
  - `cancelRecovery`;
  - the event `account.recoveryPending`.
- **The envelope:** a `kdf` field holds the Argon2id parameters per envelope, so they can differ by device and change later. The OPRF key version (E2) goes there too.
- **Crates:** no duplicate crypto crates. The OPRF and `argon2` crates are chosen to share the tree's existing curve and hash crates. `argon2` is already used by `identity`.
- **Answers to §6** (matching the Security Lead's):
  - behind a flag (`available: false`) until two bootstraps are live;
  - our own mailer, sending through an SMTP relay;
  - a 72-hour hold, off by default;
  - the KDF parameters in the envelope.
- **Critical path:** devops' OPRF service and share holders.
4. **OPRF key rotation is authorised by `w`:**
   - The OPRF servers rotate an account's key version, and delete the old share (E2), **only on a request signed by `w`**. They check it against the `w_pub` registered with them at opt-in.
   - Nobody else can force a rotation, so nobody else can lock the owner out.
   - **A rotation cancels a pending recovery:** only a device holding the seed can sign with `w`, so a rotation means the owner is active. Every share holder treats it as a `w`-signed cancel.
   - The recovery shares themselves don't change. `r` is tied to `E`, not to the password, so a recovery set up before a password change still works afterwards.

## 10. Security Lead's additions (approved at 67e248e; for the build)
- **Every holder publishes the pending state:** each share holder, not only the mailer, publishes the pending-reset state for `tag_e`. My devices poll all of them, so a lying mailer can't hide a reset.
- **The opt-in text also says:** "If any one of the recovery holders loses its share, email recovery stops working. Your 12 words always still work."
