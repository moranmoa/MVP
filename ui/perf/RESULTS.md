# B14: UI performance: results

**On a normal desktop CPU the UI meets the targets with room to spare.**

- The pending echo appears on screen in 17–22 ms p50 (p95 ≤ 33 ms), against
  the B6 target of 50 ms.
- Channel switches take 34–64 ms p50.
- Scrolling through 5,000 messages runs at 60 fps, with 0.6–5% of frames
  missed and no blank placeholders.

**With the CPU throttled 4× (a slow laptop), the UI misses its targets:**

- The pending echo is 44–64 ms p50, and its p95 reaches 120 ms.
- Channel switches take 156–329 ms p50.
- Scrolling missed about half its frames in these runs, but most of that
  was a busy shared Docker host. Measured again on a quiet host, it misses
  5–24% with 0–70 long tasks; see "Scroll at 4× (P2)".
- The echo and switch numbers at 4× come from the same runs and may be
  inflated the same way. On a quiet host (current code, 4 full runs at 4×)
  the echo was 33–43 ms p50, so within target, and switches took 107–191 ms
  p50.

Measured on 2026-11-10 with headless Chromium 141 (the engine inside the
Electron shell), in the Playwright Docker image on a 12-CPU desktop, against
the mock core (`?demo=1`). Three runs; each cell gives the range of the
three runs' values.

## Reproduce

From `ui/`:

```bash
docker compose run --rm shots npm run perf
```

The script builds the UI, serves `dist/` with `vite preview`, drives it in
Chromium, and prints the table below. Raw numbers go to
`perf/out/results.json` (not committed). Options (environment):
- `PERF_SENDS` (30) and `PERF_SWITCHES` (20): samples per run.
- `PERF_SCROLL_STEP` (400): px scrolled per frame.
- `PERF_THROTTLE` (`1,4`): CPU slow-down factors.

It is not part of CI.

## Numbers

ms, p50 / p95 unless noted; ranges are across the 3 runs.

