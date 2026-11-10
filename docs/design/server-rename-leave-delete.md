# Design note: rename, leave and delete a server

Status: draft for the Tech Lead and the security lead. Author: dev-identity. No code yet.

## Where we start
- `records` already defines the three kinds (`core/records/src/lib.rs`):
  - `ServerEdit = 6`: body `history u8` (0) then metadata, up to 2 KiB;
  - `ServerClose = 7`: up to 64 B;
  - `Leave = 14`: body is a cut.
- `authority` never accepts them. `Action::from_record` returns `None` for all three (`core/authority/src/lib.rs`, "come in B4-3").
- The api has no command for them. `Db::remove_server` (used on a kick) deletes only the `servers` row. Records, keys and sync subscriptions stay.

## The records
| Action | Record | Body | Who may | Accepted when |
| --- | --- | --- | --- | --- |
| Rename | `ServerEdit` | `0u8` ‖ JSON `{v:1, name}` | owner or admin | the author has the role at the record's deps; the name follows the `createServer` rules |
| Leave | `Leave` | a cut, empty today like a kick's (B4-4 TODO) | any member except the owner | the author is a member and not kicked |
| Delete | `ServerClose` | empty | the owner only | the author is the genesis author |

- The latest `ServerEdit` in authority order sets the name. The age restriction stays as the genesis set it. Changing it would change who may read, so it is out of scope.
- The owner cannot leave: v1 has no ownership transfer, so the owner deletes instead (`not-allowed`).
- After `ServerClose`, authority refuses every later control or channel record of that server. The server is closed for good, and nobody can reopen it.
- A leaver isn't banned. Unlike a kick, they can join again with a new invite.

## What other members see
- **Rename:** `server.updated` with the new name. The rail and the header update.
- **Leave:** `members.changed`, plus an "X left" line in the member list. Messages the leaver already sent stay; they were signed and shared.
- **Delete:** `server.removed { serverId, reason: "deleted" }` and a one-time notice: "The owner deleted this server". Members offline at the time learn it on their next sync: peers keep serving the close record.

## Keys and local data
- **Leave, on the remaining members:** the same path as a kick (M2). The first moderator or above to read the `Leave` rotates every channel key the leaver held.
  - This happens at once and isn't batched (GAP-8 batches only joins).
  - With no moderator online, it's `rekey-waiting` as today, and sending answers `not-allowed` until one rotates.
- **Leave and delete, on the leaver's own device, and on everyone's device after a delete:** purge the server. That means:
  - close its sync logs (gossip unsubscribe, drop pending records);
  - delete its records, invites, profiles and channel keys (`GroupCipher::forget`);
  - drop any call in it;
  - then emit `server.removed`.
- **Delete:** no rotation is needed, since nothing after the close is accepted.
- **Honest limit:** a delete or leave isn't erasure. Any member may keep copies of what they already received. The UI wording must say "removed from your devices", not "erased".

## Compatibility
Clients up to 0.2.x refuse these kinds. Control records name the latest control heads in their deps, so an old client would also refuse every control record after a rename or leave, and its view would stall. So the rollout takes two releases:
1. **Release N:** accepts and renders all three kinds, but the api answers `not-implemented` to the new commands.
2. **Release N+1:** sends them. It ships only once the updater's minimum version is at least N.

## Pieces (in order)
1. **authority** (owner: Tech Lead / dev-records): `core/authority/src/lib.rs`.
   - Add `Action::ServerEdit{name}`, `Leave{cut}` and `ServerClose`.
   - Change `from_record`, the resolver's role checks, and `State`: name override, closed flag, leavers removed from `members`.
   - Tests in `core/authority/tests/`.
   - `core/records/src/lib.rs` needs no change.
2. **api, accept side** (dev-identity):
   - `core/api/src/model.rs`: the summary name comes from the latest `ServerEdit`.
   - `core/api/src/service.rs`: purge on a read close or my own leave; rekey on a read `Leave`.
   - `core/api/src/db.rs`: `purge_server`.
   - `core/api/src/crypto/mod.rs` and `core/api/src/ports.rs`: `GroupCipher::forget`, `Network::close_server`.
   - `core/api/src/types.rs`: an additive `reason` field on `ServerRemoved`.
   - Flows tests in `core/api/tests/flows.rs`, and `core/api/CONFORMANCE.md`.
3. **sync** (dev-sync):
   - `core/sync/src/node.rs`: a `Command::CloseServer` that unsubscribes and drops pending records.
   - `core/sync/src/store.rs`: delete a log's records.
   - `core/api/src/syncnet.rs`: the `close_server` port.
4. **api, send side** (dev-identity, release N+1):
   - `renameServer(serverId, name)` → `ServerSummary`, `leaveServer(serverId)` → `null`, `deleteServer(serverId)` → `null`, with errors `not-allowed`, `not-found`, `bad-request`.
   - Files: `service.rs`, `dispatch.rs`, `CONFORMANCE.md`, `tests/flows.rs`.
5. **UI** (dev-ui / dev-ui-settings):
   - `ui/src/core/api.ts` and `ui/src/app/store.ts`.
   - `ui/src/screens/shell/ServerSettings.tsx`: a rename field, and delete with a typed-name confirm.
   - `ui/src/screens/shell/ServerRail.tsx` with `ui/src/components/ContextMenu.tsx`: "Leave server".
   - Strings in `ui/src/i18n/en.json` and `he.json`; tests in `ui/src/test/`.
6. **QA:** a two-peer smoke covering:
   - a rename seen by the other peer;
   - a leave that rotates keys, after which the leaver reads nothing new;
   - a delete that removes the server on both peers, including one that was offline at the time.

## Open questions for review
- Should a member be able to leave silently, with no `Leave` record and only a local purge? This proposal says no: without the record, nobody rotates keys.
- How long peers keep a closed server's control log: until every member has acknowledged it, or for a fixed 30 days?
