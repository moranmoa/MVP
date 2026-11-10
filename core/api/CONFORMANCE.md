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

## Proposed contract additions (Rust has them; `api.ts` doesn't yet)

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
    - A full call's refusal counts only if it comes from someone already in that call, within 10 s of joining.
  - `applyUpdate` answers `not-allowed` during a call.
- Relay-only (GAP-6r), per device:
  - `setRelayOnly(on: boolean)` returns `null`. It's stored in the core database and needs no identity. On:
    - the network keeps no direct listener, makes no direct dials, and reaches peers only through relay circuits. Closing direct connections that are already open is sync's GAP-6r blocker 1;
    - my call ends at once (a bye to the others, then a `call.state` with `participants: []` and `relayOnly: true`);
    - `joinCall` answers `relay-only` until it's off.
  - At start, `mvs-core` reads the stored value before the network starts and the node starts with it.
  - Shown up front:
    - `AppInfo.relayOnly` from `getAppInfo`;
    - `VoiceRoom.relayOnly`, so the UI can disable "Join" before trying;
    - `CallState.relayOnly`.
  - After a change, one `voice.room` per server at the next sync round carries the new value.
- `CoreError.code`, voice: `'call-full' | 'relay-only'`.
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
