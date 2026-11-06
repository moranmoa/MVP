# qa – MVS test plan, automated checks, bug list

| File | What it is |
| --- | --- |
| `TEST_PLAN.md` | Closed-alpha test plan (Hebrew): cases per feature, non-functional targets, what is automatable |
| `BUGS.md` | Bugs found by exploratory testing, with severity, steps, expected vs actual and owner |
| `tests/smoke.spec.ts` | Smoke suite: onboarding → word check → create server → send → join by link → delivered |
| `tests/channels.spec.ts` | Smoke suite, channels (B11c): owner create/rename/delete, no controls for members, messages stay in their channel |
| `tests/explore.spec.ts` | Exploratory checks and one repro per UI bug in `BUGS.md` |
| `core/cli-checks.sh` | `updater` with tampered manifests and `spike-peer` with bad input |

Nothing needs Node, Rust or a browser on the host; everything runs in Docker.

## Run the smoke suite

From the repository root:

```bash
docker compose -f qa/compose.yaml run --rm smoke
```

The first run builds the UI (`ui` service, `node:22.23.4-bookworm`) and
serves it with `vite preview`; the tests run in
`mcr.microsoft.com/playwright:v1.56.1-noble` against
`http://localhost:5173`. Expected result: `12 passed, 1 skipped` (voice
channels, not in the UI yet).

## Run the exploratory checks

```bash
docker compose -f qa/compose.yaml run --rm explore
```

Expected result: `46 passed`. Tests named `QA-xx` or `UX-xx` are known bugs marked
`test.fail()`: the list reporter shows them with ✘ but counts them as
passed. When one reports "expected to fail, but passed", the bug is fixed –
remove its `bug('QA-xx')` line and close the row in `BUGS.md`.
To see which bugs still reproduce as plain failures:

```bash
docker compose -f qa/compose.yaml run --rm -e QA_RAW=1 explore
```

Screenshots for manual review are written to `qa/test-results/explore-shots/`;
the HTML report to `qa/playwright-report/` (both git-ignored).

Stop the UI server afterwards:

```bash
docker compose -f qa/compose.yaml down
```

## Run the core CLI checks

From the repository root, in PowerShell:

```
docker run --rm -v "${PWD}:/repo" -v cargo-registry:/usr/local/cargo/registry -v qa-core-target:/target -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 rust:1.90-bookworm bash /repo/qa/core/cli-checks.sh
```

In Git Bash, prefix it with `MSYS_NO_PATHCONV=1` and use `"$(pwd -W):/repo"`.
Today it prints `34 passed, 3 failed`: the three failures are QA-14, QA-15
and QA-16. Keys are generated in `/tmp` inside the container only.

The crates' own tests are run as described in `core/README.md`.
