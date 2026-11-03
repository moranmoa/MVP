# api: the MVS core API

What the UI calls. It mirrors `CoreApi` in `ui/src/core/api.ts`, with the types
from `ui/src/core/types.ts` (serde, camelCase JSON). It doesn't depend on any
shell: Tauri, Electron or plain IPC only forward JSON.

```rust
let core = Core::open(Config { db_path, app_version }, Ports {
    secrets: Arc::new(OsKeychain::new()),  // identity::OsKeychain
    network: Arc::new(LoopbackNetwork::new()),  // TODO: sync adapter
    events: Arc::new(MySink),              // forwards Event JSON to the UI
    clock: Arc::new(SystemClock),
    cipher: Arc::new(PendingGroupKeys),    // TODO: groupkeys adapter
})?;
let reply: String = core.handle(r#"{"cmd":"listServers"}"#);  // {"ok":[...]}
```

## JSON boundary

| | Shape |
| --- | --- |
| Request | `{ "cmd": "<CoreCommands method>", "args": { <TS parameter name>: value, … } }` |
| Reply | `{ "ok": <result or null> }` or `{ "error": { "code", "positions"? } }` |
| Event | `{ "event": "<CoreEvents name>", "payload": … }`, through `EventSink` |

A command emits its events only after its change is stored, and never while
the core holds its lock, so a sink may call back into the core. The core is
`Send + Sync`, and commands run synchronously, so a shell wraps them in its
own async layer.

## What works

| Area | Commands | Backed by |
| --- | --- | --- |
| Identity | `getIdentity`, `createIdentity` (with progress events), `checkRecoveryWords`, `getRecoveryWords` (only until the check passes, then `not-allowed` until the app lock, #31), `restoreIdentity`, `updateProfile` (length caps; the avatar must be a base64 PNG, JPEG, WebP or GIF `data:` URL) | `identity::Vault` in the OS keychain; the profile is in SQLite, sealed with the identity's storage key |
| Servers | `createServer` (after the recovery check), `listServers`, `listChannels`, `listMembers`, `joinServer`, `cancelJoin` | Genesis, invite and join records (`records`); roles from one `authority::Resolver` per server, loaded once and fed record by record |
| Records | — | Only records the resolver has joined are stored, used as heads (at most 64, the latest) or read for members. Records waiting on deps are held in memory, bounded by the resolver's pending limit; what it evicts is dropped, and sync redelivers it |
| Moderation | `kickMember`, plus additions `grantRole` and `revokeRole` | `authority` rules. Dry run on the live resolver: a record it does not accept is refused (`not-allowed`), never stored or sent, and the log is rebuilt from the store. TODO(records B4): `Resolver::would_accept` |
| App | storage usage and limit, `clearCache`, `exportDiagnostics` (counts only), `getAppInfo`, `setUpdateChannel`, `restartToUpdate`, `blockUser`, `markRead` | SQLite |
| Not yet | `getMessageCount`, `getMessages`, `sendMessage`, `editMessage`, `deleteMessage`, `retryMessage` | Answer `not-implemented`. Next step, on top of `groupkeys` |

## Formats (frozen for the alpha)

- **Server:** `authority::sign_genesis`. Its body is the 16-byte nonce
  followed by JSON `{v: 1, name, ageRestriction}`. The server ID is
  `records::ServerId::derive(owner, nonce)`, so `records` refuses a genesis
  that anyone but the owner signs for that ID.
- **Invite link (not frozen):** `mvs://join/<server id hex>/<invite record id hex>`.
  `createServer` signs a first invite (no expiry, no use limit) right after
  the genesis; the link shows a usable invite, mine first. TODO(#46): a
  secret in the link.
- **Default channel:** `"general"`, ID `derive_key("mvs 2026-11 default channel id v1", server id)`.
- **Join:** `authority::Action::Join { invite }`, with the invite among its
  deps. `authority` refuses it for an unknown, revoked or used-up invite, and
  then nothing is stored or sent.
- **Display name:** a control-log `Post` whose text is
  `{"mvs":"mvs/profile/v1","displayName":…}`, signed after the join (posts
  need a member). The latest one per member wins.
- **Members:** `authority`'s member set (the owner, and accepted joins, minus
  kicks).
- **Short code:** 8 characters, `XXXX-XXXX`, from `derive_key("mvs 2026-11 short code v1", key)`.

## Ports, and what plugs in later

| Trait | Today | Later |
| --- | --- | --- |
| `Network` | `LoopbackNetwork`, in-process, for tests. `syncNow` pulls from it | TODO(sync, PR #5): push and pull through `sync`; `syncNow` goes away |
| `GroupCipher` | `PendingGroupKeys`: never claims a renewal, so the kick notice isn't sent | TODO(groupkeys, PR #9): rotate on kick, encrypt messages, emit `notice` |
| `EventSink` | `CollectingSink` for tests | The shell's emitter |
| `SecretStore` | `identity::MemoryStore` in tests | `identity::OsKeychain` |

Other TODOs in the code: app-lock commands, and the birth date restored from
the encrypted backup. Display names in profile posts are readable by anyone
holding the records until control-log content is encrypted.

## Tests

```
docker run --rm -e CARGO_BUILD_JOBS=4 -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm cargo test --release -p api
```

Run from `core/`.

- `tests/flows.rs`: create identity → recovery check → create server → two
  peers join → grant → moderator kicks → the kicked peer gets
  `server.removed` and can't rejoin. Also covered:
  - a forged genesis is refused;
  - the age gate;
  - state survives a restart, and the birth date is never stored in the clear;
  - dispatcher errors;
  - diagnostics hold no secrets.
- `tests/conformance.rs`: the shared JSON fixture. See `CONFORMANCE.md`.
