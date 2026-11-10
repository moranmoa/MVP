# Voice v1 test plan

What this plan covers: one voice room per server ("Join chat"), up to 5 people.

Sources:
- the voice-room contract (tech lead, 2026-11-10; the shared notes, `voice-room-contract.md`);
- #223: api, piece B;
- #224: call bar, piece E;
- the room-entry follow-up to #224 (piece E);
- #228: microphone permission, piece C;
- #225: the one-time IP notice spec, `docs/ux/specs/voice-ip-notice.md`;
- #220: the relay on a Linux host.

Docs only. Each case says how to automate it.

## How each case is run

| Mark | Meaning |
| --- | --- |
| **PW** | Playwright against the mock core (`qa/`, the `smoke` or `explore` project). It drives the UI only. The mock fakes the roster and the error codes. |
| **PW+** | Playwright, but it needs a mock switch that doesn't exist yet. The switch is named in the case. Today the mock has `?callError=relay-only\|call-full\|not-implemented`, plus a call with you and 2 members. |
| **U** | Already a unit test, or belongs in one: vitest in `ui/`, `cargo test` in `core/api`, or `node --test` in `desktop/`. Listed so QA checks it exists, not to duplicate it. |
| **E2E-D** | Desktop e2e (`desktop/test/e2e.test.mjs`, Electron). Not in PR CI until CI-D1 (#230). |
| **M** | Manual, on installed builds and real machines. Needs a microphone and speakers or a headset. |

Machines: **A** is the owner, and **B**, **C**, **D**, **E** and **F** are members. Up to 6 accounts are needed for "full". Several accounts can run on one PC as separate OS users or VMs. Audio checks need at least 2 physical machines.

Report failures as in `docs/qa/v0.2.5-smoke.md`: the case ID, expected vs actual, a screenshot and the diagnostics file. Write `<relay-domain>` and never a real address or IP, even though calls expose IPs between participants.

---

## V1. The voice room in the sidebar

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V1.1 | Open any server as a member. | The channel sidebar has one **"Join chat"** entry (Hebrew label from i18n `call.room`) with the speaker icon, below or apart from the text channels. There is exactly one per server. | PW |
| V1.2 | Open a text channel's header. | There's no "Join call" button and no speaker marker on text channels: both were removed from #224. | PW |
| V1.3 | Nobody is in the room. | The entry shows no avatars, and there's no "in a call" state anywhere. | PW |
| V1.4 | B, C and D are in the room; A is not. | A sees B, C and D under "Join chat", with avatar, name and mic-off icon when muted. The list matches the people in the room, on every member's screen, whether they're in the room or not. | PW (mock roster), **M** across 3+ machines |
| V1.5 | A member who isn't synced yet. | Falls back to the short code from the ID. There's no blank row and no raw ID. | PW+ (mock: roster entry for an unknown user) |
| V1.6 | A name with bidi or control characters, very long, or with RTL+LTR mixed. | Rendered through `guardCore`, clipped, with no layout break and no direction leaking into the next row. | PW+ (mock: hostile display name in the roster) |
| V1.7 | A `voice.room` event with more than 5 entries, a non-hex ID or unknown fields. | Dropped by `sanitize.ts`, and the UI keeps the last good roster. | **U** (vitest, sanitize) |
| V1.8 | Switch server, then come back. | Each server shows its own roster. There's no leak from the previous server. The UI re-reads with `getVoiceRoom`. | PW |
| V1.9 | Hebrew and English, light and dark. | RTL/LTR mirrored correctly; icons and names aligned. Screenshot range 240–249. | PW (screenshots) |
| V1.10 | Keyboard and screen reader. | "Join chat" can be reached with Tab and pressed with Enter or Space. The roster is read out as a list of names with "mic off" where it applies. A disabled entry gives its reason (V2.6, V3.1). | PW (roles and names) + **M** (NVDA) |

## V2. Join, mute, leave, and the 5-person cap

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V2.1 | A clicks "Join chat" (the IP notice was already acknowledged). | The call bar appears ("בשיחה ב-…", the people, Mute, Leave). A appears in their own roster entry and, within about 10 s, in every other member's roster. | PW (UI) + **M** (propagation) |
| V2.2 | A and B are in the room on 2 machines. | Both hear each other. The audio starts within 5 s of the second join, with no echo when using a headset. | **M** |
| V2.3 | A mutes. | `aria-pressed=true`, and the mic-off icon shows for A on every member's roster within about 10 s. B hears nothing from A. Unmute restores the audio. | PW (UI) + **M** (audio, propagation) |
| V2.4 | A leaves. | The bar goes away and "Join chat" can be clicked again. A drops off every roster within about 10 s, and B's audio from A stops at once. | PW (UI) + **M** |
| V2.5 | A closes the app or kills the process without leaving. | A drops off the other rosters within **30 s** (hello timeout). B's audio from A stops. | **M** |
| V2.6 | Five people are in the room; F looks at the sidebar. | F's "Join chat" entry is **disabled**, and its tooltip says the call is full (up to 5). | PW+ (mock: roster of 5) + **M** |
| V2.7 | F calls `joinCall` anyway, for example racing a 5th joiner. | The core refuses with `call-full`, and F sees "השיחה מלאה: אפשר עד 5 משתתפים." The roster stays at 5. Nobody already in the call is dropped. | PW (`?callError=call-full`) + **M** (5 real clients) |
| V2.8 | One of the 5 leaves. | F's entry becomes enabled, and F can join. | PW+ + **M** |
| V2.9 | Five people for 10 minutes, at least 2 of them on different networks. | No drops. Audio stays clear. CPU on a mid-range laptop is under about 25% for the app. | **M** |
| V2.10 | Join, switch to another server, and come back. | The call continues and the bar stays. The roster in the first server still shows you. | PW + **M** |
| V2.11 | `joinCall` with a text channel ID (through the api, not the UI). | Refused with `not-allowed`. The UI never offers it. | **U** (core/api `may_call`) |

## V3. Relay-only mode refuses calls

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V3.1 | Relay-only mode is on, and you open a server. | "Join chat" is **disabled**, and its tooltip says calls are off in relay-only mode. The roster is still visible. | PW+ (mock: relay-only flag on the room entry) |
| V3.2 | You call `joinCall` anyway. | The **core** refuses with `relay-only`, and the UI shows the relay-only reason line. **The disabled entry is only cosmetic.** | PW (`?callError=relay-only`) + **U** (core) + **M** |
| V3.3 | Relay-only, first call ever on this computer. | **No IP notice** dialog, only the reason line (#225 §1). | PW+ |
| V3.4 | Turn relay-only on while in a call. | You leave the call, or the call is refused on the next tick, and the bar explains why. **Confirm the intended behaviour with dev-identity**; the contract doesn't say. | **M** (open question) |
| V3.5 | Relay-only, during a call that other members are in. | No direct connection from this machine to the participants (check with `netstat`, or the relay logs: no new circuit is used for media). | **M** |

## V4. A kicked member drops off the roster at once

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V4.1 | B is in the room; owner A kicks B. | B leaves **every** member's roster within one tick (SYNC_EVERY), not after the 30 s timeout. B's audio stops for everyone. | **M** (time it with a stopwatch, 3 tries) |
| V4.2 | B (kicked) tries to join again, or keeps sending hellos. | `getVoiceRoom` and `joinCall` give `not-allowed` for B. B's hellos are ignored, so B doesn't reappear. | **U** (core/api) + **M** |
| V4.3 | The kicked B still has the app open on that server. | B's sidebar doesn't show the room's roster anymore (only members get `voice.room`). | **M** |
| V4.4 | A kicks a member who isn't in the call. | The roster doesn't change, and nothing flickers. | **M** |

## V5. Microphone permission, in the app only (#228; V-C2 in #230)

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V5.1 | First "Join chat" on a fresh install. | The OS mic prompt (Windows privacy, or the macOS/Linux portal) appears **for MVS**, or the mic just works if allowed. Audio flows after allowing. | **M** |
| V5.2 | Mic permission for the app page (`app://mvs`, top frame). | `permissions.query('microphone')` is granted. `getUserMedia({audio:true})` works. | **E2E-D** (flipped assertion in #228) + **U** (`security.test.js`) |
| V5.3 | Camera: `getUserMedia({video:true})` and `{audio:true, video:true}`. | Refused. | **E2E-D** + **U** |
| V5.4 | Screen and window capture: `getDisplayMedia`. | Refused. | **E2E-D** + **U** |
| V5.5 | Desktop or system audio (`getDisplayMedia({audio:true})`, `chromeMediaSource: 'desktop'`). | Refused. Nothing but the mic is captured. | **E2E-D** (add the case if missing) + **M** (play music during a call, B must not hear it except through A's mic) |
| V5.6 | Any other origin or a subframe asks for the mic (e.g. an iframe from a link preview). | Refused. | **U** |
| V5.7 | The OS mic permission is denied for MVS. | Joining shows a clear "no microphone" message. The app doesn't crash. You can still listen, if the engine supports it; otherwise the join is refused with a reason. | **M** |
| V5.8 | No mic device at all, or it's unplugged mid-call. | Same as V5.7. On unplug the call keeps going muted and shows a notice. | **M** |
| V5.9 | **V-C2 (after #230's follow-up):** look at the OS "mic in use" indicator. | It shows only while you're in a call, and goes off within about 2 s of leaving. **Before V-C2 ships, record what happens; it's not a failure.** | **M** |
| V5.10 | The web build (`ui/` in a browser, not the desktop shell). | The browser's own prompt, scoped to the site. This plan covers the desktop app only. | (out of scope) |

## V6. The one-time IP notice (#225)

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V6.1 | First "Join chat" on this computer. | A modal dialog: "לפני השיחה הראשונה", with the IP text. **`joinCall` isn't called** until "הבנתי". | PW (assert no join request before OK, through the mock or a store spy) |
| V6.2 | "ביטול", Esc or X. | No join, and no `mvs.notices.voiceIp.v1` key. The next click shows the dialog again. Focus goes back to "Join chat". | PW |
| V6.3 | "הבנתי". | The join goes ahead, the key is saved as `"1"`, and focus moves to Mute in the call bar. | PW |
| V6.4 | Join again, also after a reload. | No dialog. | PW |
| V6.5 | Relay-only. | No dialog (V3.3). | PW+ |
| V6.6 | `localStorage` throws, for example storage blocked. | The dialog shows on every join, and the join still works after OK. | PW (stub `localStorage` with `addInitScript`) |
| V6.7 | Dialog a11y. | `role=dialog`, labelled by the title and described by the body. Initial focus is on "הבנתי". | PW |
| V6.8 | English, light theme. | Matches screenshot 244. | PW (screenshots) |
| V6.9 | The installed desktop app: install, first call, restart, call again. | The notice appears once per install. **Record** whether it comes back after an update (it shouldn't: same profile). | **M** |

## V7. Real networks

Run after B's room work is merged and on a build that includes it.

### V7a: two machines on one LAN (now)

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V7a.1 | A and B on the same Wi-Fi or LAN, both join. | Audio both ways within 5 s, and the direct/relay indicator shows **direct**. Latency feels conversational: clap test, under about 300 ms. | **M** |
| V7a.2 | Mute, unmute, leave and rejoin, 5 times. | Every step works. No stuck "in call" state and no ghost roster entry after 30 s. | **M** |
| V7a.3 | 10-minute call. | No drop, and no audio drift. | **M** |
| V7a.4 | Wi-Fi off and on, on B, mid-call. | B drops off A's roster within 30 s. B can rejoin after the network is back. A's call continues. | **M** |

### V7b: home + hotspot (after the relay moves to a Linux host, #220)

The relay must see the clients' real public addresses (host networking) before DCUtR and direct media can work across NATs.

| # | Case | Expected | How |
| --- | --- | --- | --- |
| V7b.0 | Precondition: #220 is deployed and checked. | The relay logs show a public remote address for the hotspot client, and `circuits_denied` is flat. Devops confirms first. | devops |
| V7b.1 | A at home, B on a phone hotspot, both join. | Audio both ways within 10 s, through the STUN server (#222) and a direct path. Record whether the indicator shows direct. | **M** |
| V7b.2 | Same, but with the direct path impossible (symmetric NAT on both sides, or blocked UDP). | The join fails with a clear message, or audio fails cleanly. There's no relay fallback for media in v1. No crash, and the roster still clears. **Record the actual behaviour.** | **M** |
| V7b.3 | 3 people: home, hotspot, and a third network. | Everyone hears everyone (mesh). | **M** |
| V7b.4 | 5 people across at least 2 networks, 10 minutes. | As V2.9. | **M** |
| V7b.5 | Privacy check during V7b.1: run `netstat` on A. | Only participants' IPs appear for media. Members who aren't in the call don't get A's IP from the roster: the roster carries no addresses, per the contract. | **M** |

---

## Automation summary

| Area | PW now | PW+ (mock switch needed) | Unit / E2E-D | Manual only |
| --- | --- | --- | --- | --- |
| V1 room entry | V1.1–V1.4, V1.8–V1.10 | V1.5, V1.6 | V1.7 | V1.4 (propagation), V1.10 (NVDA) |
| V2 join/mute/leave/full | V2.1, V2.3, V2.4, V2.7, V2.10 | V2.6, V2.8 | V2.11 | V2.2, V2.5, V2.9 |
| V3 relay-only | V3.2 | V3.1, V3.3 | V3.2 (core) | V3.4, V3.5 |
| V4 kick | none | none | V4.2 | V4.1, V4.3, V4.4 |
| V5 mic permission | none | none | V5.2–V5.6 | V5.1, V5.7–V5.9 |
| V6 IP notice | V6.1–V6.4, V6.6–V6.8 | V6.5 | none | V6.9 |
| V7 networks | none | none | none | all |

### Mock switches to ask piece E for

- `?voiceRoom=N`: a roster of N members (0–5);
- `?relayOnly=1`;
- a roster entry for an unknown user;
- a roster entry with a hostile name;
- a way to emit `voice.room` after a delay, for propagation-style assertions.

### Proposed Playwright file

`qa/tests/voice.spec.ts`:
- **smoke project:** V1.1, V2.1, V2.3, V2.4, V6.1–V6.3;
- **explore project:** everything else marked PW or PW+.

It is written once piece E's room entry merges.

## Open questions

1. V3.4: what happens to a running call when relay-only is turned on? (dev-identity / the tech lead)
2. V5.7: can someone without a microphone join to listen? (product)
3. V7b.2: what exactly does the user see when no direct path exists, given there's no media relay in v1? (dev-voice-shell / UX)
