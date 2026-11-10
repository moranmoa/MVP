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
| Rename | `ServerEdit` | `0u8` ‖ JSON `{v:1, name}` | owner or admin | the author has the role at the record's deps; the name passes the strict `createServer` rule |
| Leave | `Leave` | a cut, empty today like a kick's (B4-4 TODO) | any member except the owner | the author is a member and not kicked |
| Delete | `ServerClose` | empty | the owner only | the author is the genesis author |

- **Rename:**
  - The latest `ServerEdit` in authority order sets the name.
  - Its JSON ignores unknown fields, so later versions can add icon or description, but `v` must be 1.
  - The name keeps the strict rule; a name that fails it makes the whole record refused.
  - The age restriction stays as the genesis set it. Changing it would change who may read, so it is out of scope.
- **Leave:**
  - The owner cannot leave: v1 has no ownership transfer, so the owner deletes instead (`not-allowed`).
  - Leaving also ends any granted role (admin or moderator), just as a revoke would. Someone who rejoins comes back as a plain member.
  - A leaver isn't banned. Unlike a kick, they can join again with a new invite.
- **Delete, the rule for `ServerClose`** (deterministic on every peer):
  - Once a close is accepted, only records in its causal past (its deps, transitively) stay accepted. Anything concurrent with it or after it is refused, including records accepted before the close arrived. The resolver drops them as it does past a kick's cut.
  - If there are two closes, the first in authority linear order wins, and later ones change nothing.
  - So the owner's core syncs before signing the close, to put recent heads in its past.
  - The server is closed for good, and nobody can reopen it.

## What other members see
- **Rename:** `server.updated` with the new name. The rail and the header update.
- **Leave:** `members.changed`, plus an "X left" line in the member list. Messages the leaver already sent stay; they were signed and shared.
- **Delete:** `server.removed { serverId, reason: "deleted" }` and a one-time notice: "The owner deleted this server". Members offline at the time learn it on their next sync:
  - peers keep a closed server's genesis and close record forever, so a late member can always verify the close;
  - everything else in that server is purged after 30 days.

## Keys and local data
- **Leave, on the remaining members:** the same path as a kick (M2). The first moderator or above to read the `Leave` rotates every channel key the leaver held.
  - This happens at once and isn't batched (GAP-8 batches only joins).
  - With no moderator online, it's `rekey-waiting` as today, and sending answers `not-allowed` until one rotates.
- **Leave and delete, on the leaver's own device, and on everyone's device after a delete:** purge the server. That means:
  - close its sync logs (gossip unsubscribe, drop pending records);
  - delete its records, invites, profiles and channel keys (`GroupCipher::forget`);
  - drop any call in it;
  - then emit `server.removed`.
  - The kick path uses the same `purge_server`; today `remove_server` leaves records and keys behind.
- **Delete:** no rotation is needed, since nothing after the close is accepted.
- **Honest limit:** a delete or leave isn't erasure. Any member may keep copies of what they already received. The UI wording must say "removed from your devices", not "erased".

## Compatibility
Clients up to 0.2.x refuse these kinds. Control records name the latest control heads in their deps, so an old client would also refuse every control record after a rename or leave, and its whole control log would stall. So the code itself enforces a gate:
1. **Release N:**
   - It accepts and renders all three kinds.
   - It also announces its support. The profile post (a control-log `Post` tagged `mvs/profile/v1`, which every version accepts) gains a field `caps: ["server-lifecycle-v1"]`.
   - Old peers parse it fine, because `ProfileBody` ignores unknown fields.
   - Release N re-posts its profile once in every server where its latest profile post lacks the cap.
2. **The gate**, in the api's `control_record`:
   - The api sends `ServerEdit`, `Leave` or `ServerClose` only when every current member's latest profile post carries `server-lifecycle-v1`.
   - Otherwise it answers a new error code, `update-needed`, without signing anything. The UI names the members who still need to update.
   - The cap is signed by each member's own key, so nobody can claim it for someone else. A member who claims it falsely only stalls their own log.
3. The updater's minimum version can later move to N, and then the gate is always open. It stays as a guard regardless.

## Pieces (in order)
1. **authority** (owner: Tech Lead / dev-records): `core/authority/src/lib.rs`.
   - Add `Action::ServerEdit{name}`, `Leave{cut}` and `ServerClose`.
   - Change `from_record`: `ServerEdit` JSON ignores unknown fields, and the name follows the strict rule.
   - Change the resolver: role checks, a leave ends a role, and the close's causal-past rule (first close wins).
   - Change `State`: name override, closed flag, leavers removed from `members`.
   - Tests in `core/authority/tests/`, including concurrent records around a close and two closes.
   - `core/records/src/lib.rs` needs no change.
2. **api, accept side** (dev-identity, release N):
   - `core/api/src/model.rs`:
     - the summary name comes from the latest `ServerEdit`;
     - `ProfileBody` gains `caps`;
     - the profile is re-posted once where the cap is missing.
   - `core/api/src/service.rs`:
     - purge on a read close, a read kick, or my own leave;
     - rekey on a read `Leave`.
   - `core/api/src/db.rs`: `purge_server`, shared with the kick path, which keeps the genesis and close record of a closed server.
   - `core/api/src/crypto/mod.rs` and `core/api/src/ports.rs`: `GroupCipher::forget`, `Network::close_server`.
   - `core/api/src/types.rs`:
     - an additive `reason` field on `ServerRemoved`;
     - the `update-needed` error code.
   - Flows tests in `core/api/tests/flows.rs`, and `core/api/CONFORMANCE.md`.
3. **sync** (dev-sync):
   - `core/sync/src/node.rs`: a `Command::CloseServer` that unsubscribes and drops pending records.
   - `core/sync/src/store.rs`:
     - delete a log's records;
     - after 30 days, purge a closed server except its genesis and close record.
   - `core/api/src/syncnet.rs`: the `close_server` port.
4. **api, send side** (dev-identity, release N+1):
   - New commands, each with errors `not-allowed`, `not-found`, `bad-request` and `update-needed`:
     - `renameServer(serverId, name)` → `ServerSummary`;
     - `leaveServer(serverId)` → `null`;
     - `deleteServer(serverId)` → `null`.
   - They sit behind the cap gate in `control_record`.
   - Files: `service.rs`, `dispatch.rs`, `CONFORMANCE.md`, `tests/flows.rs`.
   - The tests include `update-needed` while one member lacks the cap.
5. **UI** (dev-ui / dev-ui-settings):
   - `ui/src/core/api.ts` and `ui/src/app/store.ts`.
   - `ui/src/screens/shell/ServerSettings.tsx`:
     - a rename field;
     - delete with a typed-name confirm;
     - for `update-needed`, the list of members still to update.
   - `ui/src/screens/shell/ServerRail.tsx` with `ui/src/components/ContextMenu.tsx`: "Leave server".
   - Strings in `ui/src/i18n/en.json` and `he.json`; tests in `ui/src/test/`.
6. **QA:** a two-peer smoke covering:
   - a rename seen by the other peer;
   - a leave that rotates keys, after which the leaver reads nothing new;
   - a delete that removes the server on both peers, including one that was offline at the time;
   - `update-needed` against a 0.2.x peer.

## Decided in review (Tech Lead)
- **No silent leave.** A leave always signs a `Leave`, so the remaining members rotate keys.
- **Retention.** A closed server's genesis and close record are kept forever. The rest is purged after 30 days.
- **The kick path** shares `purge_server`.
