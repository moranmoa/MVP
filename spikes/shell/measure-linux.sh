#!/usr/bin/env bash
# Footprint on Linux (experiment 6), inside the container from
# Dockerfile.measure: both .deb packages installed, Xvfb as the display.
#
#   measure-linux.sh <command> <label> [runs]
#
# Prints one JSON line: start times of every run (ms from exec to the first
# message rows on screen), PSS of the whole process tree at idle and after
# scrolling 10k messages (first run), and the installed size.
set -euo pipefail
CMD="$1" LABEL="$2" RUNS="${3:-5}"
export DISPLAY=:99
pgrep Xvfb >/dev/null || { Xvfb :99 -screen 0 1366x900x24 >/dev/null 2>&1 & sleep 1; }

tree() { # pid and all descendants
  local p=$1; echo "$p"
  for c in $(cat /proc/"$p"/task/*/children 2>/dev/null); do tree "$c"; done
}
pss_mib() {
  local sum=0
  for p in $(tree "$1"); do
    v=$(awk '/^Pss:/ {print $2}' /proc/"$p"/smaps_rollup 2>/dev/null || echo 0)
    sum=$((sum + ${v:-0}))
  done
  echo $((sum / 1024))
}
wait_mark() { # file label timeout-s
  for _ in $(seq 1 $(($3 * 20))); do grep -q "^$2 " "$1" 2>/dev/null && return 0; sleep 0.05; done
  echo "timeout waiting for $2" >&2; return 1
}

starts=() idle=0 scrolled=0
for run in $(seq 1 "$RUNS"); do
  out=$(mktemp)
  t0=$(date +%s%3N)
  SHELL_BENCH_OUT="$out" $CMD >/dev/null 2>&1 &
  pid=$!
  wait_mark "$out" rendered 60
  starts+=($(( $(awk '/^rendered/ {print $2}' "$out") - t0 )))
  if [ "$run" = 1 ]; then
    sleep 10; idle=$(pss_mib $pid)
    wait_mark "$out" scrolled 180
    sleep 3; scrolled=$(pss_mib $pid)
  fi
  for p in $(tree $pid | tac); do kill "$p" 2>/dev/null || true; done
  wait $pid 2>/dev/null || true
  sleep 2
done
printf '{"shell":"%s","startMs":[%s],"idlePssMiB":%s,"scrolled10kPssMiB":%s}\n' \
  "$LABEL" "$(IFS=,; echo "${starts[*]}")" "$idle" "$scrolled"
