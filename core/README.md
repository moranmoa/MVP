# Core (Rust)

| Crate | What it is |
| --- | --- |
| `records` | Signed record envelope: canonical binary encoding, BLAKE3 content IDs, Ed25519 signatures, per-kind size limits |
| `authority` | Deterministic resolution of a server's control log: roles, revoke, kick, and voiding of concurrent "backdated" actions. `resolve` recomputes from scratch (the reference); `Resolver` applies arriving records incrementally |
| `spike-peer` | Spike experiment 1: connects two peers through the relay, then tries a direct hole-punched connection. Tester instructions (Hebrew): `spike-peer/TESTING.md` |
| `updater` | Spike experiment 7: signed update chain. Release manifest with 1-of-1 or 2-of-3 Ed25519 root signatures, expiry, no-downgrade and revocation rules, artifact hash check, and a rollback state machine. CLI: `keygen`, `make-root`, `hash`, `sign-manifest`, `verify` |

## Running the tests

No local Rust needed; the tests run in a container:

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm cargo test --release -- --nocapture
```

Run it from this folder. `tests/convergence.rs` is spike experiment 3: 10,000
random scenarios, each delivered to a peer in 3 random orders, must all reach
the same state, and after every arrival the incremental `Resolver` must equal
`resolve` over the records held so far. It also prints timings for a ~5,000
record log, incremental vs full.

## Building spike-peer.exe for Windows testers

Cross-compiled in a container; the exe lands in `dist/` (git-ignored):

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm bash -c "apt-get update -qq && apt-get install -y -qq gcc-mingw-w64-x86-64 && rustup target add x86_64-pc-windows-gnu && cargo build --release -p spike-peer --target x86_64-pc-windows-gnu && mkdir -p dist && cp /target/x86_64-pc-windows-gnu/release/spike-peer.exe dist/"
```

## Signing a release (updater CLI)

Keys never go inside a git checkout; `keygen` refuses to write there. Keep
the secret `.key` files offline. Example with three keys, any two must sign:

```
updater keygen --out D:\offline-keys --name a        # repeat for b, c
updater make-root --threshold 2 --pub a.pub --pub b.pub --pub c.pub --out root.json
updater hash --file app-1.1.0.exe                     # paste into body.json
updater sign-manifest --key a.key --body body.json --out manifest.json
updater sign-manifest --key c.key --manifest manifest.json --out manifest.json
updater verify --manifest manifest.json --root root.json --installed 1.0.0 --artifact app-1.1.0.exe --platform windows-x86_64
```

For the closed alpha use one key and `--threshold 1`.

## Licence check

CI runs `cargo-deny` with `deny.toml` (policy from `docs/spec.md`). Locally:

```
docker run --rm -v "${PWD}/..:/work" -v cargo-registry:/usr/local/cargo/registry -w /work rust:1.90-bookworm bash -c "curl -sSL https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz | tar xz -C /tmp && /tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny --manifest-path core/Cargo.toml --config core/deny.toml check licenses sources bans"
```
