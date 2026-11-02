# Core (Rust)

| Crate | What it is |
| --- | --- |
| `authority` | Deterministic resolution of a server's control log: roles, revoke, kick, and voiding of concurrent "backdated" actions |

## Running the tests

No local Rust needed; the tests run in a container:

```
docker run --rm -v "${PWD}:/work" -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm cargo test --release -- --nocapture
```

Run it from this folder. `tests/convergence.rs` is spike experiment 3: 10,000
random scenarios, each delivered to a peer in 3 random orders, must all reach
the same state.
