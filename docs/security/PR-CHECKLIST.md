# Security checklist for PRs touching `core/` or `infra/`

Copy the sections that apply into the PR description and answer each line
with ✔, ✘ (with the reason) or n/a. A ✘ is allowed only if the PR adds or
updates a GAP in [`threat-model.md`](threat-model.md). The security reviewer
checks the answers against the code, not just the ticks.

Background: [`threat-model.md`](threat-model.md). The rule behind most of this
list: **a valid signature is not permission, and nothing from the network is
trusted until it has been decoded, verified and authorised.**

## 1. Every PR

- [ ] No secrets, IPs, hostnames, `.env` files, keys or real recovery words in
      code, tests, fixtures, logs or screenshots. Test keys are generated or
      fixed-seed and named as such.
- [ ] `cargo test --release --locked` and `cargo deny check` pass in Docker
      (`rust:1.90-bookworm`); `Cargo.lock` is committed for every binary
      (including `infra/relay`).
- [ ] New dependencies: licence allowed by `core/deny.toml`, maintained, no
      RustSec advisory, no C/OpenSSL/libdbus pulled in without a note. Say why
      the crate is needed.
- [ ] No `unsafe`. No `unwrap`/`expect`/indexing/arithmetic that can panic on
      peer-controlled data (use `checked_*`, `get`, `split_at_checked`).
- [ ] Nothing logs secrets, message plaintext, recovery words, or (above
      `debug`) peer IDs paired with IPs. `Debug`/`Display` of secret types is
      redacted.
- [ ] Tests include at least one hostile input for every new parser or
      handler (truncated, oversized, wrong version, wrong scope, bad
      signature).

## 2. Wire formats and records (`core/records`, any new encoding)

- [ ] One canonical encoding: decode rejects anything `encode` wouldn't
      produce (order, duplicates, trailing bytes).
- [ ] Every length is bounded **before** allocation; limits are named
      constants and covered by a test at the limit and one past it.
- [ ] New signed or hashed data uses its own domain prefix starting with
      `mvs/` and a version (`mvs/<thing>/v1\0`).
- [ ] New record kinds: scope restricted (`allowed_in`), body limit set, and
      `MAX_RECORD_BYTES` impact stated (it bounds every pending buffer).
- [ ] Format changes are versioned; frozen constants (identity labels, keychain
      names, domain strings) are not changed.
- [ ] Timestamps are informational only: no ordering or permission decision
      uses `ts`.

## 3. Authorisation (`core/authority`, `core/groupkeys`, glue)

- [ ] Every new record kind has an explicit "who may write this" rule in
      authority, with tests for each role, for a kicked author and for a
      **non-member key** (GAP-3).
- [ ] Channel-scope records are checked against control state via
      `auth_deps`, not accepted because they're signed (GAP-2).
- [ ] Revokes and kicks void the target's concurrent records of the new kind
      (backdating test: target signs on top of pre-removal heads).
- [ ] Resolution stays order-independent: a test applies the same records in
      several orders and gets the same result.
- [ ] Anything decided from `ServerId` checks it against the genesis.
- [ ] Key rotation: rotator authorised, recipients ⊆ current members, removed
      members excluded, `set_voided` fed from authority (GAP-5).

## 4. Network and sync (`core/sync`, `infra/relay`)

- [ ] Incoming bytes go through `Record::decode` (signature) and the
      authorisation check **before** they're stored, forwarded (`Accept`) or
      served to others (GAP-1).
- [ ] The log a record is filed under is derived from the record, not from
      the topic or the request.
- [ ] Every handler is bounded: message size, items per request, request rate
      per peer and in total, buffered bytes, connections. Buffers expire.
- [ ] Pulls and gossip for a server are answered only for members (GAP-4); no
      new protocol answers strangers with server data.
- [ ] No new address, peer ID, topic or identifier is published to the DHT,
      relay or gossip without saying who can see it and why. Non-global
      addresses are never advertised (GAP-6).
- [ ] Custom protocol names (`/mvs/...`), never library defaults like
      `/ipfs/kad/1.0.0`.
- [ ] Nothing dials addresses supplied by a peer without a cap.

## 5. Keys and local storage (`core/identity`, any file or keychain access)

- [ ] Secrets are `Zeroizing`/zeroize-on-drop; no secret is cloned into a
      `String` or `Vec` that outlives its use.
- [ ] Key files are created owner-only (Unix `0600`; Windows: single ACL entry
      for the user, no inheritance, as in `core/updater` root keys) and never
      overwritten silently.
- [ ] No silent fallback from the OS keychain to plaintext storage.
- [ ] Data at rest containing messages or keys is encrypted with the identity's
      storage key, or the PR says why not (GAP-11).
- [ ] KDFs and AEADs come from vetted crates; no hand-rolled primitives; nonces
      are random 192-bit (XChaCha) or provably unique.

## 6. Updater and release

- [ ] `core/updater/SECURITY.md` rules 1–8 still hold; any new state file is
      validated on load and can't go below the baseline.
- [ ] A release embeds the latest trust root.
- [ ] Signing keys are never on a networked or cloud-synced disk.

## 7. Helper servers (`infra/`)

- [ ] Ports: only what the README table lists is published; anything
      unencrypted binds to `127.0.0.1`.
- [ ] Container runs as non-root with `read_only`, `cap_drop: [ALL]`,
      `no-new-privileges`, memory and pid limits, log rotation.
- [ ] Images pinned by digest; builds use `--locked`.
- [ ] Logs don't keep who-talks-to-whom beyond counts at the default level.
- [ ] Secrets come from `.env` with fail-closed defaults (`${VAR:?}`), never
      a placeholder that works.
- [ ] TURN (when touched): private, loopback and link-local peer ranges denied;
      quotas set.

## 8. UI boundary (when the PR changes what the core sends the UI)

- [ ] New text, images or colours from other users pass through `guardCore`
      (`ui/src/core/sanitize.ts`).
- [ ] No command returns recovery words or keys, except the one-time words
      screen.
- [ ] CSP unchanged or tightened; no `unsafe-eval`, no remote script sources.
