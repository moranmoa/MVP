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
- `Member.connection: 'direct' | 'relay' | null` (#187): how we reach an online member, a direct connection (LAN) or through the relay; `null` for us and for members who are offline. A change fires `members.changed`.
- `CoreError.code`: `'bad-request' | 'not-implemented' | 'internal'`.
- `NoticeKind`: `'rekey-waiting'`, sent when removed members still hold a channel key and only a moderator can renew it. Until a moderator does, sending there answers `not-allowed`.
- `syncNow()`: test-only until `sync` drives updates. It shouldn't ship in
  the UI.

## Proposed: in-app update (B8)

From `core/updater/docs/B8-in-app-update.md`, approved by the tech lead and
the security lead. A proposal from dev-updater, for dev-identity to build
after M1. The `updater` crate does the work (verify, unpack, state, lock),
and `core/api` only wires it in.

**Startup.** `mvs-core --update-root <install folder>`. The shell passes it
only when packaged and the root checks out (§0: derived from
`process.execPath`, and `mvs-launcher` must exist). Without it, updates are
off: each command below answers `not-implemented`, and no timer runs. The
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
- It checks at start and then every 6 h ± 30 min (jitter), and downloads as
  soon as something is available. Downloads are never paused for a call:
  only the apply step waits.
- All state goes through `UpdateLock`, `save_state` and `write_atomic`.
- Before any schema migration, it writes `core.db.pre-<schema>` (owner-only)
  and records it in `db-snapshot.json`. It deletes the snapshot when
  `markHealthy` commits (§0).
- The UI channel maps to the manifest channel: `stable` → `alpha` during the
  closed alpha.

**`api.ts` changes for dev-ui:**
- add `update.installed` and `update.rolledBack` to `CoreEvents`;
- add `'offline' | 'invalid-update' | 'check-clock' | 'nothing-to-download' | 'nothing-staged' | 'disk-full'` to `CoreError.code`, for when main forwards them.
