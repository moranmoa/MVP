# Bug list – exploratory testing of `main` (2026-11-03)

Tested at `5ef10ff` (UI shell with the mock core, core crates `records`,
`authority`, `sync`, `updater`, `spike-peer`). Every UI bug has a repro in
`qa/tests/explore.spec.ts` marked `bug('QA-xx')`: it runs as an
expected failure, so the suite turns red when the bug is fixed and the mark
can go. Core findings come from `qa/core/cli-checks.sh`.

Severity: **High** – loses data or blocks a main flow; **Medium** – wrong
behaviour a tester will hit, or a spec/security requirement missed;
**Low** – cosmetic or an edge case.

## Bugs

| ID | Sev. | Owner | Summary | Steps to reproduce | Expected | Actual | Repro |
| --- | --- | --- | --- | --- | --- | --- | --- |
| QA-01 | High | ui | Recovery words are lost for good if the app reloads while they are shown | 1. New account, enter a birth date. 2. On "מילות השחזור שלך" reload the page (crash, power cut, Ctrl+R). | Onboarding resumes on the words, or Settings → Account can show them again (`getRecoveryWords` exists in the core API). | The app opens on Home with the "words not checked" banner. The words are never shown again, so the check can never pass and **the account can never create a server**. | `QA-01` |
| QA-02 | Medium | ui | Profile cannot be edited after onboarding | 1. Finish onboarding (or press "דילוג" on the profile step). 2. Open Settings → חשבון. | The profile step says "אפשר לשנות בכל רגע"; spec (MVP): display name, avatar, banner, bio. | Settings has only recovery words. A skipped profile shows the identity code as the name forever. There is no banner field anywhere. | `QA-02` |
| QA-03 | Medium | ui | A large profile picture traps the user on the profile step | 1. Profile step. 2. Pick an 8 MB image, enter a name. 3. Press "סיום". | The picture is scaled down or refused with a message; onboarding finishes. | Nothing happens; "סיום" stays active. Console: uncaught `QuotaExceededError` from `MockCore.save` via `updateProfile`. The UI has no size limit, no resize and no error handling for a rejected `updateProfile` – the real core will need a limit too. | `QA-03` |
| QA-04 | Low | ui | Birth year 1900 (age 126) is accepted | Date-of-birth step: 1 / 1 / 1900. | A plausibility bound (e.g. ≤ 120 years) catches typos such as 1909 for 2009. | Accepted. | `QA-04` |
| QA-05 | Low | ui | Restore: pasting 13 words silently drops one | "כבר יש לי חשבון" → paste 13 words into box 1. | A message that 13 words were pasted (12 expected). | Boxes 1–12 are filled, the 13th word disappears without notice. | `QA-05` |
| QA-06 | Low | ui | Server name made only of invisible characters is accepted | Create server, name = U+200B U+200F U+2060, any age limit, "יצירה". | "צריך לתת לשרת שם." – `trim()` does not remove these. | An "empty" server is created; the rail button has no visible initials. Same gap for display names. | `QA-06` |
| QA-07 | Low | ui | "Copy invite" says "הועתק" even when copying failed | Make `navigator.clipboard.writeText` reject (insecure context, permission denied). Create a server → "העתקה". Same in the sidebar link button. | Show that copying failed (and select the link text so it can be copied by hand). | Shows "הועתק". | `QA-07` |
| QA-08 | Medium | ui, sync | "Keep looking in the background" does not keep anything visible | Join with a link containing `offline` → "להמשיך לחפש ברקע". | The copy promises "the server appears in the list once someone connects": some pending-join indicator, and the join actually completes later. | The dialog closes and nothing shows a join in progress. `CoreApi` has no background-join concept (only `cancelJoin`), so the contract with sync is missing. | `QA-08` |
| QA-09 | Low | ui | Avatar letter breaks flags and ZWJ emoji | Display name "🇮🇱 ישראל", send a message. | Avatar shows 🇮🇱. | Shows a lone regional indicator "🇮" (first code point, not first grapheme). Same for 👨‍👩‍👧. | `QA-09` |
| QA-10 | Medium | ui, records | Composer has no limit, but `records` caps a Message body at 16 KiB | Paste 8,200 Hebrew letters (16,400 bytes UTF-8) into the composer. | Send is disabled with a counter / message, matching `Kind::Message.max_body()`. | Send is enabled. With the real core the send will be rejected – and the composer clears the text *before* `sendMessage` resolves, with no catch, so the text is lost. | `QA-10` |
| QA-11 | Low | ui | Unsent draft is lost when switching channels | Type in #כללי, click #lfg, click #כללי. | The draft is still there. | Composer is empty (`ChatView` is keyed by channel). | `QA-11` |
| QA-12 | Medium | ui | At 200% scale the chat column shrinks to ~260 px | 1366×768, Settings → מראה: scale 200%, font 24 px. Open a server with the member list on. | Spec: "הפריסה לא נשברת ב-200% ובגופן 24px". Side panels collapse so chat stays readable. | Chat column is 262 px: one word per line, composer placeholder cut to "כתי". No horizontal overflow, so it is easy to miss. Screenshot: `test-results/explore-shots/scale-200-he.png`. | `QA-12` |
| QA-13 | Medium | ui, records | Bidi control characters in names reorder them (impersonation) | Display name "Moran " + U+202E + "evil". Send a message. | Bidi controls (U+202A–202E, U+2066–2069) are stripped or neutralised in names, server and channel names – in the UI and in core validation. | Shows "Moran live". `<bdi>` isolates the name from the line but not from itself. | `QA-13` |
| QA-14 | Medium | updater | A manifest that never expires can be signed and is accepted | `updater sign-manifest` with `expires_at = 18446744073709551615`, then `verify`. | Spec (security, required before alpha): signed metadata with an expiry so a user cannot be frozen on an old release. A maximum validity window (e.g. 30 days) enforced in `ManifestBody::validate`. | Signed and verified. Any such manifest can be replayed forever to hide newer releases from a client that lost its `ClientPolicy` state. | `cli-checks.sh` |
| QA-15 | Low | updater | A manifest issued a year in the future is accepted | Body with `issued_at = now + 365 d`, sign, `verify`. | Refused, or at least warned (clock skew allowance). | `signatures ok … exit 0`. | `cli-checks.sh` |
| QA-16 | Medium | sync | `spike-peer` hangs silently when the relay is unreachable or the relay ID is wrong | `spike-peer dial /ip4/127.0.0.1/tcp/9/p2p/<any id> <any id>` (or a mistyped relay address from a tester). | A `RESULT path=failed …` line or an error within the 30 s dial timeout. | Runs forever with no output after the peer-ID line. The wait for the relay's `identify` has no timeout and ignores `OutgoingConnectionError`; the 30 s deadline starts only afterwards. Testers following `TESTING.md` will just wait. | `cli-checks.sh` |

### Seen in code, not reproduced through the mock

- `ProfileCard`: `await core.kickMember(...)` has no catch – a rejected kick
  leaves the confirm open and an unhandled rejection (ui, Low).
- Restoring an account skips the profile step, so a restored identity has no
  display name until QA-02 is fixed (ui, Low).
- `MockCore.restoreIdentity` sets the birth date to `1995-01-01`; the spec says
  the date returns from the backup. Mock only, but the real core must restore
  it (records/authority, note).

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
- Messages: mixed he/en/emoji bodies (`dir="auto"`), 100,000-character paste
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
