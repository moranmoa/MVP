# MVS bug list – exploratory testing of `main` (2026-11-03)

Tested at `5ef10ff` (UI shell with the mock core, core crates `records`,
`authority`, `sync`, `updater`, `spike-peer`). Every UI bug has a repro in
`qa/tests/explore.spec.ts` marked `bug('QA-xx')`: it runs as an
expected failure, so the suite turns red when the bug is fixed and the mark
can go. Core findings come from `qa/core/cli-checks.sh`.

**Re-test 1 – `0aa9b45` (after PR #7, #10, #13), 2026-11-03.** Smoke 8/8;
explore 46/46 as expected (35 checks + 11 open UI bug repros + UX-01);
`cargo test` 98 green (records 16, authority 21 incl. 8 new security tests,
sync 4, updater 57); cli-checks 34/37 (QA-14/15/16, fixes not merged yet).
Closed: QA-03, QA-13. Still open: QA-01, 02, 04–12, 14–16, UX-01. New:
QA-17, QA-18, QA-19. PR #10's hostile-data demo, settings validation, phone
member sheet and English phone layout passed (see "What passed").

**Re-test 2 – `959b492` (after PR #5, #8, #9, #11, #17, #24), 2026-11-03.**
Smoke 8/8; explore 46/46 as expected. `cargo test --workspace` 218 green
(new crates `identity` and `groupkeys` included); cli-checks 37/37. Closed:
QA-14, QA-15 (PR #8), QA-16 (PR #5), UX-01 (PR #24, kept as a regression
check). Updated after the PR #27 review: QA-17's expected result (one
4,000-code-point limit with a counter from 3,800), QA-18 to Medium, QA-03's
resize follow-up in B11e. No new bugs.

Severity (same scale as TEST_PLAN.md): **Blocker** – identity or data loss,
or a security bypass; no release. **High** – a main flow works only with a
workaround, or an MVP security requirement is missed. **Medium** – wrong
behaviour a tester will hit, or a spec requirement missed. **Low** –
cosmetic or an edge case. Severities for the closed alpha were set in the
product owner's review of PR #13.

## Status and backlog

The backlog tasks (B-numbers) are from the product owner's review
(`docs/reviews/po-review-1.md`, PR #6).

| ID | Sev. | Status | Backlog |
| --- | --- | --- | --- |
| QA-01 | Blocker | open | B19 (screen returns to the words) + B2 (words kept until checked, `getRecoveryWords` after restart) |
| QA-02 | Blocker | open | B11e |
| QA-03 | High | **closed** – fixed by PR #10 (512 KB limit with a message; verified 2026-11-03) | B11e – the resize behaviour (ש-22: scale down pictures up to 25 MB / 40 MP) is tracked there, plus a UI error when `updateProfile` rejects |
| QA-04 | Low | open | B7 |
| QA-05 | Low | open | B22 |
| QA-06 | Low | open – since PR #10 the server is named "?" instead of blank, still no error | B23 |
| QA-07 | Low | open | B22 |
| QA-08 | Medium | open | B18 |
| QA-09 | Low | open | B22 |
| QA-10 | Medium | open | B6 + B4 |
| QA-11 | Low | open | B22 |
| QA-12 | **High** | open – raised from Medium by the product lead (PR #50 review: the chat column is ~120 px there) | B12 (with UX-17) |
| QA-13 | High | **closed** – fixed by PR #10 (`guardCore` strips bidi controls; verified 2026-11-03) | B23 (new, P1: neutralise bidi and invisible characters in UI and core) |
| QA-14 | High | **closed** – fixed by PR #8 (verified 2026-11-03, cli-checks) | B8 (max validity 30 days) |
| QA-15 | Low | **closed** – fixed by PR #8 (verified 2026-11-03, cli-checks) | B8 (`issued_at` at most 24 h ahead) |
| QA-16 | Medium | **closed** – fixed by PR #5 (verified 2026-11-03, cli-checks: error within 60 s) | B5 (fix before the NAT experiment) |
| UX-01 | Blocker | **closed** – fixed by PR #24 (verified 2026-11-03) | B17 – regression check `fixed UX-01` in explore.spec.ts |
| QA-17 | Medium | open – limit decided in the PR #27 review | B6 (one limit of 4,000 code points for composer, display and core; counter from 3,800) |
| QA-18 | Medium | open – Medium for the closed alpha (the phone app is phase 3) | B12 / UX-05; the fix ships with B19 |
| QA-19 | Low | open – new | B12 |

## Bugs

| ID | Sev. | Owner | Summary | Steps to reproduce | Expected | Actual | Repro |
| --- | --- | --- | --- | --- | --- | --- | --- |
| QA-01 | **Blocker** | ui | Recovery words are lost for good if the app reloads while they are shown | 1. New account, enter a birth date. 2. On "מילות השחזור שלך" reload the page (crash, power cut, Ctrl+R). | Onboarding resumes on the words, or Settings → Account can show them again (`getRecoveryWords` exists in the core API). | The app opens on Home with the "words not checked" banner. The words are never shown again, so the check can never pass and **the account can never create a server**. | `QA-01` |
| QA-02 | **Blocker** | ui | Profile cannot be edited after onboarding | 1. Finish onboarding (or press "דילוג" on the profile step). 2. Open Settings → חשבון. | The profile step says "אפשר לשנות בכל רגע"; spec (MVP): display name, avatar, banner, bio. | Settings has only recovery words. A skipped profile shows the identity code as the name forever. There is no banner field anywhere. | `QA-02` |
| QA-03 | **High** | ui | A large profile picture traps the user on the profile step | 1. Profile step. 2. Pick an 8 MB image, enter a name. 3. Press "סיום". | The picture is scaled down or refused with a message; onboarding finishes. | Nothing happens; "סיום" stays active. Console: uncaught `QuotaExceededError` from `MockCore.save` via `updateProfile`. The UI has no size limit, no resize and no error handling for a rejected `updateProfile` – the real core will need a limit too. | `QA-03` |
| QA-04 | Low | ui | Birth year 1900 (age 126) is accepted | Date-of-birth step: 1 / 1 / 1900. | A plausibility bound (e.g. ≤ 120 years) catches typos such as 1909 for 2009. | Accepted. | `QA-04` |
| QA-05 | Low | ui | Restore: pasting 13 words silently drops one | "כבר יש לי חשבון" → paste 13 words into box 1. | A message that 13 words were pasted (12 expected). | Boxes 1–12 are filled, the 13th word disappears without notice. | `QA-05` |
| QA-06 | Low | ui | Server name made only of invisible characters is accepted | Create server, name = U+200B U+200F U+2060, any age limit, "יצירה". | "צריך לתת לשרת שם." – `trim()` does not remove these. | An "empty" server is created; the rail button has no visible initials. Same gap for display names. | `QA-06` |
| QA-07 | Low | ui | "Copy invite" says "הועתק" even when copying failed | Make `navigator.clipboard.writeText` reject (insecure context, permission denied). Create a server → "העתקה". Same in the sidebar link button. | Show that copying failed (and select the link text so it can be copied by hand). | Shows "הועתק". | `QA-07` |
| QA-08 | Medium | ui, sync | "Keep looking in the background" does not keep anything visible | Join with a link containing `offline` → "להמשיך לחפש ברקע". | The copy promises "the server appears in the list once someone connects": some pending-join indicator, and the join actually completes later. | The dialog closes and nothing shows a join in progress. `CoreApi` has no background-join concept (only `cancelJoin`), so the contract with sync is missing. | `QA-08` |
| QA-09 | Low | ui | Avatar letter breaks flags and ZWJ emoji | Display name "🇮🇱 ישראל", send a message. | Avatar shows 🇮🇱. | Shows a lone regional indicator "🇮" (first code point, not first grapheme). Same for 👨‍👩‍👧. | `QA-09` |
| QA-10 | Medium | ui, records | Composer has no limit, but `records` caps a Message body at 16 KiB | Paste 8,200 Hebrew letters (16,400 bytes UTF-8) into the composer. | Send is disabled with a counter / message, matching `Kind::Message.max_body()`. | Send is enabled. With the real core the send will be rejected – and the composer clears the text *before* `sendMessage` resolves, with no catch, so the text is lost. | `QA-10` |
| QA-11 | Low | ui | Unsent draft is lost when switching channels | Type in #כללי, click #lfg, click #כללי. | The draft is still there. | Composer is empty (`ChatView` is keyed by channel). | `QA-11` |
| QA-12 | **High** | ui | At 200% + 24 px on 1366×768 both side panels stay open and the chat column is squeezed | 1366×768, Settings → מראה: scale 200%, font 24 px (effective width ~683 px). Open a server with the member list on. | Spec: "הפריסה לא נשברת ב-200% ובגופן 24px". The member list collapses below ~900 effective px and the channel list moves to a drawer below ~700, as on the phone layout. | Both the channel list and the member list stay open. On main the chat column is 262 px, one word per line, and the composer placeholder is cut to "כתי". In PR #50 it is ~120 px and words break letter by letter (screenshot 76 there). No sideways overflow, so it is easy to miss. Screenshot: `test-results/explore-shots/scale-200-he.png`. | `QA-12` |
| QA-13 | **High** | ui, records | Bidi control characters in names reorder them (impersonation) | Display name "Moran " + U+202E + "evil". Send a message. | Bidi controls (U+202A–202E, U+2066–2069) are stripped or neutralised in names, server and channel names – in the UI and in core validation. | Shows "Moran live". `<bdi>` isolates the name from the line but not from itself. | `QA-13` |
| QA-14 | **High** | updater | A manifest that never expires can be signed and is accepted | `updater sign-manifest` with `expires_at = 18446744073709551615`, then `verify`. | Spec (security, required before alpha): signed metadata with an expiry so a user cannot be frozen on an old release. A maximum validity window (e.g. 30 days) enforced in `ManifestBody::validate`. | Signed and verified. Any such manifest can be replayed forever to hide newer releases from a client that lost its `ClientPolicy` state. | `cli-checks.sh` |
| QA-15 | Low | updater | A manifest issued a year in the future is accepted | Body with `issued_at = now + 365 d`, sign, `verify`. | Refused, or at least warned (clock skew allowance). | `signatures ok … exit 0`. | `cli-checks.sh` |
| QA-16 | Medium | sync | `spike-peer` hangs silently when the relay is unreachable or the relay ID is wrong | `spike-peer dial /ip4/127.0.0.1/tcp/9/p2p/<any id> <any id>` (or a mistyped relay address from a tester). | A `RESULT path=failed …` line or an error within the 30 s dial timeout. | Runs forever with no output after the peer-ID line. The wait for the relay's `identify` has no timeout and ignores `OutgoingConnectionError`; the 30 s deadline starts only afterwards. Testers following `TESTING.md` will just wait. | `cli-checks.sh` |
| QA-17 | Medium | ui, records | A 4,001–16,384-byte message is accepted but shown cut for everyone, silently | Send 5,000 × "a" (5,000 bytes, under the 16 KiB `Kind::Message` limit). | Decided: one limit of 4,000 code points everywhere (composer, display, core), with a visible counter from 3,800; nothing is cut silently. | The sanitizer (`BODY_MAX = 4000`, PR #10) cuts the body to 4,000 characters for every reader, including the sender, and the "ההודעה ארוכה מדי" notice does not appear. Extends QA-10. | `QA-17` |
| QA-18 | Medium | ui | The recovery-word check does not fit a 375 px phone; "בדיקה" is off-screen | Phone 375×812: onboarding to "בדיקה קצרה". | The step fits the width; the button can be tapped. | Page is 516 px wide (141 px sideways overflow); "בדיקה" is at x = 416, outside the screen, and the hint text is cut on the left. Phone onboarding cannot be finished without sideways scrolling. Same root as UX-05. Screenshot `test-results/explore-shots/phone-word-check.png`. | `QA-18` |
| QA-19 | Low | ui | Reply reference is a 19 px tap target on phones | Phone 375×812, demo server, a message that replies to another. | Every control ≥ 44×44 px (PR #10 goal). | "ההודעה המקורית" button is about 212×19 px. | `QA-19` |

### Seen in code, not reproduced through the mock

- `ProfileCard`: `await core.kickMember(...)` has no catch – a rejected kick
  leaves the confirm open and an unhandled rejection (ui, Low; backlog B12).
- Restoring an account skips the profile step, and `MockCore.restoreIdentity`
  sets the birth date to a fixed `1995-01-01`. The 12 words do not contain
  the birth date (ש-8), so restore must ask for the birth date and the
  profile again (ui + records/authority, Low; backlog B2).

## MVP features from the spec with no UI yet

Not bugs in the shell, but each needs a test once built (see TEST_PLAN.md):
leaving a server and deleting the local copy; editing / deleting (closing) a
server; creating, renaming and deleting channels; marking a channel 18+;
revoking the invite link; correcting the birth date in Settings; banner image.

## What passed

- Smoke flow: onboarding (he, RTL), 3-word check, create server, send while
  nobody online (pending), join by link, delivered (✓✓), failed + retry,
  reload keeps the account, English is LTR.
- Age: exactly 16 today allowed, 16 tomorrow blocked; 31/02, 29/02/2001,
  future dates, day 0, month 13 refused; 29/02/2000 accepted; digit-only
  fields.
- Words check: wrong words refused; upper case and spaces accepted. Restore
  paste of 12 words with commas/capitals fills all boxes.
- Double-click "יצירה" creates one server; double Enter + cancel on join does
  not join; non-invite links ("hello", https, `javascript:`) refused.
- Messages: mixed he/en/emoji bodies send and display without errors (their
  direction is wrong – UX-01, blocker), 100,000-character paste
  renders in ~0.5 s, 15 rapid Enters send once, 10 rapid sends never empty,
  edit (ArrowUp) / "(נערך)" / reply / delete, block hides with "show anyway".
- Layout: long names ellipsised, no horizontal overflow at 200% (he/en),
  800×600 at 150%, resize 1920→640→1366 without errors, Ctrl+= / Ctrl+- /
  Ctrl+0 clamp to 75–200%, dark theme, 100k-message channel keeps < 80 rows
  in the DOM.
- Timing (production build in Docker, best of 3): server switch 32–50 ms
  (target < 100), channel switch 28–45 ms (target < 150).
- Diagnostics export contains no recovery words, birth date or message text.
- Core: `cargo test --release --locked` – 86 tests green (records 12,
  authority 13 incl. 10,000-scenario convergence in ~20 s, sync 4, updater
  57). `cli-checks.sh`: 34 of 37 checks pass – tampered body, channel,
  signature, artifact byte, truncated artifact, wrong platform, expiry,
  downgrade, wrong channel, untrusted key, signature transplant, missing
  signatures, > 64 KiB, empty file, duplicate JSON key, keygen inside the
  checkout, path in key name, key overwrite, bad thresholds, duplicate keys,
  non-canonical versions, unknown fields, bad CLI input; spike-peer rejects
  bad addresses, bad peer IDs, extra args and bad interactive input.
- Re-test 1 (PR #10): `?hostile=1` demo – no script runs, no `<img>`/`<a>`
  from message markup, no request to the remote "colour" URL, no bidi
  controls left on screen, no sideways overflow, the owner look-alike
  (מורן → מודן) is tagged "שם דומה". Corrupt settings in storage
  (`locale: "xx"`, `uiScale: 100`) fall back to defaults. Phone 375 px:
  send, back to channels and into another channel; member sheet is a
  dialog that Esc closes; English phone layout is LTR with no overflow.
  Avatar over 512 KB is refused with a message (QA-03 closed).
