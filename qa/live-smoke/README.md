# Live release smoke

This runs a release's Linux `mvs-core` twice, as the owner and as a member. The two copies sit on separate Docker networks, so they can meet only through the **live relay** named in that release's own `resources/mvs-config.json`.

It runs 10 rounds. In each round, A creates a server, B joins it by invite, and a message goes each way. It reports the time to join and the time to first message.

```bash
MVS_LIVE_SMOKE_OK="<the manager's OK for this run>" TAG=v0.3.0 bash qa/live-smoke/run.sh
```

## Conditions (tech lead, #281)

- **QA only, after a release, with the manager's OK for each run.**
  - The script refuses to start without `MVS_LIVE_SMOKE_OK`. Its value goes into the output, so the run's report says who approved it.
  - It never runs in CI: the script exits when `CI` or `GITHUB_ACTIONS` is set. No workflow calls it.
- **No relay address or IP in the output.** Everything the script prints, and the logs it saves, go through `mask()`. That function hides:
  - the relay's host;
  - multiaddr hosts;
  - peer IDs;
  - IPv4 and IPv6 addresses.

  The real address stays only in the git-ignored `.build/` (the release's config). Never paste raw container logs anywhere.

## What it uses

- `qa/upgrade/run.sh` downloads the release's `linux-x86_64.tar.gz` and checks it against `SHA256SUMS`.
- Each side runs in the `mvs-voice-e2e-app` image (Node, a session D-Bus and a test keyring) with a temp data dir.
- `live.mjs` speaks the core's stdin/stdout line protocol and drops the core's stderr.

## The first send after a join (#285)

Up to v0.3.0, the joiner's first `sendMessage` right after a join got `not-allowed` once, and worked about 0.5 s later. #285 fixed this in v0.3.1. `live.mjs` now reports each round's first send as `OK`, or as `FAIL` with the number of refusals. It still retries for up to 60 s, so the remaining rounds can run.

## Results

- **v0.3.0, 2026-11-10:** 10/10 joins in 0.5–2.7 s. First message B→A 104–291 ms, A→B 106–303 ms.
- **v0.3.1, 2026-11-10:** 10/10 joins in 0.3–0.5 s. The first send right after a join succeeded in 10/10 rounds, in 110–121 ms (#285). First message B→A 60–260 ms, A→B 47–246 ms.
