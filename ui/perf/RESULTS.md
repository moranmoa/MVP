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
- Scrolling misses about half its frames. The main thread runs 280–477 long
  tasks during the scroll; the virtual list never shows placeholders, so the
  time goes into rendering rows, not into waiting for data.

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

## Findings for dev-ui

These are not filed as issues yet; the numbers above are the evidence.

1. **Pending echo misses 50 ms on a slow CPU** (4×: p50 44–64, p95 54–120
   ms). On a normal CPU the DOM update itself is 4–6 ms and the rest is
   waiting for the next frame, so the gap at 4× is render work.
2. **Scrolling a long channel drops about half its frames on a slow CPU**,
   with hundreds of long tasks and no data waits (0% placeholders). Row
   rendering and measuring (`measureElement` on every row) is the likely
   place to look.
3. **Channel switch takes 150–330 ms p50 on a slow CPU** (p95 up to
   550 ms), against 34–64 ms at full speed.
