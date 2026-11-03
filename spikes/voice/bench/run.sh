#!/usr/bin/env bash
# One bench scenario: a room with C Chromium and S str0m participants, each
# in its own container with the same netem rule on its egress.
#
#   ./run.sh <room> <chromium count> <str0m count> ["netem rule"] [duration s]
#   ./run.sh clean-c6 6 0
#   ./run.sh lossy-s6 0 6 "delay 50ms loss 2%"
#
# Results: results/<room>/<peer>.json; summarise with `node summarize.mjs`.
set -euo pipefail
cd "$(dirname "$0")"
export MSYS_NO_PATHCONV=1
export ROOM="$1" NETEM="${4:-}" DURATION="${5:-60}" WARMUP="${WARMUP:-10}" SRC="${SRC:-pulses}" PROCESSING="${PROCESSING:-1}"
C="$2" S="$3"

rm -rf "results/$ROOM"
STR0M="${STR0M:-str0m}" # or str0m-apm
docker compose build -q signal chromium "$STR0M"
docker compose up -d signal >/dev/null
# Everyone stops at the same moment: joins take ~1 s each.
export END_AT=$(( $(date +%s) + 15 + 2 * (C + S) + DURATION ))
# Staggered joins, like people arriving; every newcomer offers to the room.
for _ in $(seq 1 "$C"); do docker compose run -d --no-deps chromium >/dev/null; sleep 1; done
for _ in $(seq 1 "$S"); do docker compose run -d --no-deps "$STR0M" >/dev/null; sleep 1; done
ids=$(docker ps -aq --filter "label=com.docker.compose.project=voicebench" --filter "label=com.docker.compose.oneoff=True")
docker wait $ids >/dev/null
docker logs --tail 3 $(echo "$ids" | head -1) 2>&1 | tail -3 || true
docker rm -f $ids >/dev/null
docker compose down -t 1 >/dev/null
ls "results/$ROOM"
