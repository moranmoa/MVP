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
