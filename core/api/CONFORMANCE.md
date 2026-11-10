# Conformance: the TS mock and the Rust core

The UI is built against `MockCore` (`ui/src/core/mock.ts`). The Rust core
(`core/api`) replaces it. `tests/fixtures/conformance.json` holds scenarios
that **both** run, as JSON commands, so the two can't drift without a test
failing on one side.

- Rust side: `tests/conformance.rs` runs every scenario whose `targets`
  includes `"rust"`, through `Core::handle`. It runs in CI with the rest of
  `cargo test`.
- TS side: planned (below). This PR doesn't touch `ui/`.

## Fixture format

```jsonc
{
  "version": 1,
  "scenarios": [{
    "name": "…",
    "targets": ["mock", "rust"],      // who must pass it
    "why-not-mock": "…",              // required when "mock" is missing
    "peers": ["alice", "bob"],        // one fresh core each; Rust peers share a loopback network
    "steps": [{
      "peer": "alice",
      "cmd": "createServer",          // a CoreCommands method name
      "args": { "input": { … } },     // the method's parameters, by TS parameter name
      "expect": { "ok": … } | { "error": { "code": "…", "positions"?: […] } },
      "events": ["server.added"],     // optional: must be emitted during the step, in this order (others may be interleaved)
      "save": { "server": "ok.id" }   // optional: dotted path into the response
    }]
  }]
}
```

Matching rules (identical in both runners):

- An expected object matches if every key it lists matches. Extra keys are
  fine: the Rust core sends a few additions, such as `Member.role`.
- An expected array must have the same length, and each element must match.
- Wildcards: `"<any>"`, `"<string>"`, and `"<len:N>"` for an array of N items.
- `"${name}"` and `"${name.N}"` are replaced by a saved value, in `args` and in
  `expect`. Only whole strings are replaced.
- `"answer": { "positions", "words", "wrong"? }` replaces `args` for
  `checkRecoveryWords`, because challenge positions are random. The runner
  sends `{ answers: { [p]: words[p - 1] } }` for each 1-based position `p`,
  with `"zzzz"` instead at the indexes (into `positions`) listed in `wrong`.
- `{ "ok": null }` is a `void` result. A rejected promise is `{ "error": CoreError }`.

## TS runner (plan, for the UI owner)

1. Add `ui/src/core/conformance.test.ts` (vitest is already set up). Import
   the fixture through a relative path, or copy it into `ui/src/test/` in a
   prebuild step, so the file stays single-sourced.
2. For each scenario with `"mock"` in `targets`, create
   `new MockCore({ speed: 0, storage: undefined })` per peer, and record
   events with `core.on(name, …)` for every `CoreEventName`.
3. Map a step onto the method: `args` values in the TS parameter order. A
   per-command table, typed against `CoreCommands`, keeps it honest:
   `{ listChannels: (a) => [a.serverId], createServer: (a) => [a.input],
   kickMember: (a) => [a.serverId, a.userId], … }`.
4. `await` the method. Wrap a resolve as `{ ok: value ?? null }` and a
   rejection as `{ error }`, then apply the matching rules above. The
   matcher is about 40 lines; port `matches` from `tests/conformance.rs`.
5. Fail if a scenario lists `"mock"` but some step names a command the mock
   lacks (`grantRole`, `revokeRole` and `syncNow` are Rust additions).

The same runner later drives the real core through the shell adapter (Tauri
`invoke` or Electron IPC), using `targets: ["rust"]` scenarios too. That turns
it into an end-to-end check of the adapter.

## Where the two differ today

Each difference keeps a scenario Rust-only. Fix the mock, then add `"mock"`.

