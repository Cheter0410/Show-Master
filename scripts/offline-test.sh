#!/usr/bin/env bash
# Proves that Show Master works with no network at all, and that the update
# check is the only thing that can ever reach the internet:
#   1. static: the HTTP client is used by the updater module and nowhere else,
#   2. static: the built web UI references no external origin,
#   3. dynamic (Linux): the app is started with --no-update-check inside an empty network namespace
#      (loopback only), driven through its API, and strace shows that it never
#      tried to connect to a non-loopback address.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== 1. dependency tree =="
# ureq (with rustls) is allowed: it is used by the updater and nothing else
# (crates/sm-server/src/update.rs). Telemetry and other HTTP stacks stay banned.
banned='^(reqwest|isahc|curl|curl-sys|hyper-tls|hyper-rustls|native-tls|openssl|openssl-sys|surf|attohttpc|sentry|opentelemetry.*)$'
if cargo tree -p showmaster --prefix none --format '{p}' --edges normal,build 2>/dev/null | awk '{print $1}' | sort -u | grep -E "$banned"; then
  echo "FAIL: network client crates found in the dependency tree" >&2; exit 1
fi
# The updater must be the only user of the HTTP client.
if grep -rl "ureq::" crates --include='*.rs' | grep -v "crates/sm-server/src/update.rs"; then
  echo "FAIL: HTTP client used outside the updater" >&2; exit 1
fi
echo "ok: the updater is the only code that can reach the internet"

echo "== 2. web UI assets =="
if [ -d ui/dist ]; then
  # XML namespace identifiers are not network requests.
  if grep -rEoh "(https?:)?//[a-zA-Z0-9.-]+\.[a-z]{2,}[^\"' )]*" ui/dist --include='*.html' --include='*.css' --include='*.js' --include='*.webmanifest' \
      | grep -vE "w3\.org|reactjs\.org|react\.dev|github\.com/facebook|mozilla\.org|example\.(com|org)|localhost|127\.0\.0\.1" | sort -u | head -20 | grep . ; then
    echo "FAIL: external URLs referenced by the UI bundle" >&2; exit 1
  fi
  if grep -rEl "<(script|link|img|iframe)[^>]+(src|href)=[\"'](https?:)?//" ui/dist --include='*.html'; then
    echo "FAIL: HTML loads external resources" >&2; exit 1
  fi
  echo "ok: UI bundle is self-contained"
else
  echo "skip: ui/dist not built"
fi

echo "== 3. run without network =="
if [ "$(uname -s)" != "Linux" ] || ! unshare -r -n true 2>/dev/null; then
  echo "skip: network namespaces unavailable on this system"; exit 0
fi
cargo build -q -p showmaster
BIN="$PWD/target/debug/showmaster"
WORK="$(mktemp -d)"; trap 'pkill -KILL -f "$WORK/offline.smshow" 2>/dev/null; rm -rf "$WORK"' EXIT
cat > "$WORK/inner.sh" <<'INNER'
set -euo pipefail
# Bring loopback up (iproute2 if present, otherwise a tiny ioctl).
ip link set lo up 2>/dev/null || python3 - <<'PY'
import fcntl, socket, struct
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
flags = struct.unpack("16sH", fcntl.ioctl(s, 0x8913, struct.pack("16sH", b"lo", 0)))[1]
fcntl.ioctl(s, 0x8914, struct.pack("16sH", b"lo", flags | 1))
PY
TRACE=""
if command -v strace >/dev/null 2>&1; then TRACE="strace -f -e trace=connect,sendto -o $WORK/trace.log"; fi
$TRACE "$BIN" --no-update-check --port 7499 --log-dir "$WORK/logs" "$WORK/offline.smshow" > "$WORK/app.log" 2>&1 &
PID=$!
for i in $(seq 1 50); do curl -fs --noproxy '*' http://127.0.0.1:7499/api/session >/dev/null 2>&1 && break; sleep 0.2; done
api() { curl -fsS --noproxy '*' "$@"; echo; }
api http://127.0.0.1:7499/api/session | grep -q '"role":"admin"'
api -X POST -H 'content-type: application/json' -d '{"cmd":"add_cue","kind":"memo"}' http://127.0.0.1:7499/api/v1/command | grep -q '"ok":true'
api -X POST -H 'content-type: application/json' -d '{"cmd":"update_show","patch":{"rtp_midi":{"enabled":true,"port":0},"timecode":{"ltc_enabled":true,"mtc_enabled":true,"mtc_port":"rtp:Show Master"}}}' http://127.0.0.1:7499/api/v1/command | grep -q '"ok":true'
api http://127.0.0.1:7499/api/v1/cmd/tc_start | grep -q '"ok":true'
api http://127.0.0.1:7499/api/v1/cmd/go | grep -q '"ok":true'
api -X POST -H 'content-type: application/json' -d "{\"cmd\":\"save_show\",\"path\":\"$WORK/offline.smshow\"}" http://127.0.0.1:7499/api/v1/command | grep -q '"ok":true'
sleep 1
api http://127.0.0.1:7499/api/v1/state | grep -q '"running":true'
curl -fs --noproxy '*' http://127.0.0.1:7499/ | grep -qi '<!doctype html'
# Stop the app (it runs as a child of strace when tracing).
pkill -INT -f "$WORK/offline.smshow" || true
for i in $(seq 1 30); do pgrep -f "$WORK/offline.smshow" >/dev/null || break; sleep 0.1; done
pkill -KILL -f "$WORK/offline.smshow" 2>/dev/null || true
wait $PID 2>/dev/null || true
INNER
export BIN WORK
unshare -r -n bash "$WORK/inner.sh"
test -s "$WORK/offline.smshow"
if [ -s "$WORK/trace.log" ]; then
  # Any connect()/sendto() to an address that is not loopback / unix / netlink is a failure.
  if grep -E "sin_addr=inet_addr\(\"|inet_pton\(AF_INET6" "$WORK/trace.log" | grep -vE '127\.0\.0\.1|"::1"|0\.0\.0\.0' | head -5 | grep . ; then
    echo "FAIL: the app tried to reach a non-loopback address" >&2; exit 1
  fi
  echo "ok: no outbound connection attempts (strace)"
else
  echo "note: strace not installed - skipped syscall check"
fi
echo "ok: app is fully functional without any network"
