# Shared look for install / build / release scripts (source it, bash 3.2+).
#
#   ui_header "Show Master" "Release 0.4.1"   title box (skipped in nested scripts)
#   ui_section "Bauen"                        a group of steps
#   ui_run "Tests" cargo test ...             spinner while it runs, then ✓ / ✗ with the
#                                             time; output goes to the log, on failure
#                                             the last lines are shown and the script ends
#   ui_skip "Tests" "schon geprüft"           a step that was not needed
#   ui_ok / ui_info / ui_warn / ui_fail       single lines (ui_fail ends the script)
#   ui_sudo                                   ask for the sudo password up front (a
#                                             password prompt inside ui_run is invisible)
#
# The log is shared with nested scripts through $UI_LOG.

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  UI_B=$'\033[1m' UI_D=$'\033[2m' UI_G=$'\033[32m' UI_R=$'\033[31m' UI_Y=$'\033[33m' UI_C=$'\033[36m' UI_0=$'\033[0m'
  UI_TTY=1
else
  UI_B='' UI_D='' UI_G='' UI_R='' UI_Y='' UI_C='' UI_0='' UI_TTY=
fi
if [ -z "${UI_LOG:-}" ]; then
  UI_LOG="${TMPDIR:-/tmp}/$(basename "$(pwd)")-$(date +%Y%m%d-%H%M%S).log"
  export UI_LOG
  : >"$UI_LOG"
  UI_TOP=1
else
  UI_TOP=
fi

ui_time() {
  local s=$1
  if [ "$s" -ge 60 ]; then printf '%d:%02d min' $((s / 60)) $((s % 60)); else printf '%d s' "$s"; fi
}

ui_header() {
  [ -n "$UI_TOP" ] || return 0
  local title="$1" sub="${2:-}"
  printf '\n%s%s%s' "$UI_B" "$title" "$UI_0"
  [ -n "$sub" ] && printf '  %s%s%s' "$UI_D" "$sub" "$UI_0"
  printf '\n%s%s%s\n' "$UI_D" "────────────────────────────────────────────" "$UI_0"
}

ui_section() { printf '\n%s%s%s\n' "$UI_B" "$1" "$UI_0"; }
ui_ok() { printf '  %s✓%s %s\n' "$UI_G" "$UI_0" "$*"; }
ui_info() { printf '  %s·%s %s\n' "$UI_D" "$UI_0" "$*"; }
ui_warn() { printf '  %s!%s %s\n' "$UI_Y" "$UI_0" "$*"; }
ui_skip() { printf '  %s↷ %s%s %s%s%s\n' "$UI_D" "$1" "$UI_0" "$UI_D" "${2:+– $2}" "$UI_0"; }
ui_fail() {
  printf '\n  %s✗ %s%s\n' "$UI_R" "$*" "$UI_0"
  exit 1
}

ui_sudo() {
  [ "$(id -u)" = 0 ] && return 0
  sudo -n true 2>/dev/null && return 0
  printf '  %s🔑%s Administrator-Passwort (für die Installation):\n' "$UI_Y" "$UI_0"
  sudo -v || ui_fail "Ohne Administrator-Rechte geht es nicht weiter."
}

# ui_run "Label" command args...
ui_run() {
  local label="$1" start=$SECONDS rc=0 pid i=0 from
  shift
  printf '\n=== %s\n' "$label" >>"$UI_LOG"
  from=$(($(wc -l <"$UI_LOG") + 1))
  if [ -n "$UI_TTY" ]; then
    local frames=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
    ("$@") >>"$UI_LOG" 2>&1 &
    pid=$!
    while kill -0 "$pid" 2>/dev/null; do
      printf '\r  %s%s%s %s %s%s%s' "$UI_C" "${frames[i % 10]}" "$UI_0" "$label" "$UI_D" "$(ui_time $((SECONDS - start)))" "$UI_0"
      i=$((i + 1))
      sleep 0.1
    done
    wait "$pid" || rc=$?
    printf '\r\033[K'
  else
    ("$@") >>"$UI_LOG" 2>&1 || rc=$?
  fi
  if [ "$rc" -eq 0 ]; then
    printf '  %s✓%s %s %s(%s)%s\n' "$UI_G" "$UI_0" "$label" "$UI_D" "$(ui_time $((SECONDS - start)))" "$UI_0"
  else
    printf '  %s✗ %s%s\n\n' "$UI_R" "$label" "$UI_0"
    # The interesting part: errors and failed tests, else the last lines.
    local out
    out=$(tail -n +"$from" "$UI_LOG")
    if printf '%s\n' "$out" | grep -qE '^(error|test .* FAILED|---- )'; then
      printf '%s\n' "$out" | grep -E '^(error|test .* FAILED|---- )|panicked at' -A3 | tail -n 40 | sed 's/^/    /'
    else
      printf '%s\n' "$out" | tail -n 25 | sed 's/^/    /'
    fi
    printf '\n  %sGanzes Protokoll: %s%s\n' "$UI_D" "$UI_LOG" "$UI_0"
    exit 1
  fi
}

# Fingerprint of files (to tell whether an output is still current).
ui_fingerprint() {
  if command -v shasum >/dev/null 2>&1; then
    cat "$@" | shasum -a 256 | cut -c1-16
  else
    cat "$@" | sha256sum | cut -c1-16
  fi
}

# ui_fresh <output> <key>: the output exists and was made from <key>.
ui_fresh() {
  [ -f "$1" ] && [ -f "$(dirname "$1")/.made/$(basename "$1")" ] &&
    [ "$(cat "$(dirname "$1")/.made/$(basename "$1")")" = "$2" ]
}
# ui_made <output> <key>: remember what the output was made from.
ui_made() {
  mkdir -p "$(dirname "$1")/.made"
  printf '%s' "$2" >"$(dirname "$1")/.made/$(basename "$1")"
}

ui_done() {
  [ -n "$UI_TOP" ] || return 0
  printf '\n%s%s%s\n' "$UI_D" "Protokoll: $UI_LOG" "$UI_0"
}
