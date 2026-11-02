# Core (Rust)

| Crate | What it is |
| --- | --- |
| `authority` | Deterministic resolution of a server's control log: roles, revoke, kick, and voiding of concurrent "backdated" actions |
| `spike-peer` | Spike experiment 1: connects two peers through the relay, then tries a direct hole-punched connection. Tester instructions (Hebrew): `spike-peer/TESTING.md` |
| `sync` | Spike experiment 2: SQLite store of content-addressed blobs + libp2p sync (pull by have-markers, gossipsub push). Harness: `bash sync/harness/run.sh`; results in `sync/RESULTS.md` |

## Running the tests

No local Rust needed; the tests run in a container:

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm cargo test --release -- --nocapture
```

Run it from this folder. `tests/convergence.rs` is spike experiment 3: 10,000
random scenarios, each delivered to a peer in 3 random orders, must all reach
the same state.

## Building spike-peer.exe for Windows testers

Cross-compiled in a container; the exe lands in `dist/` (git-ignored):

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm bash -c "apt-get update -qq && apt-get install -y -qq gcc-mingw-w64-x86-64 && rustup target add x86_64-pc-windows-gnu && cargo build --release -p spike-peer --target x86_64-pc-windows-gnu && mkdir -p dist && cp /target/x86_64-pc-windows-gnu/release/spike-peer.exe dist/"
```