| Difference | Fix |
| --- | --- |
| The mock's word list is 64 stand-in words, with no checksum | Give the mock the BIP-39 English list and a checksum check (or call into a WASM build of `identity`) |
| The mock is one person; `joinServer` adds a canned server | Multi-peer scenarios stay Rust-only until the mock simulates peers |
| The mock lets only the owner kick | `authority`'s rule: moderators and up, against lower ranks. The mock should follow it |
| The mock may not follow the B19 §6 challenge exactly (#28) | The Rust core: `getRecoveryChallenge` gives 3 stable positions. `checkRecoveryWords` takes exactly those (else `not-allowed`) and answers `{ ok }` or `{ ok: false, wrong }`. The 5th failure burns the challenge and rejects with `too-many-attempts`, until `getRecoveryWords` succeeds. All of it survives a restart. The mock runner needs the `answer` directive too |
| The mock's `getRecoveryWords` answers at any time | The Rust core answers only until the check passes, then `not-allowed` (until the app lock, #31); the mock should too |
| The mock accepts any `updateProfile` input | The Rust core refuses oversize fields and any avatar other than a base64 PNG, JPEG, WebP or GIF `data:` URL (`bad-request`) |
| The mock has no `grantRole` / `revokeRole` | Add them to `CoreCommands` (proposed below) |
| The mock never holds a message for its key | The Rust core (see "Messages held for their key" below): a plain member without the channel key yet gets the message back at once as `pending`, with a provisional ID, instead of `not-allowed`. The mock should do the same when it simulates a fresh join |

## Proposed contract additions (Rust has them; `api.ts` doesn't yet)

- Restore servers, phase A (#291, `src/backup.rs`). No new command or event.
  - **When the backup is written:** only when my server set changed since the last write, on the first sync round after start (login), or every 24 h (`BACKUP_REPUBLISH_MS`). Then my server list (server IDs and sync secrets) is merged into the newest backup the holders keep (`Network::fetch_backup`), sealed as the next version and handed to them (`Network::put_backup`). Rounds with nothing due fetch and put nothing. The fetch and the put run outside the core lock.
    - Servers this device listed and no longer has get a tombstone.
    - Merging keeps what my other devices wrote.
  - **The blob:** `mvsb1 ‖ tag ‖ w_pub ‖ version ‖ nonce ‖ XChaCha20-Poly1305(padded JSON) ‖ Ed25519 sig by w`.
    - The keys come from the words (#292). There's no identity key in it.
    - It's padded to 4 KiB steps, at most 64 KiB.
  - **On `restoreIdentity`:** the core fetches the backup (`Network::fetch_backup`), keeps the newest valid one, and rejoins each live server as the same member (#290). It posts no new join, profile or age claim.
    - A server whose log no longer lists me isn't entered.
    - Servers nobody could be reached for are retried each sync round.
  - **Holders:** the ports are no-ops until dev-sync's `/mvs/backup/1` and devops' mailbox exist. `LoopbackNetwork` holds them for tests.

- `signOut({ words: string[] } | { withoutWords: true })` returns `null` (both read at the top level of `args`). It removes this identity from this computer.
  - `words` must be this identity's 12 words: `invalid-words` (with `positions`) for unknown words, `not-allowed` for another identity's. `withoutWords: true` is the explicit "I accept losing this account" path. Neither: `bad-request`. No identity: `not-found`.
  - In order, fail safe:
    1. A wipe marker (the intent) is written beside `core.db`. If it can't be written: `internal`, and nothing changed.
    2. The keychain secret is deleted. If that fails: `keyring-unavailable`, the marker is removed, and nothing changed. Once the secret is gone, a failure to delete the recovery-check entry doesn't stop the sign-out.
    3. The local database is emptied and servers are closed. The core answers and exits, and the shell restarts it.
  - At the next start, before anything opens, with the marker present:
    - **The keychain says the secret is gone:** `core.db` and the sync folder are deleted, then the marker. That includes its `node.key`, so a new identity here gets a new peer ID. On failure the core exits and retries at the next start.
    - **The secret is still there:** only the marker is cleared.
    - **The keychain can't be read:** nothing is deleted, the marker stays, and the core starts offline, never with the old node key.
  - The app then starts at "create or restore". No new event: the UI goes back to onboarding on `ok` (pending requests fail with `core-restarted`).

- `updateProfile` with a new display name (trimmed) posts a new profile post to every server I'm still a member of (not kicked, not left, not closed); the latest one wins for everyone. The same name, or a new bio or avatar only, posts nothing: bio and avatar stay on this device, since the profile post carries the name (and caps) only. A server that refuses the post is skipped; the call still succeeds. Errors as before: `bad-request` for a bad name, bio or avatar, `not-found` with no identity.

- Age claims and the 30-day rule (decision 18, B13 §3, B7 §9). Each claim is decided from my latest accepted claim in that server:
  - A birth date that raises me from "not adult" to adult posts nothing. It sets `Identity.pendingBirthDate: { date, effectiveAt }` (`effectiveAt` = when found + 30 days). Until then I count as under 18: the core refuses 18+ joins (`age-blocked`), and servers keep my earlier claim.
  - When `effectiveAt` passes, the next sync round posts "adult" to every server I'm still in, clears `pendingBirthDate`, and emits `identity.changed`.
  - A lowering is posted at once. A claim equal to the one the server holds isn't posted again.
  - So a restore (`restoreIdentity(words, input)`) can't skip the 30 days: the rejoin compares against the claim the server already holds.
  - A raise means a *changed* date. A date entered at sign-up on this computer, or one a server's **"adult"** claim already agrees with, is the one my claims come from. A "not adult" agreement proves nothing: a restored date a day short of 18 still waits. Turning 18 with it is a birthday, with no wait.
  - **Known limit,** until the encrypted backup (#291) brings the old birth date back: after a restore with a new adult date, a server I join for the first time has no earlier claim to compare, so it gets "adult" at once. Joining a fresh server first doesn't lift the wait on the old ones: only an agreeing claim or the 30 days do. The client clock, or a modified client, can also skip the wait (Security Lead, GAP-22).

- Restore on a new computer:
  - `restoreIdentity(words, input?: { locale, country, birthDate })`. The words don't carry the country or birth date, so the UI asks them again (with the agreement) before the words and passes them here, as `createIdentity` takes them. The name follows with `updateProfile`.
  - Rejoining a server I'm still a member of (its invite link, after a restore) signs no new join. It never posts an empty display name or an age claim without a birth date: the server keeps the profile and claim it holds. With no birth date it isn't age-blocked either: my earlier claim stands. A known age below the server's minimum still blocks.

- `grantRole(serverId, userId, role: 'admin' | 'moderator' | 'member')` and
  `revokeRole(serverId, userId)`.
- `Member.role: 'owner' | 'admin' | 'moderator' | 'member'`.
- Voice calls (voice v1, piece B; `src/voice.rs`). IDs are 64 lower-case hex, and `identity` is a user ID.
  - Calls happen only in each server's one voice room, whose ID is derived: `blake3::derive_key("mvs/voice-room/v1", serverId)`. There is no channel record for it, no 18+ gate, and text channels carry no calls.
  - `getVoiceRoom(serverId)` returns `VoiceRoom { serverId, roomId, participants: { userId, muted }[], relayOnly }`.
    - Participants: me first when I'm in it, at most 5 (the first to arrive), built from verified hellos only, each dropped 30 s after we last received one.
    - Errors: `not-found` for a server I don't have (a kicked member's core removes it), `not-allowed` if I'm not a member or I'm kicked.
  - `joinCall(channelId)` takes the room ID and returns `CallState { channelId, participants: { identity, muted }[], full, relayOnly }`, me first; `channelId` is the room ID. It leaves any other call first.
    - Errors:
      - `not-found`: an unknown ID.
      - `not-allowed`: a text channel's ID, not a member, or kicked.
      - `call-full`: 5 are already in.
      - `relay-only`: relay-only is on, so calls are off.
      - `in-call-elsewhere`: my own identity is already in this room from another computer. One computer per call until linked devices (phase 3).
      - `not-implemented`: no voice transport (sync's piece A).
  - `leaveCall()` returns `null` and is idempotent. The app lock and quitting leave too.
  - `setMuted(muted: boolean)` returns `null`. `not-allowed` outside a call.
  - `sendSignal(to, kind: 'offer' | 'answer' | 'ice', payload: string)` returns `null`.
    - Errors:
      - `not-allowed`: not in a call, or `to` isn't in it.
      - `bad-request`: an unknown kind, a payload over 12 KiB, or an offer or answer whose SDP has no `a=fingerprint`.
      - `not-found`: a malformed `to`.
  - Events:
    - `call.state` (a `CallState`) on every change to my call: join, leave, mute or timeout. After a leave, or a refusal by a full call, its `participants` is `[]`.
    - `call.signal { from, kind, payload, fingerprintVerified: true }`.
    - `voice.room` (a `VoiceRoom`) when a room's roster changes (join, leave, mute, timeout, kick), whether or not I'm in it. At most one per room per sync round, and only while I'm a member. It's a hint: re-read with `getVoiceRoom`. `muted` is a display hint.
  - It fails closed. A core-to-core envelope is signed by the sender's identity key under `mvs/voice-signal/v1`, and the signature covers the SDP and its fingerprint. The core drops, without an event:
    - an envelope that doesn't verify or isn't `v: 1`;
    - one outside ±120 s of our clock;
    - a replayed nonce (up to 4096 remembered; when the cache is full, new envelopes are dropped);
    - one from someone who couldn't join that call themselves;
    - an offer or answer without a fingerprint.
  - Presence: a hello every 10 s, a bye on leaving, and a participant dropped after 30 s of silence.
    - Hellos and byes go only to members who could join that call themselves, so a call in an 18+ channel isn't shown to anyone else.
    - They also go to my own identity, which reaches only my other computers (same recovery words). A hello or bye signed by me counts only when sync bound its sender to me and it has no `to`. It marks the room "in a call elsewhere" until its bye or 30 s without a hello, and never appears in the roster. My own envelopes echoed back are replays.
    - A full call's refusal counts only if it comes from someone already in that call, within 10 s of joining.
  - `applyUpdate` answers `not-allowed` during a call.
- Relay-only (GAP-6r), per device:
  - `setRelayOnly(on: boolean)` returns `null`. It's stored in the core database and needs no identity. On:
    - the network keeps no direct listener, makes no direct dials, and reaches peers only through relay circuits, and direct connections already open are closed (#274);
    - my call ends at once (a bye to the others, then a `call.state` with `participants: []` and `relayOnly: true`);
    - `joinCall` answers `relay-only` until it's off.
  - At start, `mvs-core` reads the stored value before the network starts, and the node starts relay-only from the first moment (`NodeConfig.relay_only`: no direct listener, no bootstrap dial, no punching). It fails closed: a database that can't be opened or read, or a value other than off, means on.
  - Shown up front:
    - `AppInfo.relayOnly` from `getAppInfo`;
    - `VoiceRoom.relayOnly`, so the UI can disable "Join" before trying;
    - `CallState.relayOnly`.
  - After a change, one `voice.room` per server at the next sync round carries the new value.
- `CoreError.code`, voice: `'call-full' | 'relay-only' | 'in-call-elsewhere'`.
- Invites (#208), all moderator and up (`not-allowed` below that):
  - `createInvite(serverId, input?: { expiresMs?: number, maxUses?: number })`
    returns `{ id, link, expiresAt?, maxUses? }`. `expiresMs` is the last time a
    join through it is accepted (ms since epoch). An absent `expiresMs` means
    never, and `bad-request` if it isn't in the future. An absent `maxUses`
    or 0 means unlimited.
  - `listInvites(serverId)` returns
    `{ id, createdBy, expiresAt?, maxUses, uses, revoked }[]`, oldest first,
    revoked ones included. `maxUses` 0 is unlimited.
  - `revokeInvite(serverId, inviteId)` is allowed for the invite's author or
    someone ranked above it (`not-allowed` otherwise). `not-found` for an
    unknown or malformed ID, or an invite already revoked.
  - A create or a revoke fires `server.updated`, since the server's
    `inviteLink` may then name another invite (or be empty once none is
    left open).
  - Short links (T8-02, `docs/design/short-invite-link.md`):
    - `createInvite` also returns `shortLink`, and `listInvites` returns it for
      an invite I made that is still live; it is absent otherwise. A short link
      is `mvs://join/` plus at most 51 base58 characters, **case-sensitive**.
    - `joinServer` accepts both formats. A short link that finds nobody online
      is retried like any other join.
    - **The UI must not show `shortLink` until release N+1.** Peers on older
      releases can't parse it, so it would read as invalid to them. Until
      then the UI keeps showing the v0 `link` / `inviteLink`. From N+1,
      `createInvite`'s `link` itself becomes the short form.
    - A short link stops working when its invite is revoked, expires or is
      used up, and when the server is closed, left or purged.
- `Member.connection: 'direct' | 'relay' | null` (#187): how we reach an online member, a direct connection (LAN) or through the relay; `null` for us and for members who are offline. A change fires `members.changed`.
- Messages held for their key (#285 follow-up). A plain member who just joined has no channel key until a moderator's re-key reaches them (usually well under a second).
  - `sendMessage` then resolves at once with `{ ..., id: 'pending-<16 hex>', sendState: 'pending' }`, and fires `message.added` with it. Nothing is sealed or sent yet.
    - The message is listed after the channel's stored messages (`getMessages`, `getMessageCount`).
    - It lives in memory only: a restart drops it.
    - At most 50 are held per channel at once; past that, `not-allowed`. Failed ones are bounded too: beyond 50 failed in a channel the oldest are dropped (the UI may still show them until it re-reads the channel).
  - When the key comes, the core's background sync seals and sends it, and fires `message.updated` with the real message: its record `id`, `sendState: 'sent'`, and **`replaces: 'pending-…'`**. From then on it is an ordinary message (`sent`, then `received`).
    - **Its `index` may differ from the pending entry's.** The same round can bring in other messages first: the key can make earlier messages readable, and others may have written meanwhile. These come as their own `message.added` events, before the replacement. The real message is placed after them, and any other held messages after it.
    - So on `replaces`, the UI drops the entry with that `pending-…` ID (wherever it sits) before applying the message at its `index`. It should also re-read `getMessageCount` (or the visible window) rather than keep its own count.
  - With no key after **2 minutes**, or if I can no longer send there (kicked, left, closed), it turns failed:
    - `message.updated` with the same `pending-…` ID and `sendState: 'failed'`;
    - and `notice: rekey-waiting` for the server.
    - It stays listed, failed, until a restart or `discardMessage(messageId)`, which drops a held or failed `pending-…` message and wipes its text (`not-found` for any other ID). "Send again" is `discardMessage` plus a new `sendMessage`.
  - `Message.replaces?: string` is new, and appears only on that one `message.updated`.
  - A moderator or above never gets a pending message: they make the key on the way. A non-member, or someone kicked, still gets `not-allowed` at once.
- `CoreError.code`: `'bad-request' | 'not-implemented' | 'internal'`, and `'keyring-unavailable'` (QA-20): the OS keychain (Secret Service on Linux) is missing or unusable. The core still starts; `getIdentity` and anything needing the identity reject with it, and the UI shows the keyring cause.
- `NoticeKind`: `'rekey-waiting'`, sent when removed members still hold a channel key and only a moderator can renew it. Until a moderator does, sending there answers `not-allowed`.
- Server lifecycle (#229 design, #251 authority; `docs/design/server-rename-leave-delete.md`):
  - `renameServer(serverId, name)` returns the updated `ServerSummary` and fires `server.updated` (here, and for every member when they read it). The name is trimmed, then must pass the strict server-name rule (1–100 code points, no bidi, control or zero-width characters). The owner or an admin only.
  - `leaveServer(serverId)` returns `null`. Any member but the owner (who deletes instead). **It never answers `update-needed`.** The server leaves this device at once and `server.removed { serverId, reason: 'left' }` fires.
    - The signed `Leave` waits in a durable outbox, which survives restarts. Only the server's sync secret and its control subscription stay.
    - Each sync round it goes out once the rollout gate is open, and is retried until a member has it (B6 `sent`). Then the secret and subscription go too, and the others re-key.
    - If I'm kicked first, the `Leave` is dropped.
    - The leaver isn't banned, and comes back through a new invite as a plain member.
  - `getUpdateNeeded(serverId)` returns the user IDs of members whose latest profile post lacks `server-lifecycle-v1`, or who have none: who blocks a rename or a delete. An empty list means the gate is open.
  - `closeServer(serverId)` returns `null`. The owner deletes the server for good: the core syncs first, then sends the close.
    - On the owner's core, and on every member's when it reads the close, the server is closed, not purged at once. `server.closed { serverId }` fires; any call there ends; its keys and sync subscription go, and it's no longer synced. It stays in `listServers`, read-only, with `ServerSummary.closed: true` (additive).
    - 30 days (`CLOSED_RETENTION_MS`) after the close was received, the rest is purged and `server.removed { serverId, reason: 'closed' }` fires. Its genesis and close record are kept forever, so the server reads as closed, never unknown.
    - An old invite, from someone who was in it or a stranger a stale peer still serves, gives `join.status` `{ phase: 'closed', link }`.
  - Purge (a leave, or a closed server's 30 days): the server's records, sync secret, channel keys, sync subscription, and any call in it. "Removed from your devices", not erased: other members keep their copies.
  - `server.removed` gains an optional `reason: 'left' | 'closed'`. It's absent for a kick, as before.
  - Errors:
    - `not-found`: a server I'm not in.
    - `not-allowed`: not permitted (role), or the owner leaving.
    - `bad-request`: a name that fails the rule.
    - `update-needed` (rename and close only): a current member's latest profile post doesn't carry `caps: ['server-lifecycle-v1']`, or they have no profile post. Nothing was signed. An older member would refuse the record and stall on every control record after it.
  - Profile posts now carry `caps: ['server-lifecycle-v1']`, and older versions ignore the field. A core re-posts its profile once where its latest post lacks the cap.
- `syncNow()`: test-only until `sync` drives updates. It shouldn't ship in
  the UI.

## In-app update (B8)

Built in B8-5 (`src/update.rs`, tests in `tests/update.rs`), from
`core/updater/docs/B8-in-app-update.md`, approved by the tech lead and the
security lead. The `updater` crate does all the work (signatures, root
chain, policy, hash, size, unpack, state, lock); `core/api` only wires it in.

**Startup.** `mvs-core --update-root <install folder>`. The shell passes it
only when packaged and the root checks out (§0: derived from
`process.execPath`, and `mvs-launcher` must exist). Without it, updates are
off: each command below answers `not-implemented`, no timer runs, and the
core never contacts the update host. Updates also stay off in a build
without `MVS_APP_VERSION` (set by release.yml; the running version). The
state folder is `updater::UpdateDirs::default_state_dir()`.

**Commands.** All four are main/core-only. None is in `CoreCommands`
(`ui/src/core/api.ts`) or in the renderer allow-list
(`desktop/src/commands.js`), per A2. The renderer keeps only
`restartToUpdate`, which main handles itself and turns into `applyUpdate`
plus a relaunch through the launcher.

| Command | Result `ok` | Errors (`code`) |
| --- | --- | --- |
| `checkForUpdate()` | `{ "status": "up-to-date" \| "available" \| "staged", "current": "1.2.0", "available": "1.3.0" \| null, "freshness": "fresh" \| "stale" \| "never" }` | `offline`, `invalid-update` (any verification failure; details only in the log), `check-clock` (`IssuedInFuture`, or every manifest expired) |
| `downloadUpdate()` | `{ "version": "1.3.0" }` once it is verified, unpacked and staged | `nothing-to-download`, `offline`, `invalid-update`, `disk-full` |
| `applyUpdate()` | `{ "version": "1.3.0" }`: main may now relaunch through the launcher | `nothing-staged`, `not-allowed` (in a voice call or screen share; `update.ready` then has `waitingForCall: true`) |
| `markHealthy()` | `{ "committed": "1.3.0" \| null }` (`null` when the running version was already committed) | `not-allowed` (not a trial boot) |

**Events.**

| Event | Payload | When |
| --- | --- | --- |
| `update.ready` (exists: `UpdateInfo`) | `{ "version": "1.3.0", "waitingForCall": false }` | after a stage, and again when a call ends |
| `update.installed` (new) | `{ "version": "1.3.0", "notes": "plain text" \| null }` | once, after `markHealthy` commits a new version; `notes` comes from `ManifestBody::notes_for(<UI language>)`. The UI shows it as **text only** |
| `update.rolledBack` (new) | `{ "failed": "1.3.0", "running": "1.2.0" }` | once, on the first healthy start after the launcher rolled back |

**Rules the core keeps:**
- The first check runs once the UI is up (the first `markHealthy`, so the
  UI hears `update.ready`), or 2 minutes after start if that never comes.
  Then every 6 h ± 30 min (jitter), and at once after `setUpdateChannel`.
  Each check downloads whatever is available. Downloads are never paused
  for a call: only the apply step waits. Check and download errors go to
  the log (`RUST_LOG`), never to the UI. An already staged version is
  announced again (`update.ready`) on every check.
- All state goes through `UpdateLock`, `save_state` and `write_atomic`.
- Before any schema migration, it writes `core.db.pre-<schema>` (owner-only)
  and records it in `db-snapshot.json`. It deletes the snapshot when
  `markHealthy` commits (§0). TODO: `db.rs` has no versioned migrations
  yet; the first one calls `Updater::record_db_snapshot`.
- The UI channel maps to the manifest channel: `stable` → `alpha` during the
  closed alpha, `beta` → `beta`. Each has its own fixed-tag release,
  `update-<channel>`. Switching channel never downgrades: the running
  version is the floor.
- `update.installed` carries the notes in the profile's language (`he`
  when there is none yet), falling back as `ManifestBody::notes_for` does.

**`api.ts` changes for dev-ui:**
- add `update.installed` and `update.rolledBack` to `CoreEvents`;
- add `'offline' | 'invalid-update' | 'check-clock' | 'nothing-to-download' | 'nothing-staged' | 'disk-full'` to `CoreError.code`, for when main forwards them.
