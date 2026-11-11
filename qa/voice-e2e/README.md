# Voice e2e: a real two-peer call in Docker

This proves a voice call end to end with nothing faked except the microphone. It uses:
- the real `mvs-core` (`core/api`);
- the real desktop shell (`desktop/`, Electron 44) and the production UI build;
- a throwaway relay built from `infra/relay`.

Everything runs in Docker. Nothing runs on the host, and the live relay is never used.

```bash
bash qa/voice-e2e/run.sh
```

Other ways to run it:
- `REBUILD=1` rebuilds `mvs-core` and the UI first. Otherwise `.build/` is reused.
- `KEEP=1` leaves the containers running afterwards.

The first run builds `mvs-core` in release mode, which takes several minutes. After that a run takes about 2 minutes. Exit code 0 means every check passed.

## What it runs

| Container | What |
| --- | --- |
| `relay` | `infra/relay` with `ALLOW_PRIVATE_ADDRS=1` (test only), on the run's own network. |
| `a` (owner), `b` (member) | `desktop/Dockerfile.e2e`: the app under Xvfb with its own temp profile (`MVS_TEST_USER_DATA`), a session D-Bus with an unlocked test keyring, and `mvs-core` started by the shell as usual, with `MVS_RELAY` set. |

Isolation:
- Each run is its own compose project, `mvs-voice-<time>-<random>`, so containers, the network and the rendezvous volume are unique per run.
- Only the build caches are shared: cargo, the UI `node_modules`, and the desktop `node_modules` (`mvs-voice-e2e-desktop-nm-<lockfile hash>`). Runs may overlap: `deps.sh` installs under a lock and skips `npm ci` when that lockfile is already installed.

The microphone:
- Chromium's `--use-fake-device-for-media-stream` provides a beeping test microphone.
- `--use-fake-ui-for-media-stream` is **not** used. It grants media before the shell's permission handler runs, which would hide what #228 and V-C2 decide.

The network:
- It is not `internal`. The shell gathers WebRTC candidates only on the default route's interface (`WEBRTC_IP_POLICY`), and an internal Docker network has no default route.
- No STUN server is set, so nothing calls out.

## The scenario

`call.mjs` runs on both sides. The two sides step through it together using files in a shared volume.

1. **Setup through the bridge.** Each side creates an identity, passes the word check and sets a display name. A creates a server. B joins it by invite over the relay.
2. **Join.** Each side opens the server in the UI. A clicks "Join chat", gets the IP notice and clicks OK. B, still outside the call, checks the roster, then joins A's room from its row (0.4.3 voice rooms: "Join chat" opens a new room).
3. **WebRTC checks.** The test wraps `RTCPeerConnection` to read the engine's own connections, then checks:
   - the connection reaches `connected`;
   - the certificate the peer presents (`getStats`) equals the fingerprint in the signed SDP;
   - inbound and outbound RTP audio bytes are over 0 and growing.
4. **Media permissions in a call.** The microphone is granted. Camera, screen, and system audio with a screen share are refused.
5. **Mute.** A mutes. B must see A's mic-off icon and receive no audio energy from A. Then A unmutes.
6. **Leave.**
   - B leaves. B's bar goes, B's connection closes, B's microphone is refused again, and A sees B drop off.
   - Then A leaves, and B (outside the call) sees an empty roster.
7. **Rejoin.** Each side joins again without the IP notice. Last comes the legacy desktop-capture probe, because Chromium kills the renderer for it.

   Then both are back in the room, and A kicks B (V4). A checks that B drops off its roster within 5 s (not the 30 s timeout) and that A stays in the call alone. B checks that its call ends, `listVoiceRooms` and `joinCall` are refused, the roster is gone, and the mic is refused.

Results land in `results/<project>/`, which is git-ignored:
- `a.log` and `b.log`: one `PASS|FAIL <case>` line per check, plus diagnostics on failure;
- `results-a.json` and `results-b.json`;
- the shell logs (`main-*.log`), the relay log, and a final screenshot per side.

## Result on `main` 6c95251 (2026-11-10)

Runs 9 and 10 gave the same result. Which side offers depends on which identity is lower, so QA-21 silences A in some runs and B in others. The "no audio while muted" check measures A→B audio, so it fails along with QA-21 whenever A is the answerer. Case IDs are from `docs/qa/voice-v1-test-plan.md`.

| Case | Result | Evidence |
| --- | --- | --- |
| S1.1 join by invite over the relay | PASS | 0.3 s |
| V1.1 one "Join chat" entry with the speaker icon | PASS | both sides |
| V1.4 roster visible to every member | PASS | B, outside the call, sees A. Then both see both. |
| V2.1 join: the call bar with Mute and Leave, me in the roster | PASS | both |
| V2.2 WebRTC `connected` | PASS | host/udp → host/udp, DTLS connected |
| V2.2 the presented certificate equals the signed SDP fingerprint | PASS | sha-256, both sides |
| V2.2 audio A → B | PASS | about 20 KB of Opus in 3 s |
| **V2.2 audio B → A** | **FAIL: QA-21** | B (the answerer) sends 0 bytes. A receives nothing. |
| V2.3 mute shows on the other side | PASS | mic-off icon on B in 0.8 s. It goes away on unmute. |
| V2.3 no audio while muted | PASS | B's audio energy from A: 0.70 → 0 |
| V2.4 leave clears the roster, the bar and the connection | PASS | A sees B leave in about 0.1 s. B sees the roster empty when A leaves. |
| V5.2 microphone granted in a call | PASS | |
| V5.3 camera refused (video, audio+video) | PASS | NotAllowedError |
| V5.4 screen capture refused | PASS | NotAllowedError |
| V5.5 system audio refused | PASS | NotAllowedError with a screen share. The legacy `chromeMediaSource: 'desktop'` request gets no answer and Chromium kills the renderer ("bad IPC message, reason 263"). |
| V5.9 / V-C2 microphone only during a call | PASS | refused before joining and after leaving |
| V6.1 IP notice before the first join, no join until OK | PASS | no peer connection before OK |
| V6.3 OK joins, the choice is remembered, focus on Mute | PASS | |
| V6.4 no notice on the second join | PASS | |
| V4.1 the kicked member drops off the owner's roster at once | PASS | 0.4 s, and the connection to B ends |
| V4.2 the kicked member can't read the room or rejoin | PASS | `not-found` for both (the server is gone for B; the plan said `not-allowed`, and either is a refusal) |
| V4.3 the kicked member's call ends and the roster goes | PASS | 0.4 s after the kick |
| V4.4 the owner stays in the call | PASS | alone in the room |

**Not covered here** (still manual, per the plan): V2.5–V2.10 (timeout, the 5-person cap with real clients, long calls), V3 (relay-only, to add once #272 and #274 land), V5.1/V5.7/V5.8 (OS prompt, no device), V7 (real networks).

## Bugs found

See `qa/BUGS.md`:
- **QA-21** (Blocker for voice): the participant who answers is never heard.
- **QA-22** (Low): no candidates on a network without a default route.
