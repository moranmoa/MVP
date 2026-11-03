#!/usr/bin/env bash
# Every scenario in RESULTS.md, in order (~20 minutes).
set -euo pipefail
cd "$(dirname "$0")"
LOSSY="delay 50ms loss 2%"
JITTER="delay 50ms 10ms distribution normal loss 2%"
./run.sh c6-clean 6 0 ""
./run.sh c6-lossy 6 0 "$LOSSY"
./run.sh c6-jitter 6 0 "$JITTER"
./run.sh s6-clean 0 6 ""
./run.sh s6-lossy 0 6 "$LOSSY"
./run.sh s6-jitter 0 6 "$JITTER"
./run.sh mix-lossy 3 3 "$LOSSY"
# Real capture path with the browser's echo cancellation / noise suppression /
# gain control on and off (fake microphone), for the CPU cost of processing.
SRC=mic PROCESSING=1 ./run.sh c6-mic-apm 6 0 ""
SRC=mic PROCESSING=0 ./run.sh c6-mic-raw 6 0 ""
# The same processing compiled into the native path.
STR0M=str0m-apm ./run.sh s6-apm 0 6 ""
node summarize.mjs c6-clean c6-lossy c6-jitter s6-clean s6-lossy s6-jitter mix-lossy c6-mic-apm c6-mic-raw s6-apm | tee summary.md