| Measure | CPU 1× | CPU 4× |
|---|---|---|
| **Pending echo** (Enter → message on screen as "pending"; 30 sends) | | |
| to the DOM | 4.1–6.0 / 6.6–13.6 | 20.3–28.8 / 35.2–76.2 |
| to the screen (target < 50) | **17.2–22.1 / 22.9–32.8** | **43.7–63.8 / 54.3–119.9** |
| **Channel switch** (click → the channel's messages on screen; 10 of each) | | |
| to a small channel (300–500 messages) | 33.5–56.8 / 49–113 | 156–253 / 285–547 |
| to a large channel (5,000–100,000 messages) | 46.5–64.4 / 77–189 | 195–329 / 340–532 |
| **Scrolling up through 5,000 messages** (400 px per frame, ~1,560 frames) | | |
| frame time p50 / p95 | 16.7 / 16.8–33.2 | 16.7–33.3 / 83.3–166.6 |
| frame time p99 | 16.8–33.4 | 116.6–283.3 |
| frames over 20 ms (≥ 1 missed) / over 34 ms (≥ 2 missed) | 0.6–5.1% / 0.1–0.8% | 46.9–54.6% / 23.5–35.2% |
| frames showing placeholder rows | 0% | 0% |
| long tasks (count, total ms) | 0–5, 0–369 | 280–477, 19–50 s |
| reached the first message | yes | yes |

Per-run tables come from the script's output; the runs differ by the usual
noise of a shared Docker host (other builds were running).

## How it measures

- **Pending echo.** The composer is filled first, then the clock starts at
  the Enter `keydown`. "To the DOM" is when a row with that text and the
  "pending" marker exists; a MutationObserver plus a per-frame check catches
  it. "To the screen" adds the frame that paints it (rAF, then a task). The
  sends go into `general` of the Minecraft IL demo server (5,000 messages).
- **Channel switch.** The clock starts at the click. The switch is done when
  the message feed is labelled with the new channel, is no longer
  `aria-busy`, shows at least one real message row, and that frame has
  painted. Switches alternate between a small and a large channel of the
  same server: `builds` ↔ `general` (Minecraft IL) and `clips` ↔ `כללי`
  (100,000 messages).
- **Scrolling.** A rAF loop moves the feed up 400 px per frame from the
  newest message to the first, a fast fling. It records the time between
  frames, whether any `.msg.skeleton` placeholder is in view, and long tasks
  (PerformanceObserver `longtask`).
- **4× CPU** is Chromium's `Emulation.setCPUThrottlingRate`. It slows
  JavaScript, layout and paint about 4×, which is roughly a low-end laptop.
  It is an approximation.

## What this does not cover

- **The real core.** The mock answers in-process, so the echo is the UI's
  own share. In the Electron shell, `sendMessage` adds an IPC round trip to
  the core before the pending row appears (B6 asks for the whole path under
  50 ms). A run against the shell with the real core (after #89) needs
  Playwright's `_electron.launch`. That would be the next step for this
  harness; nothing in the measure code is specific to the mock.
- **Cold start and memory after an hour** were in the first B14 brief, not in
  this one. They are not measured yet; both fit the same script.
- **Real GPU and display.** Headless Chromium paints in software, and rAF
  pacing is the browser's, not a monitor's. A display at more than 60 Hz
  needs its own check.

## Scroll at 4× (P2): investigation, before and after

**Result: no code change. The "about half the frames drop" figure was mostly
CPU taken by other containers on the shared Docker host.** On a quieter host
the current code met the P2 acceptance (under 15% of frames over 20 ms,
under 50 long tasks) in 4 of 6 runs (5.2–10.7%, 0–12 long tasks). The other
two were 15.5% (11 long tasks) and 24.1% (70). None came near half. Of the
changes tried, the obvious one, a per-row height estimate, made scrolling
worse, and none of the others beat the noise, so none is proposed. A clean
verdict needs a quiet machine, not this shared one.

### Host load

The Docker VM is shared by every agent's builds. 4× throttling amplifies any
CPU taken from the browser. The harness now records the VM's 1-minute load
average and warns when it is above half the CPU count.

Current code at 4×, every run that recorded the load (before → after the
run). A 1-minute average lags, so it only roughly tracks what the browser
competed with:

| Run | Host load | Frames over 20 ms | Long tasks (count, total) |
|---|---|---|---|
| scroll only | 3.0 → 2.8 | 5.2% | 0 |
| scroll only | 5.0 → 4.0 | 6.4% | 0 |
| full harness | 8.1 → 5.4 | 10.7% | 12, 0.7 s |
| full harness | 2.8 → 4.2 | 24.1% | 70, 5.5 s |
| scroll only (busy warning) | 8.6 → 7.3 | 27.6% | 66, 3.9 s |
| full harness (other agents building) | 4.5 → 16.7 | 41.9% | 275, 30 s |
| B14 runs above (load not recorded) | – | 46.9–54.6% | 280–477, 19–50 s |

Two earlier full runs, made before the load was recorded but while the host
was idle, gave 7.2% and 15.5% (8 and 11 long tasks).

### What was tried

1. **A per-row estimate.** The list estimated every row at 56 px; real rows
   are 60 px (header), 55 px (grouped) or 81 px (reply), measured. The idea
   was that matching estimates mean fewer corrections, each of which costs a
   layout while scrolling. Each row's estimate was computed from its message.
2. **`contain: layout style` on `.row`**, so a mounted row is laid out on
   its own. There is no paint containment, because the hover toolbar sticks
   out above the row.
3. **A constant 60 px estimate**, alone and with 2.

Interleaved runs (each variant is a prebuilt `dist`, run in turn so all see
the same host load), scroll only, 4×, host load 2–5:

| Build | Frames over 20 ms (2 runs) | Long tasks (2 runs) |
|---|---|---|
| current code | 6.4%, 5.2% | 0, 0 |
| 1. per-row estimate | **49.4%, 44.2%** | 28, 0 |
| 2. containment | 11.8%, 3.9% | 42, 0 |
| 3. constant 60 px | 7.8%, 21.6% | 0, 20 |
| 3 + 2 | 11.8%, 13.6% | 0, 33 |

The full harness at 4× (3 interleaved pairs, current code vs 1 + 2) shows
the same thing. With 1 + 2 the median frame is 33.3 ms in every run (53–59%
of frames over 20 ms), against 16.7 ms for the current code.

- **Why the per-row estimate hurts.** TanStack calls `estimateSize` for
  every row not yet measured, which is thousands of rows in a 5,000-message
  channel, each time it recomputes positions. A function that looks up two
  messages costs more on every frame than the corrections it saves. Any
  future estimate must stay a constant, or be cached per index.
- **Variants 2 and 3** are within the run-to-run noise of the current code.
  There is no measured gain, so they are not worth a change.

Where the time goes (`node perf/profile.mjs`, the CPU profiler during the
4× scroll, quiet host): about 40% browser-native style, layout and paint;
15% forced layout in the scroll driver (layout the browser would do anyway);
6% TanStack position recomputes; React rendering, `formatDate` and the
direction checks about 1% each. Rows are already memoised (`MessageRow` is
`memo`, with stable props), and TanStack 3.19 already skips measuring rows
on mount while scrolling. The two fixes the task suggested are therefore in
place.

### Reproduce the comparison

```bash
docker compose run --rm -e PERF_ONLY=scroll -e PERF_THROTTLE=4 shots npm run perf
```

To compare builds, build each into a folder (`npx vite build --outDir
perf/out/dist-x`) and run with `-e PERF_DIST=perf/out/dist-x`, alternating
between builds. Only compare runs without the busy-host warning.

## Findings for dev-ui

These are not filed as issues yet; the numbers above are the evidence.

1. **Pending echo misses 50 ms on a slow CPU** (4×: p50 44–64, p95 54–120
   ms). On a normal CPU the DOM update itself is 4–6 ms and the rest is
   waiting for the next frame, so the gap at 4× is render work.
2. **Scrolling a long channel on a slow CPU: withdrawn.** The first runs
   (about half the frames dropped) were inflated by a busy shared host. On
   a quiet host the current code meets the P2 target; see "Scroll at 4×
   (P2)" above.
3. **Channel switch takes 150–330 ms p50 on a slow CPU** (p95 up to
   550 ms), against 34–64 ms at full speed.
