#!/usr/bin/env bash
# Measures start-up time (process start -> API answering) and resident memory
# of the release build. Prints the numbers; fails only on gross regressions.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=target/release/showmaster
[ -x "$BIN" ] || cargo build --release -q -p showmaster
PORT=7498
WORK="$(mktemp -d)"; trap 'kill $PID 2>/dev/null || true; rm -rf "$WORK"' EXIT
now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
T0=$(now_ms)
"$BIN" --no-update-check --port $PORT --local-only --log-dir "$WORK" > "$WORK/out.log" 2>&1 &
PID=$!
for i in $(seq 1 400); do
  curl -fs --noproxy '*' "http://127.0.0.1:$PORT/api/session" >/dev/null 2>&1 && break
  sleep 0.025
done
T1=$(now_ms)
sleep 2
RSS=$(ps -o rss= -p $PID | tr -d ' ')
CPU=$(ps -o %cpu= -p $PID | tr -d ' ')
echo "startup: $((T1 - T0)) ms until the API answers"
echo "idle:    ${RSS} kB resident, ${CPU} % CPU"
echo "binary:  $(du -h "$BIN" | cut -f1)"
[ $((T1 - T0)) -lt 5000 ] || { echo "FAIL: start-up took more than 5 s" >&2; exit 1; }
[ "$RSS" -lt 300000 ] || { echo "FAIL: idle memory above 300 MB" >&2; exit 1; }
